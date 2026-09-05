// SPDX-License-Identifier: GPL-3.0-only

//! Root view: selection, filters and editing state, rendered from the shared
//! Projection. The three columns are rendered by sibling modules.
//!
//! This view deliberately does *not* own the issues. They live in an
//! app-level `Entity<Projection>` that outlives the window and that the HTTP
//! API writes to as well; the view observes it and re-settles itself on every
//! change, whoever made it. See `docs/adr/0005`.

use std::collections::BTreeSet;
use std::time::Duration;

use gpui::*;
use gpui_component::input::{InputEvent, InputState, TextareaState};

use gpui_component::ThemeMode;
use gpui_component::combobox::{ComboboxEvent, ComboboxState};
use gpui_component::searchable_list::SearchableVec;
use gpui_component::select::{SelectEvent, SelectState};

use super::theme_catalogue::{ThemeCatalogue, ThemeListDelegate};
use issue_tracker::domain::{Issue, IssueId, Priority, Status, Tag, View};
use issue_tracker::projection::{IssuePatch, Projection, Written};
use issue_tracker::store::settings_keys;

/// The Tag editor in the detail pane. Tags are plain names, so the delegate is
/// the library's own `SearchableVec` and no custom one is needed.
pub(super) type TagCombobox = ComboboxState<SearchableVec<String>>;

/// A picker over Issues. Entries read `#12 Title`, optionally annotated with
/// the parent they would be taken from.
pub(super) type IssueCombobox = ComboboxState<SearchableVec<String>>;

/// How an Issue is spelled inside a picker.
fn issue_label(issue: &Issue) -> String {
    format!("#{} {}", issue.id, issue.display_title())
}

/// The same, annotated when the Issue already belongs to someone.
///
/// Attaching it would move it, so the row has to say what it would be taken
/// from — without that this would be a silent theft from another Issue.
fn issue_label_annotated(issue: &Issue, projection: &Projection) -> String {
    match issue.parent_id.and_then(|id| projection.get(id)) {
        Some(parent) => format!("{} · under #{}", issue_label(issue), parent.id),
        None => issue_label(issue),
    }
}

/// Recovers the id from a picker entry.
fn id_from_label(label: &str) -> Option<IssueId> {
    label.strip_prefix('#')?.split(' ').next()?.parse().ok()
}

fn issue_items(labels: Vec<String>) -> SearchableVec<String> {
    SearchableVec::new(labels)
}

fn tag_items(in_use: &BTreeSet<Tag>) -> SearchableVec<String> {
    SearchableVec::new(
        in_use
            .iter()
            .map(|tag| tag.as_str().to_owned())
            .collect::<Vec<_>>(),
    )
}

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
        FocusTags,
        FocusSubIssues,
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
fn read_setting(projection: &Projection, key: &str) -> Option<String> {
    match projection.setting(key) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("failed to read setting {key}: {err:#}");
            None
        }
    }
}

/// Every open window's view, so a pending edit can be flushed before the API
/// writes. Weak, because a window closing must not be prevented by this list.
#[derive(Default)]
struct OpenTrackers(Vec<WeakEntity<IssueTracker>>);

impl Global for OpenTrackers {}

/// Lands every window's debounced edit immediately.
///
/// Called before the API applies a write. A pending edit belongs to an earlier
/// moment than the request now arriving, so it goes first and the API's
/// change — being later — wins. Without this the debounce would fire *after*
/// the API write and silently undo it.
pub fn flush_pending_edits(cx: &mut App) {
    let open = cx.default_global::<OpenTrackers>().0.clone();
    let mut live = Vec::with_capacity(open.len());
    for weak in open {
        if let Some(tracker) = weak.upgrade() {
            tracker.update(cx, |this, cx| this.flush_pending_save(cx));
            live.push(weak);
        }
    }
    // Closed windows are dropped here rather than accumulating forever.
    cx.default_global::<OpenTrackers>().0 = live;
}

