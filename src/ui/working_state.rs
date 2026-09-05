// SPDX-License-Identifier: GPL-3.0-only

//! What this window is looking at: the View, the Tag filter, the title
//! filter, and which Issue is selected.
//!
//! Free of `gpui` on purpose. Everything here needs the Issues in display
//! order and nothing else — no `Window`, no `App`, no `Projection`, no
//! `Store` — so a test builds a `Vec<Issue>` and exercises the real rules.
//! That matters because the rules are not obvious: a Tag filter can be left
//! pointing at a Tag that no longer exists, and a selection can fall out of
//! the visible set from either end.
//!
//! Working state is per window, as `docs/adr/0005` requires, which is why it
//! lives here rather than in the shared library.

use issue_tracker::domain::{Issue, IssueId, Tag, View};
use issue_tracker::store::settings_keys;

/// What changed, for the caller to act on.
///
/// The view does the GPUI work — refreshing inputs, scheduling a save,
/// notifying — because none of it belongs to the question "what am I looking
/// at". This says only whether the answer moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settled {
    pub selection_moved: bool,
}

impl Settled {
    fn moved(moved: bool) -> Self {
        Settled {
            selection_moved: moved,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct WorkingState {
    view: View,
    selected: Option<IssueId>,
    /// Substring filter over titles, driven by the list header input.
    filter: String,
    /// Narrows the list to one Tag *within* the active View. Deliberately not
    /// a `View` variant: a View is predefined, a Tag filter is whatever the
    /// user invented. See docs/adr/0004.
    tag_filter: Option<Tag>,
}

impl WorkingState {
    /// Restores last session's working state.
    ///
    /// Reading is the caller's job — this takes a lookup rather than a store,
    /// so the keys stay here with the fields they belong to. Anything
    /// unparseable is treated as absent rather than fatal: losing your place
    /// should not cost you the window.
    pub fn restore(issues: &[Issue], read: impl Fn(&str) -> Option<String>) -> Self {
        let mut state = WorkingState {
            view: read(settings_keys::UI_VIEW)
                .and_then(|value| value.parse::<View>().ok())
                .unwrap_or_default(),
            filter: read(settings_keys::UI_FILTER).unwrap_or_default(),
            tag_filter: read(settings_keys::UI_TAG).and_then(|value| value.parse::<Tag>().ok()),
            selected: read(settings_keys::UI_SELECTED)
                .and_then(|value| value.parse::<IssueId>().ok()),
        };
        // A stored Tag nothing carries any more, or an Issue that has since
        // been deleted or filtered out, is exactly what settling handles.
        state.settle(issues);
        state
    }

    /// The keys and values to persist. No I/O: the caller owns the writing,
    /// and this owns the list of what there is to write.
    pub fn settings(&self) -> Vec<(&'static str, String)> {
        vec![
            (settings_keys::UI_VIEW, self.view.to_string()),
            (
                settings_keys::UI_SELECTED,
                self.selected.map(|id| id.to_string()).unwrap_or_default(),
            ),
            (settings_keys::UI_FILTER, self.filter.clone()),
            (
                settings_keys::UI_TAG,
                self.tag_filter
                    .as_ref()
                    .map(|tag| tag.as_str().to_owned())
                    .unwrap_or_default(),
            ),
        ]
    }

    // ---- what is being looked at --------------------------------------------

    pub fn view(&self) -> View {
        self.view
    }

    pub fn tag(&self) -> Option<&Tag> {
        self.tag_filter.as_ref()
    }

    pub fn selected(&self) -> Option<IssueId> {
        self.selected
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Issues in the active View matching both filters, in display order.
    ///
    /// The three narrowings compose: a Tag filter narrows *within* a View,
    /// and the title filter narrows within both.
    pub fn visible<'a>(&self, issues: &'a [Issue]) -> Vec<&'a Issue> {
        let needle = self.filter.trim().to_lowercase();
        issues
            .iter()
            .filter(|issue| self.view.contains(issue))
            .filter(|issue| needle.is_empty() || issue.title.to_lowercase().contains(&needle))
            .filter(|issue| {
                self.tag_filter
                    .as_ref()
                    .is_none_or(|tag| issue.tags.contains(tag))
            })
            .collect()
    }

    /// How many Issues a View would show, ignoring both filters — which is
    /// why it takes no `self`: a sidebar badge counts the View, not the
    /// narrowing currently applied to it.
    pub fn count_for(view: View, issues: &[Issue]) -> usize {
        issues.iter().filter(|issue| view.contains(issue)).count()
    }

    // ---- changing what is being looked at ------------------------------------

    /// Re-settles after a change to the Issues themselves.
    ///
    /// Both halves are load-bearing. A derived Tag vanishes the moment the
    /// last Issue lets go of it, and a filter still pointing at one would show
    /// an empty list with no way back — the sidebar row you would clear it
    /// from is gone too. A selected Issue can likewise be deleted, or edited
    /// until it no longer matches.
    pub fn settle(&mut self, issues: &[Issue]) -> Settled {
        self.prune_tag(issues);
        self.reselect(issues)
    }

    pub fn select_view(&mut self, view: View, issues: &[Issue]) -> Settled {
        self.view = view;
        self.reselect(issues)
    }

    pub fn select_issue(&mut self, id: IssueId) -> Settled {
        if self.selected == Some(id) {
            return Settled::moved(false);
        }
        self.selected = Some(id);
        Settled::moved(true)
    }

    /// Applies a Tag filter, or clears it when the active Tag is picked again.
    pub fn toggle_tag(&mut self, tag: Tag, issues: &[Issue]) -> Settled {
        self.tag_filter = if self.tag_filter.as_ref() == Some(&tag) {
            None
        } else {
            Some(tag)
        };
        self.reselect(issues)
    }

    pub fn set_filter(&mut self, text: String, issues: &[Issue]) -> Settled {
        self.filter = text;
        self.reselect(issues)
    }

    /// Moves the selection by `delta` rows, stopping at either end rather
    /// than wrapping.
    pub fn move_selection(&mut self, delta: isize, issues: &[Issue]) -> Settled {
        let visible = self.visible_ids(issues);
        if visible.is_empty() {
            return Settled::moved(false);
        }
        let current = self
            .selected
            .and_then(|id| visible.iter().position(|candidate| *candidate == id));
        let next = match current {
            Some(index) => (index as isize + delta).clamp(0, visible.len() as isize - 1) as usize,
            None => 0,
        };
        self.select_issue(visible[next])
    }

    // ---- internals -----------------------------------------------------------

    fn visible_ids(&self, issues: &[Issue]) -> Vec<IssueId> {
        self.visible(issues).iter().map(|issue| issue.id).collect()
    }

    /// Drops a Tag filter whose Tag no longer exists.
    fn prune_tag(&mut self, issues: &[Issue]) {
        if let Some(tag) = &self.tag_filter
            && !issues.iter().any(|issue| issue.tags.contains(tag))
        {
            self.tag_filter = None;
        }
    }

    /// Keeps the selection inside the visible set, falling back to the first
    /// row rather than to nothing.
    fn reselect(&mut self, issues: &[Issue]) -> Settled {
        let before = self.selected;
        let visible = self.visible_ids(issues);
        if !self.selected.is_some_and(|id| visible.contains(&id)) {
            self.selected = visible.first().copied();
        }
        Settled::moved(before != self.selected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use issue_tracker::domain::{Priority, Status};
    use std::collections::HashMap;

    fn issue(id: IssueId, title: &str, status: Status, tags: &[&str]) -> Issue {
        Issue {
            id,
            title: title.to_string(),
            body: String::new(),
            status,
            priority: Priority::None,
            tags: tags.iter().map(|tag| tag.parse().unwrap()).collect(),
            parent_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn corpus() -> Vec<Issue> {
        vec![
            issue(1, "Fix the flash", Status::Doing, &["ui"]),
            issue(2, "Write the ADR", Status::Todo, &["docs"]),
            issue(3, "Ship the CLI", Status::Done, &["ui", "cli"]),
        ]
    }

    fn stored(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn the_three_narrowings_compose() {
        let issues = corpus();
        let mut state = WorkingState::default();
        assert_eq!(state.visible(&issues).len(), 3, "All Issues by default");

        // A Tag filter narrows within the View...
        state.toggle_tag("ui".parse().unwrap(), &issues);
        assert_eq!(state.visible(&issues).len(), 2);

        // ...the title filter narrows within both...
        state.set_filter("ship".into(), &issues);
        assert_eq!(state.visible(&issues).len(), 1);
        assert_eq!(state.visible(&issues)[0].id, 3);

        // ...and the View narrows all of it.
        state.select_view(View::WithStatus(Status::Todo), &issues);
        assert!(state.visible(&issues).is_empty());
    }

    #[test]
    fn the_title_filter_ignores_case_and_surrounding_space() {
        let issues = corpus();
        let mut state = WorkingState::default();
        state.set_filter("  FLASH  ".into(), &issues);
        assert_eq!(state.visible(&issues).len(), 1);
        assert_eq!(state.visible(&issues)[0].id, 1);
    }

    #[test]
    fn a_tag_filter_ignores_case_because_tag_identity_does() {
        let issues = corpus();
        let mut state = WorkingState::default();
        state.toggle_tag("UI".parse().unwrap(), &issues);
        assert_eq!(state.visible(&issues).len(), 2);
    }

    #[test]
    fn picking_the_active_tag_again_clears_it() {
        let issues = corpus();
        let mut state = WorkingState::default();
        let ui: Tag = "ui".parse().unwrap();

        state.toggle_tag(ui.clone(), &issues);
        assert_eq!(state.tag(), Some(&ui));
        state.toggle_tag(ui, &issues);
        assert_eq!(state.tag(), None);
    }

    #[test]
    fn a_tag_filter_is_pruned_when_its_last_issue_lets_go() {
        // The list would otherwise empty out, with the sidebar row you would
        // clear the filter from already gone. See docs/adr/0004.
        let mut issues = corpus();
        let mut state = WorkingState::default();
        state.toggle_tag("docs".parse().unwrap(), &issues);
        assert!(state.tag().is_some());

        issues[1].tags.clear();
        state.settle(&issues);

        assert_eq!(state.tag(), None);
        assert_eq!(state.visible(&issues).len(), 3);
    }

    #[test]
    fn a_selection_that_falls_out_of_view_moves_to_the_first_row() {
        let issues = corpus();
        let mut state = WorkingState::default();
        state.select_issue(3);

        let settled = state.select_view(View::WithStatus(Status::Doing), &issues);

        assert!(settled.selection_moved);
        assert_eq!(state.selected(), Some(1), "the first visible Issue");
    }

    #[test]
    fn a_selection_that_survives_does_not_move() {
        let issues = corpus();
        let mut state = WorkingState::default();
        state.select_issue(1);

        let settled = state.select_view(View::WithStatus(Status::Doing), &issues);

        assert!(!settled.selection_moved);
        assert_eq!(state.selected(), Some(1));
    }

    #[test]
    fn selecting_what_is_already_selected_changes_nothing() {
        let mut state = WorkingState::default();
        assert!(state.select_issue(7).selection_moved);
        assert!(!state.select_issue(7).selection_moved);
    }

    #[test]
    fn moving_the_selection_stops_at_both_ends() {
        let issues = corpus();
        let mut state = WorkingState::default();
        state.select_issue(1);

        state.move_selection(1, &issues);
        assert_eq!(state.selected(), Some(2));
        state.move_selection(-1, &issues);
        assert_eq!(state.selected(), Some(1));
        // Clamped rather than wrapped: k at the top stays at the top.
        state.move_selection(-1, &issues);
        assert_eq!(state.selected(), Some(1));
        state.move_selection(99, &issues);
        assert_eq!(state.selected(), Some(3));
    }

    #[test]
    fn moving_within_an_empty_list_is_a_no_op() {
        let issues = corpus();
        let mut state = WorkingState::default();
        state.set_filter("matches nothing".into(), &issues);

        let settled = state.move_selection(1, &issues);

        assert!(!settled.selection_moved);
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn restoring_prefers_the_stored_issue_but_only_if_it_is_visible() {
        let issues = corpus();

        let state = WorkingState::restore(
            &issues,
            stored(&[
                (settings_keys::UI_VIEW, "All"),
                (settings_keys::UI_SELECTED, "2"),
            ]),
        );
        assert_eq!(state.selected(), Some(2));

        // Stored under a View that no longer shows it.
        let state = WorkingState::restore(
            &issues,
            stored(&[
                (settings_keys::UI_VIEW, "Doing"),
                (settings_keys::UI_SELECTED, "3"),
            ]),
        );
        assert_eq!(state.selected(), Some(1));

        // Deleted since the window closed.
        let state = WorkingState::restore(&issues, stored(&[(settings_keys::UI_SELECTED, "99")]));
        assert_eq!(state.selected(), Some(1));
    }

    #[test]
    fn restoring_treats_unusable_values_as_absent() {
        let issues = corpus();
        let state = WorkingState::restore(
            &issues,
            stored(&[
                (settings_keys::UI_VIEW, "Nonsense"),
                (settings_keys::UI_SELECTED, "not a number"),
                (settings_keys::UI_TAG, "   "),
            ]),
        );

        assert_eq!(state.view(), View::default());
        assert_eq!(state.tag(), None);
        assert_eq!(state.selected(), Some(1), "fell back to the first row");
    }

    #[test]
    fn restoring_drops_a_tag_nothing_carries_any_more() {
        let issues = corpus();
        let state = WorkingState::restore(&issues, stored(&[(settings_keys::UI_TAG, "abandoned")]));
        assert_eq!(state.tag(), None);
        assert_eq!(state.visible(&issues).len(), 3);
    }

    #[test]
    fn what_is_written_is_what_comes_back() {
        let issues = corpus();
        let mut written = WorkingState::default();
        written.select_view(View::WithStatus(Status::Doing), &issues);
        written.toggle_tag("ui".parse().unwrap(), &issues);
        written.set_filter("flash".into(), &issues);
        written.select_issue(1);

        let saved = written.settings();
        let restored = WorkingState::restore(&issues, move |key| {
            saved
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.clone())
        });

        assert_eq!(restored, written);
    }

    #[test]
    fn an_empty_working_state_round_trips_too() {
        // Absent selection and Tag are written as empty strings, which must
        // read back as absent rather than as a Tag named "".
        let issues: Vec<Issue> = Vec::new();
        let state = WorkingState::default();
        let saved = state.settings();
        let restored = WorkingState::restore(&issues, move |key| {
            saved
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.clone())
        });
        assert_eq!(restored, state);
    }

    #[test]
    fn a_view_count_ignores_the_filters_narrowing_it() {
        let issues = corpus();
        let mut state = WorkingState::default();
        state.set_filter("nothing matches".into(), &issues);

        assert!(state.visible(&issues).is_empty());
        assert_eq!(WorkingState::count_for(View::All, &issues), 3);
        assert_eq!(
            WorkingState::count_for(View::WithStatus(Status::Done), &issues),
            1
        );
    }
}
