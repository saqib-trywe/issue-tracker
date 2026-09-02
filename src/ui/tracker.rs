//! Root view: owns the Store, the in-memory projection, and all selection
//! state. The three columns are rendered by sibling modules.

use std::time::Duration;

use gpui::*;
use gpui_component::input::{InputEvent, InputState, TextareaState};

use gpui_component::ThemeMode;
use gpui_component::select::{SelectEvent, SelectState};

use super::theme_catalogue::{ThemeCatalogue, ThemeListDelegate};
use crate::domain::{Issue, IssueId, Priority, Status, View, sort_for_display};
use crate::store::{Store, settings_keys};

/// How long editing pauses before an auto-save fires.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(400);

/// Key context for the issue list. Navigation bindings are scoped to it so
/// that typing `j` into a text input types a `j` rather than moving the
/// selection.
pub(super) const LIST_CONTEXT: &str = "IssueList";

actions!(
    issue_tracker,
    [
        SelectNext,
        SelectPrev,
        CreateIssue,
        EditIssue,
        DeleteIssue,
        FocusFilter,
        CancelEditing,
        ToggleSidebar,
        Quit,
        Hide,
        HideOthers,
        Minimize,
        Zoom,
        CloseWindow,
    ]
);

/// Switches to a View. Carries its payload so one action serves all six menu
/// items; `no_json` keeps `schemars` out of the dependency tree.
#[derive(Clone, PartialEq, Default, Debug, gpui::Action)]
#[action(namespace = issue_tracker, no_json)]
pub struct ShowView {
    pub view: View,
}

/// Reads a preference, treating a read failure as "unset" — a broken settings
/// row should cost you a theme, not the app.
fn read_setting(store: &Store, key: &str) -> Option<String> {
    match store.get_setting(key) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("failed to read setting {key}: {err:#}");
            None
        }
    }
}

/// Registers key bindings. Called once at startup.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, Some(LIST_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(LIST_CONTEXT)),
        KeyBinding::new("k", SelectPrev, Some(LIST_CONTEXT)),
        KeyBinding::new("up", SelectPrev, Some(LIST_CONTEXT)),
        KeyBinding::new("c", CreateIssue, Some(LIST_CONTEXT)),
        KeyBinding::new("e", EditIssue, Some(LIST_CONTEXT)),
        KeyBinding::new("x", DeleteIssue, Some(LIST_CONTEXT)),
        KeyBinding::new("/", FocusFilter, Some(LIST_CONTEXT)),
        KeyBinding::new("escape", CancelEditing, None),
        // Deliberately unscoped: unlike j/k/c these have to work while a text
        // input has focus, and cmd chords cannot collide with typing.
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-n", CreateIssue, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-h", Hide, None),
    ]);
}

pub struct IssueTracker {
    store: Store,
    /// Every Issue, display-sorted. The render path reads only from here.
    issues: Vec<Issue>,
    view: View,
    selected: Option<IssueId>,
    /// Substring filter over titles, driven by the list header input.
    filter: String,

    pub(super) title_input: Entity<InputState>,
    pub(super) body_input: Entity<TextareaState>,
    pub(super) filter_input: Entity<InputState>,
    pub(super) new_issue_input: Entity<InputState>,
    /// Whether the inline "new issue" row is showing.
    pub(super) creating: bool,
    /// Hiding the sidebar also hides the View list and the theme pickers;
    /// `cmd-b` brings them back.
    pub(super) sidebar_hidden: bool,

    pub(super) list_focus: FocusHandle,
    /// Replaced on every keystroke; dropping the previous task cancels it,
    /// which is what makes the save debounced.
    save_task: Option<Task<()>>,
    /// Same debounce trick for View/selection/filter, which change far too
    /// often to write through on every keypress.
    ui_state_task: Option<Task<()>>,

    /// Every embedded theme, split by mode. Held so a selection can be
    /// resolved back to the config it applies.
    catalogue: ThemeCatalogue,
    pub(super) light_select: Entity<SelectState<ThemeListDelegate>>,
    pub(super) dark_select: Entity<SelectState<ThemeListDelegate>>,