/// Registers key bindings. Called once at startup.
pub fn init(cx: &mut App) {
    cx.default_global::<OpenTrackers>();
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, Some(LIST_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(LIST_CONTEXT)),
        KeyBinding::new("k", SelectPrev, Some(LIST_CONTEXT)),
        KeyBinding::new("up", SelectPrev, Some(LIST_CONTEXT)),
        KeyBinding::new("c", CreateIssue, Some(LIST_CONTEXT)),
        KeyBinding::new("e", EditIssue, Some(LIST_CONTEXT)),
        KeyBinding::new("x", DeleteIssue, Some(LIST_CONTEXT)),
        KeyBinding::new("/", FocusFilter, Some(LIST_CONTEXT)),
        KeyBinding::new("t", FocusTags, Some(LIST_CONTEXT)),
        KeyBinding::new("s", FocusSubIssues, Some(LIST_CONTEXT)),
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
    /// The shared issue store. Not owned — see the module docs.
    projection: Entity<Projection>,
    view: View,
    selected: Option<IssueId>,
    /// Substring filter over titles, driven by the list header input.
    filter: String,
    /// Narrows the list to one Tag *within* the active View. Deliberately not
    /// a `View` variant: a View is predefined, a Tag filter is whatever the
    /// user invented. See docs/adr/0004.
    tag_filter: Option<Tag>,

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
    pub(super) tag_select: Entity<TagCombobox>,
    /// Adds a sub-issue to the selected Issue.
    pub(super) sub_issue_select: Entity<IssueCombobox>,
    /// Chooses the selected Issue's parent. Clearing it detaches; choosing a
    /// different one moves it.
    pub(super) parent_select: Entity<IssueCombobox>,
    pub(super) light_select: Entity<SelectState<ThemeListDelegate>>,
    pub(super) dark_select: Entity<SelectState<ThemeListDelegate>>,

    /// Subscriptions die with the view if not held here.
    _subscriptions: Vec<Subscription>,
}

