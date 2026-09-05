//! App-level ownership of the [`Store`] and the in-memory Issue list.
//!
//! This is the single writer. Both the UI and the HTTP API mutate through it,
//! which is what makes their capabilities identical by construction rather
//! than by discipline — there is no second path to the database.
//!
//! Deliberately free of `gpui`: the UI wraps this in an `Entity` to get
//! change notification, but nothing here knows that. That keeps every
//! interesting mutation testable without a window. See `docs/adr/0005`.

use std::collections::BTreeSet;

use anyhow::Result;

use crate::domain::{Issue, IssueId, Priority, Status, Tag, normalise_tags, sort_for_display};
use crate::store::Store;

/// A change to some of an Issue's fields.
///
/// Every field is optional and only those supplied are written. Two callers
/// touching different fields therefore cannot clobber one another — the API
/// setting a Status never rewrites a title the UI is in the middle of editing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IssuePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub status: Option<Status>,
    pub priority: Option<Priority>,
    /// Replaces the whole Tag set. Use [`Projection::add_tag`] to change one
    /// Tag without a read-modify-write.
    pub tags: Option<Vec<Tag>>,
}

impl IssuePatch {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn status(mut self, status: Status) -> Self {
        self.status = Some(status);
        self
    }

    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = Some(priority);
        self
    }

    pub fn tags(mut self, tags: Vec<Tag>) -> Self {
        self.tags = Some(tags);
        self
    }
}

/// Every Issue, display-sorted, plus the database they came from.
pub struct Projection {
    store: Store,
    issues: Vec<Issue>,
}

impl Projection {
    pub fn load(store: Store) -> Result<Self> {
        let mut issues = store.load_all()?;
        sort_for_display(&mut issues);
        Ok(Self { store, issues })
    }

    // ---- reads --------------------------------------------------------------