    /// Subscriptions die with the view if not held here.
    _subscriptions: Vec<Subscription>,
}

impl IssueTracker {
    pub fn new(store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut issues = store.load_all().unwrap_or_else(|err| {
            eprintln!("failed to load issues: {err:#}");
            Vec::new()
        });
        sort_for_display(&mut issues);
        // Resolved properly once the restored View and filter are known.
        let selected: Option<IssueId> = None;

        let title_input = cx.new(|cx| InputState::new(window, cx).placeholder("Issue title"));
        let body_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(8, 40)
                .placeholder("Describe the work…")
        });
        // Themes: resolve the persisted choices, falling back to the built-in
        // defaults so a first run looks exactly as it did before this feature.
        let catalogue = ThemeCatalogue::load();
        let stored_light = read_setting(&store, settings_keys::THEME_LIGHT);
        let stored_dark = read_setting(&store, settings_keys::THEME_DARK);
        // Anything other than "true" — absent or malformed included — means
        // visible, which is the safer state to land on.
        let sidebar_hidden = read_setting(&store, settings_keys::SIDEBAR_HIDDEN)
            .is_some_and(|value| value == "true");

        // Working state from the last session. Anything unparseable is
        // treated as absent rather than fatal.
        let view = read_setting(&store, settings_keys::UI_VIEW)
            .and_then(|value| value.parse::<View>().ok())
            .unwrap_or_default();
        let filter = read_setting(&store, settings_keys::UI_FILTER).unwrap_or_default();
        let restored_selection = read_setting(&store, settings_keys::UI_SELECTED)
            .and_then(|value| value.parse::<IssueId>().ok());

        let light_config = stored_light
            .as_deref()
            .and_then(|name| catalogue.find(ThemeMode::Light, name))
            .map(|choice| choice.config.clone());
        let dark_config = stored_dark
            .as_deref()
            .and_then(|name| catalogue.find(ThemeMode::Dark, name))
            .map(|choice| choice.config.clone());

        if light_config.is_some() || dark_config.is_some() {
            super::theme::apply_themes(light_config, dark_config, window, cx);
        }

        let light_delegate = ThemeListDelegate::new(catalogue.for_mode(ThemeMode::Light).to_vec());
        let dark_delegate = ThemeListDelegate::new(catalogue.for_mode(ThemeMode::Dark).to_vec());
        let light_index = stored_light
            .as_deref()
            .and_then(|name| light_delegate.index_of(name));
        let dark_index = stored_dark
            .as_deref()
            .and_then(|name| dark_delegate.index_of(name));

        let light_select = cx.new(|cx| SelectState::new(light_delegate, light_index, window, cx));
        let dark_select = cx.new(|cx| SelectState::new(dark_delegate, dark_index, window, cx));

        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter titles…"));
        let new_issue_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("New issue title…"));

        let subscriptions = vec![
            cx.subscribe_in(
                &title_input,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.schedule_save(cx);
                    }
                },
            ),
            cx.subscribe_in(&body_input, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.schedule_save(cx);
                }
            }),
            cx.subscribe_in(
                &filter_input,
                window,
                |this, input, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.filter = input.read(cx).value().to_string();
                        this.ensure_selection_visible();
                        this.schedule_ui_state_save(cx);
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &new_issue_input,
                window,
                |this, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        let title = input.read(cx).value().to_string();
                        this.commit_new_issue(title, window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &light_select,
                window,
                |this, _, event: &SelectEvent<ThemeListDelegate>, window, cx| {
                    let SelectEvent::Confirm(Some(name)) = event else {
                        return;
                    };
                    this.choose_theme(ThemeMode::Light, &name.clone(), window, cx);
                },
            ),
            cx.subscribe_in(
                &dark_select,
                window,
                |this, _, event: &SelectEvent<ThemeListDelegate>, window, cx| {
                    let SelectEvent::Confirm(Some(name)) = event else {
                        return;
                    };
                    this.choose_theme(ThemeMode::Dark, &name.clone(), window, cx);
                },
            ),
            // Chrome rather than issues, but this is the only object that
            // lives as long as the window, so it holds the subscription.
            super::theme::observe_system_appearance(window),
            // Closing the window drops this entity along with any pending
            // debounced write, so flush on the way out.
            cx.on_release(|this, cx| {
                if this.save_task.take().is_some()
                    && let Some(id) = this.selected
                {
                    this.write_edits(id, cx);
                }
                this.write_ui_state();
            }),
        ];

        let mut this = Self {
            store,
            issues,
            view,
            selected,
            filter,
            title_input,
            body_input,
            filter_input,
            new_issue_input,
            creating: false,
            sidebar_hidden,
            list_focus: cx.focus_handle(),
            save_task: None,
            ui_state_task: None,
            catalogue,
            light_select,
            dark_select,
            _subscriptions: subscriptions,
        };
        // Prefer the issue we were last on, but only if it still exists and
        // is visible under the restored View and filter — it may have been
        // deleted, or filtered out, since the window closed.
        let visible: Vec<IssueId> = this.visible_issues().iter().map(|i| i.id).collect();
        this.selected = restored_selection
            .filter(|id| visible.contains(id))
            .or_else(|| visible.first().copied());

        // The filter came back as a plain String; the input showing it has to
        // be told separately. `set_value` suppresses change events, so this
        // does not re-trigger the filter subscription.
        if !this.filter.is_empty() {
            let filter = this.filter.clone();
            this.filter_input
                .update(cx, |input, cx| input.set_value(filter, window, cx));
        }

        this.load_selected_into_inputs(window, cx);
        // Without this the window opens with nothing focused, so the list
        // bindings (j/k/c/e/x) are dead until something is clicked.
        this.list_focus.focus(window, cx);
        this
    }

    // ---- projection queries -------------------------------------------------

    /// Issues in the active View matching the filter, in display order.
    pub(super) fn visible_issues(&self) -> Vec<&Issue> {
        let needle = self.filter.trim().to_lowercase();
        self.issues
            .iter()
            .filter(|issue| self.view.contains(issue))
            .filter(|issue| needle.is_empty() || issue.title.to_lowercase().contains(&needle))
            .collect()
    }

    pub(super) fn active_view(&self) -> View {
        self.view
    }

    pub(super) fn selected_id(&self) -> Option<IssueId> {
        self.selected
    }

    pub(super) fn selected_issue(&self) -> Option<&Issue> {
        let id = self.selected?;
        self.issues.iter().find(|issue| issue.id == id)
    }

    /// How many Issues a View would show, ignoring the filter.
    pub(super) fn count_for(&self, view: View) -> usize {
        self.issues
            .iter()
            .filter(|issue| view.contains(issue))
            .count()
    }

    // ---- selection ----------------------------------------------------------

    pub(super) fn select_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        self.view = view;
        self.ensure_selection_visible();
        self.load_selected_into_inputs(window, cx);
        self.schedule_ui_state_save(cx);
        // The View menu carries a checkmark, so the tree has to be rebuilt
        // for it to follow the active View.
        super::menus::rebuild(view, cx);
        cx.notify();
    }

    pub(super) fn select_issue(
        &mut self,
        id: IssueId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected == Some(id) {
            return;
        }
        // Any pending edit belongs to the issue we're leaving, so flush it
        // rather than letting it land on the newly selected one.
        self.flush_pending_save(cx);
        self.selected = Some(id);
        self.load_selected_into_inputs(window, cx);
        self.schedule_ui_state_save(cx);
        cx.notify();
    }

    /// Keeps the selection inside the visible set after a View or filter change.
    fn ensure_selection_visible(&mut self) {
        let visible: Vec<IssueId> = self.visible_issues().iter().map(|i| i.id).collect();
        if self.selected.is_some_and(|id| visible.contains(&id)) {
            return;
        }
        self.selected = visible.first().copied();
    }

    fn move_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let visible: Vec<IssueId> = self.visible_issues().iter().map(|i| i.id).collect();
        if visible.is_empty() {
            return;
        }
        let current = self
            .selected
            .and_then(|id| visible.iter().position(|candidate| *candidate == id));
        let next = match current {
            Some(index) => (index as isize + delta).clamp(0, visible.len() as isize - 1) as usize,
            None => 0,
        };
        self.select_issue(visible[next], window, cx);
    }

    // ---- editing ------------------------------------------------------------

    /// Mirrors the selected Issue into the inputs. `set_value` deliberately
    /// suppresses change events, so this never triggers an auto-save.
    fn load_selected_into_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (title, body) = match self.selected_issue() {
            Some(issue) => (issue.title.clone(), issue.body.clone()),
            None => (String::new(), String::new()),
        };
        self.title_input
            .update(cx, |input, cx| input.set_value(title, window, cx));
        self.body_input
            .update(cx, |input, cx| input.set_value(body, window, cx));
    }

    /// Restarts the debounce window. The previous task is dropped, cancelling
    /// the save it was waiting to perform.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            this.update(cx, |this, cx| this.write_edits(id, cx)).ok();
        }));
    }

    /// Saves immediately, abandoning any pending debounce.
    fn flush_pending_save(&mut self, cx: &mut Context<Self>) {
        if self.save_task.take().is_some()
            && let Some(id) = self.selected
        {
            self.write_edits(id, cx);
            cx.notify();
        }
    }

    /// Writes the inputs through to SQLite, then updates the projection.
    ///
    /// Takes `&mut App` rather than `Context<Self>` so it can also run from
    /// `on_release`, where the entity is being torn down and no `Context`
    /// exists. Callers that are still live should `cx.notify()` afterwards.
    fn write_edits(&mut self, id: IssueId, cx: &mut App) {
        self.save_task = None;
        let title = self.title_input.read(cx).value().to_string();
        let body = self.body_input.read(cx).value().to_string();

        let Some(issue) = self.issues.iter_mut().find(|issue| issue.id == id) else {
            return;
        };
        if issue.title == title && issue.body == body {
            return;
        }
        issue.title = title;
        issue.body = body;

        let snapshot = issue.clone();
        match self.store.update(&snapshot) {
            Ok(updated_at) => {
                if let Some(issue) = self.issues.iter_mut().find(|issue| issue.id == id) {
                    issue.updated_at = updated_at;
                }
            }
            Err(err) => eprintln!("failed to save issue #{id}: {err:#}"),
        }
    }

    pub(super) fn set_status(
        &mut self,
        status: Status,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mutate_selected(cx, |issue| issue.status = status);
    }

    pub(super) fn set_priority(
        &mut self,
        priority: Priority,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mutate_selected(cx, |issue| issue.priority = priority);
    }

    /// Applies a change to the selected Issue and writes it through.
    fn mutate_selected(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Issue)) {
        let Some(id) = self.selected else { return };
        let Some(issue) = self.issues.iter_mut().find(|issue| issue.id == id) else {
            return;
        };
        change(issue);
        let snapshot = issue.clone();
        match self.store.update(&snapshot) {
            Ok(updated_at) => {
                if let Some(issue) = self.issues.iter_mut().find(|issue| issue.id == id) {
                    issue.updated_at = updated_at;
                }
            }
            Err(err) => eprintln!("failed to save issue #{id}: {err:#}"),
        }
        sort_for_display(&mut self.issues);
        // A status change can move the Issue out of the active View.
        self.ensure_selection_visible();
        cx.notify();
    }

    // ---- create -------------------------------------------------------------

    pub(super) fn start_creating(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.creating = true;
        self.new_issue_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn commit_new_issue(&mut self, title: String, window: &mut Window, cx: &mut Context<Self>) {
        let title = title.trim().to_string();
        if title.is_empty() {
            self.cancel_creating(window, cx);
            return;
        }
        match self.store.insert(&title) {
            Ok(issue) => {
                let id = issue.id;
                self.issues.push(issue);
                sort_for_display(&mut self.issues);
                self.selected = Some(id);
                // Stay in create mode so several issues can be typed in a row.
                self.new_issue_input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.load_selected_into_inputs(window, cx);
            }
            Err(err) => eprintln!("failed to create issue: {err:#}"),
        }
        cx.notify();
    }

    pub(super) fn cancel_creating(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.creating = false;
        self.new_issue_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.list_focus.focus(window, cx);
        cx.notify();
    }

    // ---- working state ------------------------------------------------------

    /// Queues a write of View, selection and filter.
    ///
    /// Debounced because these change on every `j`/`k` and every character
    /// typed into the filter, unlike the preferences which change rarely.
    fn schedule_ui_state_save(&mut self, cx: &mut Context<Self>) {
        self.ui_state_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            this.update(cx, |this, _| this.write_ui_state()).ok();
        }));
    }

    /// Writes working state immediately. Failures are logged, never fatal —
    /// losing your place is not worth interrupting anyone over.
    fn write_ui_state(&mut self) {
        self.ui_state_task = None;

        let selected = self.selected.map(|id| id.to_string()).unwrap_or_default();
        let values = [
            (settings_keys::UI_VIEW, self.view.to_string()),
            (settings_keys::UI_SELECTED, selected),
            (settings_keys::UI_FILTER, self.filter.clone()),
        ];

        for (key, value) in values {
            if let Err(err) = self.store.set_setting(key, &value) {
                eprintln!("failed to save {key}: {err:#}");
            }
        }
    }

    /// Flushes everything still sitting behind a debounce.
    ///
    /// Without this, quitting or closing the window within the debounce
    /// window silently discards the last edit. That path only became
    /// reachable once `cmd-q` started working.
    pub(super) fn flush_pending_writes(&mut self, cx: &mut Context<Self>) {
        self.flush_pending_save(cx);
        if self.ui_state_task.is_some() {
            self.write_ui_state();
        }
    }

    // ---- sidebar ------------------------------------------------------------

    /// Shows or hides the sidebar.
    ///
    /// Unlike [`Self::choose_theme`], a failed write does not abort the
    /// change: this is view state, and the user's immediate intent should not
    /// be held hostage by a settings write. The worst case is that the next
    /// launch disagrees, which is recoverable with another `cmd-b`.
    pub(super) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_hidden = !self.sidebar_hidden;
        cx.notify();

        let value = if self.sidebar_hidden { "true" } else { "false" };
        if let Err(err) = self.store.set_setting(settings_keys::SIDEBAR_HIDDEN, value) {
            eprintln!("failed to save sidebar visibility: {err:#}");
        }
    }

    pub(super) fn sidebar_hidden(&self) -> bool {
        self.sidebar_hidden
    }

    fn on_toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_sidebar(cx);
    }

    // ---- application ---------------------------------------------------------

    fn on_quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        // Flush before asking the app to go away, or the last 400ms of
        // editing is lost.
        self.flush_pending_writes(cx);
        cx.quit();
    }

    fn on_hide(&mut self, _: &Hide, _: &mut Window, cx: &mut Context<Self>) {
        cx.hide();
    }

    fn on_hide_others(&mut self, _: &HideOthers, _: &mut Window, cx: &mut Context<Self>) {
        cx.hide_other_apps();
    }

    fn on_minimize(&mut self, _: &Minimize, window: &mut Window, _: &mut Context<Self>) {
        window.minimize_window();
    }

    fn on_zoom(&mut self, _: &Zoom, window: &mut Window, _: &mut Context<Self>) {
        window.zoom_window();
    }

    fn on_close_window(&mut self, _: &CloseWindow, window: &mut Window, cx: &mut Context<Self>) {
        // The app stays alive with no window, so this has to flush too.
        self.flush_pending_writes(cx);
        window.remove_window();
    }

    fn on_show_view(&mut self, action: &ShowView, window: &mut Window, cx: &mut Context<Self>) {
        self.select_view(action.view, window, cx);
    }

    // ---- appearance ---------------------------------------------------------

    /// Persists a theme choice and applies it.
    ///
    /// Writes through to the store *before* touching the live theme, mirroring
    /// the ordering discipline in ADR-0002 — if the write fails the app keeps
    /// the appearance the database can actually reproduce on next launch.
    pub(super) fn choose_theme(
        &mut self,
        mode: ThemeMode,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(choice) = self.catalogue.find(mode, name) else {
            eprintln!("theme {name:?} is not in the catalogue");
            return;
        };
        let config = choice.config.clone();

        let key = match mode {
            ThemeMode::Light => settings_keys::THEME_LIGHT,
            ThemeMode::Dark => settings_keys::THEME_DARK,
        };
        if let Err(err) = self.store.set_setting(key, name) {
            eprintln!("failed to save {key}: {err:#}");
            return;
        }

        match mode {
            ThemeMode::Light => super::theme::apply_themes(Some(config), None, window, cx),
            ThemeMode::Dark => super::theme::apply_themes(None, Some(config), window, cx),
        }
        cx.notify();
    }

    // ---- delete -------------------------------------------------------------

    /// Erases an Issue. Guarded by a confirmation dialog at the call site —
    /// cancelling an Issue is a Status change, this is for mistakes.
    pub(super) fn delete_issue(
        &mut self,
        id: IssueId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Don't let a queued auto-save resurrect what we're about to erase.
        self.save_task = None;
        if let Err(err) = self.store.delete(id) {
            eprintln!("failed to delete issue #{id}: {err:#}");
            return;
        }
        self.issues.retain(|issue| issue.id != id);
        if self.selected == Some(id) {
            self.selected = None;
            self.ensure_selection_visible();
            self.load_selected_into_inputs(window, cx);
        }
        cx.notify();
    }

    // ---- actions ------------------------------------------------------------

    fn on_select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, window, cx);
    }

    fn on_select_prev(&mut self, _: &SelectPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, window, cx);
    }

    fn on_create_issue(&mut self, _: &CreateIssue, window: &mut Window, cx: &mut Context<Self>) {
        self.start_creating(window, cx);
    }

    fn on_edit_issue(&mut self, _: &EditIssue, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_some() {
            self.title_input
                .update(cx, |input, cx| input.focus(window, cx));
        }
    }

    fn on_delete_issue(&mut self, _: &DeleteIssue, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(issue) = self.selected_issue() {
            let id = issue.id;
            let title = issue.display_title().to_string();
            self.confirm_delete(id, title, window, cx);
        }
    }

    fn on_focus_filter(&mut self, _: &FocusFilter, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    /// Escape always returns to the list, from wherever focus currently is.
    fn on_cancel_editing(
        &mut self,
        _: &CancelEditing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.creating {
            self.cancel_creating(window, cx);
            return;
        }
        self.flush_pending_save(cx);
        self.list_focus.focus(window, cx);
        cx.notify();
    }
}

impl Render for IssueTracker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `Option` is an iterator of zero or one, so this drops the sidebar
        // out of the tree entirely rather than rendering it at zero width.
        let sidebar = if self.sidebar_hidden {
            None
        } else {
            Some(self.render_sidebar(cx))
        };

        div()
            .key_context("IssueTracker")
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_create_issue))
            .on_action(cx.listener(Self::on_edit_issue))
            .on_action(cx.listener(Self::on_delete_issue))
            .on_action(cx.listener(Self::on_focus_filter))
            .on_action(cx.listener(Self::on_cancel_editing))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_hide))
            .on_action(cx.listener(Self::on_hide_others))
            .on_action(cx.listener(Self::on_minimize))
            .on_action(cx.listener(Self::on_zoom))
            .on_action(cx.listener(Self::on_close_window))
            .on_action(cx.listener(Self::on_show_view))
            .size_full()
            .flex()
            .flex_row()
            .children(sidebar)
            .child(self.render_issue_list(window, cx))
            .child(self.render_issue_detail(window, cx))
    }
}