impl IssueTracker {
    pub fn new(
        projection: Entity<Projection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Settings and Tag vocabulary are read up front, in a single borrow,
        // so it has ended before any entity is constructed below.
        let stored = {
            let p = projection.read(cx);
            RestoredState {
                light: read_setting(p, settings_keys::THEME_LIGHT),
                dark: read_setting(p, settings_keys::THEME_DARK),
                // Anything other than "true" — absent or malformed included —
                // means visible, which is the safer state to land on.
                sidebar_hidden: read_setting(p, settings_keys::SIDEBAR_HIDDEN)
                    .is_some_and(|value| value == "true"),
                // Working state from the last session. Anything unparseable is
                // treated as absent rather than fatal.
                view: read_setting(p, settings_keys::UI_VIEW)
                    .and_then(|value| value.parse::<View>().ok())
                    .unwrap_or_default(),
                filter: read_setting(p, settings_keys::UI_FILTER).unwrap_or_default(),
                tag: read_setting(p, settings_keys::UI_TAG)
                    .and_then(|value| value.parse::<Tag>().ok()),
                selection: read_setting(p, settings_keys::UI_SELECTED)
                    .and_then(|value| value.parse::<IssueId>().ok()),
                in_use: p.tags_in_use(),
            }
        };

        // A stored Tag that nothing carries any more is dropped rather than
        // restored: it would filter the list down to nothing, and the sidebar
        // row you would clear it from no longer exists.
        let tag_filter = stored.tag.filter(|tag| stored.in_use.contains(tag));

        let title_input = cx.new(|cx| InputState::new(window, cx).placeholder("Issue title"));
        let body_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(8, 40)
                .placeholder("Describe the work…")
        });

        // Themes: resolve the persisted choices, falling back to the built-in
        // defaults so a first run looks exactly as it did before this feature.
        let catalogue = ThemeCatalogue::load();
        let light_config = stored
            .light
            .as_deref()
            .and_then(|name| catalogue.find(ThemeMode::Light, name))
            .map(|choice| choice.config.clone());
        let dark_config = stored
            .dark
            .as_deref()
            .and_then(|name| catalogue.find(ThemeMode::Dark, name))
            .map(|choice| choice.config.clone());

        if light_config.is_some() || dark_config.is_some() {
            super::theme::apply_themes(light_config, dark_config, window, cx);
        }

        let light_delegate = ThemeListDelegate::new(catalogue.for_mode(ThemeMode::Light).to_vec());
        let dark_delegate = ThemeListDelegate::new(catalogue.for_mode(ThemeMode::Dark).to_vec());
        let light_index = stored
            .light
            .as_deref()
            .and_then(|name| light_delegate.index_of(name));
        let dark_index = stored
            .dark
            .as_deref()
            .and_then(|name| dark_delegate.index_of(name));

        let light_select = cx.new(|cx| SelectState::new(light_delegate, light_index, window, cx));
        let dark_select = cx.new(|cx| SelectState::new(dark_delegate, dark_index, window, cx));

        let tag_select = cx.new(|cx| {
            ComboboxState::new(tag_items(&stored.in_use), Vec::new(), window, cx)
                .multiple(true)
                .searchable(true)
        });

        let sub_issue_select = cx.new(|cx| {
            ComboboxState::new(issue_items(Vec::new()), Vec::new(), window, cx).searchable(true)
        });
        let parent_select = cx.new(|cx| {
            ComboboxState::new(issue_items(Vec::new()), Vec::new(), window, cx).searchable(true)
        });

        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter titles…"));
        let new_issue_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("New issue title…"));

        let subscriptions = vec![
            // Every change to the shared store lands here, whether this window
            // made it or the HTTP API did.
            cx.observe_in(&projection, window, |this, _, window, cx| {
                this.on_projection_changed(window, cx);
            }),
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
                |this, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.filter = input.read(cx).value().to_string();
                        this.ensure_selection_visible(cx);
                        this.refresh_inputs(true, window, cx);
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
            // Fires on every toggle in the dropdown, add or remove.
            cx.subscribe_in(
                &tag_select,
                window,
                |this, _, event: &ComboboxEvent<SearchableVec<String>>, _, cx| {
                    let ComboboxEvent::Change(values) = event else {
                        return;
                    };
                    this.apply_tag_selection(values.clone(), cx);
                },
            ),
            cx.subscribe_in(
                &sub_issue_select,
                window,
                |this, _, event: &ComboboxEvent<SearchableVec<String>>, window, cx| {
                    let ComboboxEvent::Change(values) = event else {
                        return;
                    };
                    if let Some(child) = values.first().and_then(|label| id_from_label(label)) {
                        this.attach_sub_issue(child, window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &parent_select,
                window,
                |this, _, event: &ComboboxEvent<SearchableVec<String>>, window, cx| {
                    let ComboboxEvent::Change(values) = event else {
                        return;
                    };
                    // An empty selection is the clear affordance: detach.
                    let parent = values.first().and_then(|label| id_from_label(label));
                    this.set_parent(parent, window, cx);
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
                this.write_ui_state(cx);
            }),
        ];

        let mut this = Self {
            projection,
            view: stored.view,
            // Resolved properly once the restored View and filter are known.
            selected: None,
            filter: stored.filter,
            tag_filter,
            title_input,
            body_input,
            filter_input,
            new_issue_input,
            creating: false,
            sidebar_hidden: stored.sidebar_hidden,
            list_focus: cx.focus_handle(),
            save_task: None,
            ui_state_task: None,
            catalogue,
            tag_select,
            sub_issue_select,
            parent_select,
            light_select,
            dark_select,
            _subscriptions: subscriptions,
        };
        // Prefer the issue we were last on, but only if it still exists and
        // is visible under the restored View and filter — it may have been
        // deleted, or filtered out, since the window closed.
        let visible: Vec<IssueId> = this.visible_issues(cx).iter().map(|i| i.id).collect();
        this.selected = stored
            .selection
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

        this.refresh_inputs(true, window, cx);
        // Without this the window opens with nothing focused, so the list
        // bindings (j/k/c/e/x) are dead until something is clicked.
        this.list_focus.focus(window, cx);

        let weak = cx.weak_entity();
        cx.default_global::<OpenTrackers>().0.push(weak);
        this
    }

    // ---- projection queries -------------------------------------------------

    /// Issues in the active View matching the filter, in display order.
    pub(super) fn visible_issues<'a>(&self, cx: &'a App) -> Vec<&'a Issue> {
        let needle = self.filter.trim().to_lowercase();
        let view = self.view;
        let tag_filter = self.tag_filter.clone();
        self.projection
            .read(cx)
            .issues()
            .iter()
            .filter(move |issue| view.contains(issue))
            .filter(|issue| needle.is_empty() || issue.title.to_lowercase().contains(&needle))
            .filter(move |issue| {
                tag_filter
                    .as_ref()
                    .is_none_or(|tag| issue.tags.contains(tag))
            })
            .collect()
    }

    /// Every Tag carried by some Issue, sorted, first-spelling-wins.
    pub(super) fn tags_in_use(&self, cx: &App) -> BTreeSet<Tag> {
        self.projection.read(cx).tags_in_use()
    }

    pub(super) fn active_tag(&self) -> Option<&Tag> {
        self.tag_filter.as_ref()
    }

    /// How many Issues carry a Tag, ignoring the View and the filter — the
    /// Tag equivalent of [`Self::count_for`].
    pub(super) fn count_for_tag(&self, tag: &Tag, cx: &App) -> usize {
        self.projection.read(cx).count_with_tag(tag)
    }

    pub(super) fn active_view(&self) -> View {
        self.view
    }

    pub(super) fn selected_id(&self) -> Option<IssueId> {
        self.selected
    }

    pub(super) fn selected_issue<'a>(&self, cx: &'a App) -> Option<&'a Issue> {
        self.projection.read(cx).get(self.selected?)
    }

    /// The selected Issue's parts, in display order.
    pub(super) fn selected_sub_issues(&self, cx: &App) -> Vec<Issue> {
        let Some(id) = self.selected else {
            return Vec::new();
        };
        self.projection
            .read(cx)
            .sub_issues(id)
            .into_iter()
            .cloned()
            .collect()
    }

    /// The Issue the selected one is part of, if any.
    pub(super) fn selected_parent(&self, cx: &App) -> Option<Issue> {
        let parent = self.selected_issue(cx)?.parent_id?;
        self.projection.read(cx).get(parent).cloned()
    }

    /// Settled-over-total for an Issue's parts, or `None` when it has none.
    pub(super) fn settled_progress(&self, id: IssueId, cx: &App) -> Option<(usize, usize)> {
        self.projection.read(cx).settled_progress(id)
    }

    /// How many Issues a View would show, ignoring the filter.
    pub(super) fn count_for(&self, view: View, cx: &App) -> usize {
        self.projection
            .read(cx)
            .issues()
            .iter()
            .filter(|issue| view.contains(issue))
            .count()
    }

    // ---- reacting to the shared store ---------------------------------------

    /// Re-settles this window after *any* change to the shared Projection.
    ///
    /// UI-initiated and API-initiated writes both arrive here, which is what
    /// keeps them consistent: there is no separate "and the API also needs
    /// to…" path to forget about.
    fn on_projection_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_selected = self.selected;
        self.prune_tag_filter(cx);
        self.ensure_selection_visible(cx);
        let selection_moved = was_selected != self.selected;
        self.refresh_inputs(selection_moved, window, cx);
        cx.notify();
    }

    /// Mirrors the selected Issue into the inputs. `set_value` deliberately
    /// suppresses change events, so this never triggers an auto-save.
    ///
    /// A *focused* input is left alone unless `force` — meaning the selection
    /// itself moved. An external write landing mid-sentence must not eat the
    /// characters under the cursor; an unfocused input, by contrast, is always
    /// refreshed, because stale text there would be written straight back over
    /// that same change on the next keystroke.
    fn refresh_inputs(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (title, body) = match self.selected_issue(cx) {
            Some(issue) => (issue.title.clone(), issue.body.clone()),
            None => (String::new(), String::new()),
        };

        let title_focused = self
            .title_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let body_focused = self.body_input.read(cx).focus_handle(cx).is_focused(window);

        if force || !title_focused {
            self.title_input
                .update(cx, |input, cx| input.set_value(title, window, cx));
        }
        if force || !body_focused {
            self.body_input
                .update(cx, |input, cx| input.set_value(body, window, cx));
        }

        // The pickers are part of "what the selected Issue looks like", so
        // they are re-pointed here rather than by each caller. Leaving this to
        // callers is exactly how the tag box and the parent box came to show
        // the previously selected Issue's answers.
        self.sync_tag_select(window, cx);
        self.sync_relation_selects(window, cx);
    }

    /// Applies a change to the shared Projection and notifies its observers.
    ///
    /// That notification is what drives [`Self::on_projection_changed`], so a
    /// write made here re-settles the view by exactly the path an API write
    /// takes.
    fn write<T>(
        &self,
        cx: &mut App,
        change: impl FnOnce(&mut Projection) -> Written<T>,
    ) -> Option<T> {
        self.projection
            .update(cx, |projection, cx| match change(projection) {
                Ok(value) => {
                    cx.notify();
                    Some(value)
                }
                Err(err) => {
                    // Refusals are mostly unreachable from the UI, which
                    // disables or filters out the controls that would cause
                    // them; anything arriving here is worth seeing.
                    eprintln!("failed to write to the issue store: {err}");
                    None
                }
            })
    }

    // ---- selection ----------------------------------------------------------

    pub(super) fn select_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        self.view = view;
        self.ensure_selection_visible(cx);
        self.refresh_inputs(true, window, cx);
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
        self.refresh_inputs(true, window, cx);
        self.schedule_ui_state_save(cx);
        cx.notify();
    }

    /// Selects a Tag filter, or clears it when the active Tag is picked again.
    pub(super) fn toggle_tag_filter(
        &mut self,
        tag: Tag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tag_filter = if self.tag_filter.as_ref() == Some(&tag) {
            None
        } else {
            Some(tag)
        };
        self.ensure_selection_visible(cx);
        self.refresh_inputs(true, window, cx);
        self.schedule_ui_state_save(cx);
        cx.notify();
    }

    /// Drops a Tag filter whose Tag no longer exists.
    ///
    /// A derived Tag vanishes the moment the last Issue lets go of it. A
    /// filter still pointing at one would show an empty list with no way back,
    /// because the sidebar row you would click to clear it is gone too.
    fn prune_tag_filter(&mut self, cx: &App) {
        if let Some(tag) = &self.tag_filter
            && !self
                .projection
                .read(cx)
                .issues()
                .iter()
                .any(|issue| issue.tags.contains(tag))
        {
            self.tag_filter = None;
        }
    }

    /// Keeps the selection inside the visible set after a View or filter change.
    fn ensure_selection_visible(&mut self, cx: &App) {
        let visible: Vec<IssueId> = self.visible_issues(cx).iter().map(|i| i.id).collect();
        if self.selected.is_some_and(|id| visible.contains(&id)) {
            return;
        }
        self.selected = visible.first().copied();
    }

    fn move_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let visible: Vec<IssueId> = self.visible_issues(cx).iter().map(|i| i.id).collect();
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
    ///
    /// Also runs before the API applies a write, so a half-typed title reaches
    /// the store before anything else touches that Issue.
    pub(super) fn flush_pending_save(&mut self, cx: &mut Context<Self>) {
        if self.save_task.take().is_some()
            && let Some(id) = self.selected
        {
            self.write_edits(id, cx);
            cx.notify();
        }
    }

    /// Writes the title and body inputs through to the shared store.
    ///
    /// Takes `&mut App` rather than `Context<Self>` so it can also run from
    /// `on_release`, where the entity is being torn down and no `Context`
    /// exists. Only these two fields are named, so this can never clobber a
    /// Status or a Tag that something else set in the meantime.
    fn write_edits(&mut self, id: IssueId, cx: &mut App) {
        self.save_task = None;
        let title = self.title_input.read(cx).value().to_string();
        let body = self.body_input.read(cx).value().to_string();
        self.write(cx, |projection| {
            projection.patch(id, IssuePatch::default().title(title).body(body))
        });
    }

    pub(super) fn set_status(
        &mut self,
        status: Status,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.patch_selected(IssuePatch::default().status(status), cx);
    }

    pub(super) fn set_priority(
        &mut self,
        priority: Priority,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.patch_selected(IssuePatch::default().priority(priority), cx);
    }

    /// Applies a patch to the selected Issue.
    ///
    /// Tagging goes through here like everything else, so it stamps
    /// `updated_at` and re-sorts exactly as a Status change does: tagging is
    /// an edit, not a free annotation.
    fn patch_selected(&mut self, patch: IssuePatch, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        self.write(cx, |projection| projection.patch(id, patch));
    }

    /// Applies whatever the Combobox now reports as selected.
    fn apply_tag_selection(&mut self, values: Vec<String>, cx: &mut Context<Self>) {
        let tags = values
            .iter()
            .filter_map(|value| value.parse().ok())
            .collect();
        self.patch_selected(IssuePatch::default().tags(tags), cx);
    }

    /// Adds the Tag currently typed into the Combobox's search box.
    ///
    /// The library's own recipe for this only appends the typed name to the
    /// dropdown list, leaving you to click the thing you just typed. Putting
    /// it straight on the Issue saves that second click.
    pub(super) fn create_tag_from_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.tag_select.read(cx).query(cx).to_string();
        let Ok(tag) = query.parse::<Tag>() else {
            return;
        };
        let Some(id) = self.selected else { return };
        self.write(cx, |projection| projection.add_tag(id, tag));
        self.sync_tag_select(window, cx);
    }

    /// Takes one Tag off the selected Issue, from its chip's remove button.
    pub(super) fn remove_tag(&mut self, tag: Tag, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        self.write(cx, |projection| projection.remove_tag(id, &tag));
        self.sync_tag_select(window, cx);
    }

    /// Files an existing Issue under the selected one.
    pub(super) fn attach_sub_issue(
        &mut self,
        child: IssueId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(parent) = self.selected else { return };
        self.write(cx, |projection| projection.set_parent(child, Some(parent)));
        self.sync_relation_selects(window, cx);
    }

    /// Removes one of the selected Issue's parts, which becomes top-level.
    pub(super) fn detach_sub_issue(
        &mut self,
        child: IssueId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.write(cx, |projection| projection.set_parent(child, None));
        self.sync_relation_selects(window, cx);
    }

    /// Files the selected Issue under `parent`, or removes it from whatever
    /// holds it. Choosing a different parent moves it; there is no separate
    /// move operation.
    pub(super) fn set_parent(
        &mut self,
        parent: Option<IssueId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected else { return };
        if self
            .selected_issue(cx)
            .is_some_and(|issue| issue.parent_id == parent)
        {
            return;
        }
        self.write(cx, |projection| projection.set_parent(id, parent));
        self.sync_relation_selects(window, cx);
    }

    /// Points both relationship pickers at what is currently possible.
    ///
    /// Candidates are filtered to what `Projection` would actually accept, so
    /// the refusals exist for the API's benefit rather than being reachable by
    /// clicking.
    fn sync_relation_selects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };

        let (children, parents, current_parent) = {
            let projection = self.projection.read(cx);
            (
                projection
                    .eligible_sub_issues(id)
                    .iter()
                    .map(|issue| issue_label_annotated(issue, projection))
                    .collect::<Vec<_>>(),
                projection
                    .eligible_parents(id)
                    .iter()
                    .map(|issue| issue_label(issue))
                    .collect::<Vec<_>>(),
                projection
                    .get(id)
                    .and_then(|issue| issue.parent_id)
                    .and_then(|parent| projection.get(parent))
                    .map(issue_label),
            )
        };

        self.sub_issue_select.update(cx, |state, cx| {
            state.set_items(issue_items(children), window, cx);
            // Nothing stays selected here: this control is an action, and the
            // Issue it just attached is no longer a candidate.
            state.set_selected_values(&[], window, cx);
        });
        self.parent_select.update(cx, |state, cx| {
            state.set_items(issue_items(parents), window, cx);
            let selected: Vec<String> = current_parent.into_iter().collect();
            state.set_selected_values(&selected, window, cx);
        });
    }

    /// Points the Combobox at the current Tag vocabulary and the selected
    /// Issue's Tags.
    ///
    /// `set_selected_values` clears the search query as a side effect, so this
    /// runs on settle rather than on every keystroke.
    fn sync_tag_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = tag_items(&self.tags_in_use(cx));
        let selected: Vec<String> = self
            .selected_issue(cx)
            .map(|issue| {
                issue
                    .tags
                    .iter()
                    .map(|tag| tag.as_str().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        self.tag_select.update(cx, |state, cx| {
            state.set_items(items, window, cx);
            state.set_selected_values(&selected, window, cx);
        });
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
        if let Some(issue) = self.write(cx, |projection| {
            projection.create(&title, IssuePatch::default())
        }) {
            self.selected = Some(issue.id);
            // Stay in create mode so several issues can be typed in a row.
            self.new_issue_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.refresh_inputs(true, window, cx);
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
            this.update(cx, |this, cx| this.write_ui_state(cx)).ok();
        }));
    }

    /// Writes working state immediately. Failures are logged, never fatal —
    /// losing your place is not worth interrupting anyone over.
    fn write_ui_state(&mut self, cx: &App) {
        self.ui_state_task = None;

        let selected = self.selected.map(|id| id.to_string()).unwrap_or_default();
        let tag = self
            .tag_filter
            .as_ref()
            .map(|tag| tag.as_str().to_owned())
            .unwrap_or_default();
        let values = [
            (settings_keys::UI_VIEW, self.view.to_string()),
            (settings_keys::UI_SELECTED, selected),
            (settings_keys::UI_FILTER, self.filter.clone()),
            (settings_keys::UI_TAG, tag),
        ];

        for (key, value) in values {
            if let Err(err) = self.projection.read(cx).set_setting(key, &value) {
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
            self.write_ui_state(cx);
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
        if let Err(err) = self
            .projection
            .read(cx)
            .set_setting(settings_keys::SIDEBAR_HIDDEN, value)
        {
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
        if let Err(err) = self.projection.read(cx).set_setting(key, name) {
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
    ///
    /// Selection and Tag filter are re-settled by
    /// [`Self::on_projection_changed`], exactly as they would be had the API
    /// done the deleting.
    pub(super) fn delete_issue(
        &mut self,
        id: IssueId,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Don't let a queued auto-save resurrect what we're about to erase.
        self.save_task = None;
        self.write(cx, |projection| projection.delete(id));
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
        let Some((id, title)) = self
            .selected_issue(cx)
            .map(|issue| (issue.id, issue.display_title().to_string()))
        else {
            return;
        };
        let sub_issue_count = self.selected_sub_issues(cx).len();
        self.confirm_delete(id, title, sub_issue_count, window, cx);
    }

    fn on_focus_filter(&mut self, _: &FocusFilter, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    fn on_focus_tags(&mut self, _: &FocusTags, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_some() {
            self.tag_select
                .update(cx, |state, cx| state.focus(window, cx));
        }
    }

    fn on_focus_sub_issues(
        &mut self,
        _: &FocusSubIssues,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected.is_some() {
            self.sub_issue_select
                .update(cx, |state, cx| state.focus(window, cx));
        }
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

/// Everything restored from the `setting` table at startup, read in one borrow.
struct RestoredState {
    light: Option<String>,
    dark: Option<String>,
    sidebar_hidden: bool,
    view: View,
    filter: String,
    tag: Option<Tag>,
    selection: Option<IssueId>,
    in_use: BTreeSet<Tag>,
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
            .on_action(cx.listener(Self::on_focus_tags))
            .on_action(cx.listener(Self::on_focus_sub_issues))
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