    /// Every Issue in display order. Readers render straight from this.
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }

    pub fn get(&self, id: IssueId) -> Option<&Issue> {
        self.issues.iter().find(|issue| issue.id == id)
    }

    /// Every Tag some Issue carries. Derived: there is no Tag registry.
    pub fn tags_in_use(&self) -> BTreeSet<Tag> {
        self.issues
            .iter()
            .flat_map(|issue| issue.tags.iter().cloned())
            .collect()
    }

    pub fn count_with_tag(&self, tag: &Tag) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.tags.contains(tag))
            .count()
    }

    // ---- writes -------------------------------------------------------------

    /// Creates an Issue, then applies any other supplied fields.
    ///
    /// The second step goes through [`Self::patch`] rather than repeating the
    /// field logic, so a Tag arriving at creation time is folded against the
    /// vocabulary exactly as it would be later.
    pub fn create(&mut self, title: &str, rest: IssuePatch) -> Result<Issue> {
        let issue = self.store.insert(title)?;
        let id = issue.id;
        self.issues.push(issue);
        sort_for_display(&mut self.issues);

        if !rest.is_empty() {
            self.patch(id, rest)?;
        }
        Ok(self.get(id).cloned().expect("just inserted"))
    }

    /// Applies a patch, returning the Issue as it now stands, or `None` when
    /// no such Issue exists.
    ///
    /// A patch that changes nothing writes nothing, so `updated_at` does not
    /// move for a no-op — a keystroke that restores the previous text should
    /// not reorder the list.
    pub fn patch(&mut self, id: IssueId, patch: IssuePatch) -> Result<Option<Issue>> {
        // Computed before the mutable borrow below, and deliberately from the
        // whole corpus: a Tag's established spelling is a global fact.
        let in_use = self.tags_in_use();

        let Some(issue) = self.issues.iter_mut().find(|issue| issue.id == id) else {
            return Ok(None);
        };
        let before = issue.clone();

        if let Some(title) = patch.title {
            issue.title = title;
        }
        if let Some(body) = patch.body {
            issue.body = body;
        }
        if let Some(status) = patch.status {
            issue.status = status;
        }
        if let Some(priority) = patch.priority {
            issue.priority = priority;
        }
        if let Some(tags) = patch.tags {
            issue.tags = normalise_tags(tags, &in_use);
        }

        if *issue == before {
            return Ok(Some(before));
        }

        let snapshot = issue.clone();
        let updated_at = self.store.update(&snapshot)?;
        if let Some(issue) = self.issues.iter_mut().find(|issue| issue.id == id) {
            issue.updated_at = updated_at;
        }
        sort_for_display(&mut self.issues);
        Ok(self.get(id).cloned())
    }

    /// Adds one Tag. Race-free where a read-modify-write from a caller is not:
    /// the read and the write happen together, under the single writer.
    pub fn add_tag(&mut self, id: IssueId, tag: Tag) -> Result<Option<Issue>> {
        let Some(issue) = self.get(id) else {
            return Ok(None);
        };
        let mut tags = issue.tags.clone();
        if !tags.contains(&tag) {
            tags.push(tag);
        }
        self.patch(id, IssuePatch::default().tags(tags))
    }

    pub fn remove_tag(&mut self, id: IssueId, tag: &Tag) -> Result<Option<Issue>> {
        let Some(issue) = self.get(id) else {
            return Ok(None);
        };
        let tags = issue
            .tags
            .iter()
            .filter(|candidate| *candidate != tag)
            .cloned()
            .collect();
        self.patch(id, IssuePatch::default().tags(tags))
    }

    /// Erases an Issue. Reports whether there was one to erase.
    pub fn delete(&mut self, id: IssueId) -> Result<bool> {
        if self.get(id).is_none() {
            return Ok(false);
        }
        self.store.delete(id)?;
        self.issues.retain(|issue| issue.id != id);
        Ok(true)
    }

    // ---- settings -----------------------------------------------------------

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.store.get_setting(key)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.store.set_setting(key, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projection() -> Projection {
        Projection::load(Store::open_in_memory().expect("in-memory store")).expect("projection")
    }

    fn tag(name: &str) -> Tag {
        name.parse().expect("valid tag")
    }

    #[test]
    fn a_new_projection_is_empty() {
        assert!(projection().issues().is_empty());
    }

    #[test]
    fn create_with_no_extra_fields_matches_the_stores_defaults() {
        let mut p = projection();
        let issue = p.create("plain", IssuePatch::default()).unwrap();

        assert_eq!(issue.title, "plain");
        assert_eq!(issue.status, Status::Todo);
        assert_eq!(issue.priority, Priority::None);
        assert!(issue.tags.is_empty());
    }

    #[test]
    fn create_applies_the_rest_of_the_fields_in_one_call() {
        let mut p = projection();
        let issue = p
            .create(
                "full",
                IssuePatch::default()
                    .body("described")
                    .status(Status::Doing)
                    .priority(Priority::Urgent)
                    .tags(vec![tag("Bug")]),
            )
            .unwrap();

        assert_eq!(issue.body, "described");
        assert_eq!(issue.status, Status::Doing);
        assert_eq!(issue.priority, Priority::Urgent);
        assert_eq!(issue.tags, vec![tag("Bug")]);
    }

    #[test]
    fn a_patch_touches_only_the_fields_it_names() {
        // The property the whole API design rests on: a caller setting Status
        // cannot clobber a title another caller is editing.
        let mut p = projection();
        let created = p.create("original", IssuePatch::default()).unwrap();

        p.patch(created.id, IssuePatch::default().status(Status::Done))
            .unwrap();

        let after = p.get(created.id).unwrap();
        assert_eq!(after.status, Status::Done);
        assert_eq!(
            after.title, "original",
            "title was not named, so not written"
        );
    }

    #[test]
    fn patching_an_unknown_issue_reports_rather_than_panics() {
        let mut p = projection();
        assert_eq!(
            p.patch(404, IssuePatch::default().title("ghost")).unwrap(),
            None
        );
    }

    #[test]
    fn a_patch_that_changes_nothing_does_not_move_updated_at() {
        let mut p = projection();
        let created = p.create("stable", IssuePatch::default()).unwrap();

        p.patch(created.id, IssuePatch::default().title("stable"))
            .unwrap();

        assert_eq!(p.get(created.id).unwrap().updated_at, created.updated_at);
    }

    #[test]
    fn a_real_patch_does_move_updated_at() {
        let mut p = projection();
        let created = p.create("moving", IssuePatch::default()).unwrap();

        p.patch(created.id, IssuePatch::default().title("moved"))
            .unwrap();

        assert!(p.get(created.id).unwrap().updated_at >= created.updated_at);
    }

    #[test]
    fn tags_arriving_by_patch_fold_against_the_established_spelling() {
        let mut p = projection();
        let first = p
            .create("first", IssuePatch::default().tags(vec![tag("Bug")]))
            .unwrap();
        let second = p.create("second", IssuePatch::default()).unwrap();

        // Lowercase on the way in; the corpus already says "Bug".
        p.patch(second.id, IssuePatch::default().tags(vec![tag("bug")]))
            .unwrap();

        assert_eq!(p.get(second.id).unwrap().tags[0].as_str(), "Bug");
        assert_eq!(p.tags_in_use().len(), 1);
        assert_eq!(p.count_with_tag(&tag("BUG")), 2);
        assert_eq!(first.tags[0].as_str(), "Bug");
    }

    #[test]
    fn adding_a_tag_leaves_the_others_alone() {
        let mut p = projection();
        let issue = p
            .create("tagged", IssuePatch::default().tags(vec![tag("ui")]))
            .unwrap();

        p.add_tag(issue.id, tag("Bug")).unwrap();

        assert_eq!(p.get(issue.id).unwrap().tags, vec![tag("Bug"), tag("ui")]);
    }

    #[test]
    fn adding_a_tag_twice_is_idempotent() {
        let mut p = projection();
        let issue = p.create("tagged", IssuePatch::default()).unwrap();

        p.add_tag(issue.id, tag("Bug")).unwrap();
        p.add_tag(issue.id, tag("bug")).unwrap();

        assert_eq!(p.get(issue.id).unwrap().tags, vec![tag("Bug")]);
    }

    #[test]
    fn removing_a_tag_is_case_insensitive_and_idempotent() {
        let mut p = projection();
        let issue = p
            .create("tagged", IssuePatch::default().tags(vec![tag("Bug")]))
            .unwrap();

        p.remove_tag(issue.id, &tag("BUG")).unwrap();
        p.remove_tag(issue.id, &tag("BUG")).unwrap();

        assert!(p.get(issue.id).unwrap().tags.is_empty());
        assert!(p.tags_in_use().is_empty(), "the Tag went with its last use");
    }

    #[test]
    fn deleting_reports_whether_there_was_anything_to_delete() {
        let mut p = projection();
        let issue = p.create("doomed", IssuePatch::default()).unwrap();

        assert!(p.delete(issue.id).unwrap());
        assert!(!p.delete(issue.id).unwrap(), "already gone");
        assert!(p.issues().is_empty());
    }

    #[test]
    fn issues_are_kept_in_display_order() {
        let mut p = projection();
        p.create("low", IssuePatch::default().priority(Priority::Low))
            .unwrap();
        let urgent = p
            .create("urgent", IssuePatch::default().priority(Priority::Urgent))
            .unwrap();

        assert_eq!(p.issues()[0].id, urgent.id);
    }

    #[test]
    fn writes_survive_a_reload_from_the_same_database() {
        // Guards the ADR-0002 failure mode: an in-memory change that never
        // reached SQLite looks identical until you restart.
        let mut p = projection();
        let issue = p
            .create(
                "persisted",
                IssuePatch::default()
                    .status(Status::Blocked)
                    .tags(vec![tag("ui")]),
            )
            .unwrap();

        let reloaded = p.store.load_all().unwrap();
        let found = reloaded.iter().find(|i| i.id == issue.id).unwrap();
        assert_eq!(found.status, Status::Blocked);
        assert_eq!(found.tags, vec![tag("ui")]);
    }
}
