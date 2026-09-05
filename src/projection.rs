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
use std::fmt;

use anyhow::Result;

use crate::domain::{Issue, IssueId, Priority, Status, Tag, normalise_tags, sort_for_display};
use crate::store::Store;

/// A write the data will not allow.
///
/// Distinct from a storage failure: nothing is broken and retrying will not
/// help — the request conflicts with the current shape of things. The API
/// turns these into `409 Conflict`, and the UI prevents most of them from
/// being expressible in the first place. See `docs/adr/0007`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Hierarchy is one level deep, and the proposed parent is itself a
    /// sub-issue.
    ParentIsSubIssue(IssueId),
    /// Hierarchy is one level deep, and the Issue being attached has
    /// sub-issues of its own.
    ChildHasSubIssues(IssueId),
    SelfParent,
    /// A Done parent may not take on work that is still outstanding.
    ParentAlreadyDone(IssueId),
    /// A parent is Done only once nothing beneath it is outstanding.
    SubIssuesOutstanding(usize),
    /// Reopening this would leave a Done parent sitting above open work.
    ParentIsDone(IssueId),
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::ParentIsSubIssue(id) => write!(
                f,
                "#{id} is itself a sub-issue, and sub-issues cannot have sub-issues"
            ),
            Refused::ChildHasSubIssues(id) => write!(
                f,
                "#{id} has sub-issues of its own, and sub-issues cannot have sub-issues"
            ),
            Refused::SelfParent => f.write_str("an issue cannot be its own parent"),
            Refused::ParentAlreadyDone(id) => {
                write!(f, "#{id} is Done and cannot take on outstanding work")
            }
            Refused::SubIssuesOutstanding(count) => {
                write!(f, "{count} sub-issue(s) are still outstanding",)
            }
            Refused::ParentIsDone(id) => {
                write!(f, "its parent #{id} is Done; reopen that first")
            }
        }
    }
}

/// Why a mutation did not happen.
#[derive(Debug)]
pub enum WriteError {
    /// No such Issue. The API answers 404.
    NotFound(IssueId),
    /// The caller asked for something the data forbids. The API answers 409.
    Refused(Refused),
    /// The database failed. The API answers 500.
    Store(anyhow::Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::NotFound(id) => write!(f, "no issue #{id}"),
            WriteError::Refused(refusal) => refusal.fmt(f),
            WriteError::Store(err) => write!(f, "{err:#}"),
        }
    }
}

/// Lets the existing `?` on store calls keep working unchanged.
impl From<anyhow::Error> for WriteError {
    fn from(err: anyhow::Error) -> Self {
        WriteError::Store(err)
    }
}

impl From<Refused> for WriteError {
    fn from(refusal: Refused) -> Self {
        WriteError::Refused(refusal)
    }
}

/// The result of any mutation.
pub type Written<T> = std::result::Result<T, WriteError>;

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

    // ---- hierarchy ----------------------------------------------------------

    /// The Issues that are part of `id`, in display order.
    pub fn sub_issues(&self, id: IssueId) -> Vec<&Issue> {
        self.issues
            .iter()
            .filter(|issue| issue.parent_id == Some(id))
            .collect()
    }

    /// How many sub-issues are settled, and how many there are — the `2/3` on
    /// a parent's row. `None` when it has no sub-issues at all.
    ///
    /// Deliberately the same predicate the Done rule uses, so the number on
    /// screen explains the disabled button exactly.
    pub fn settled_progress(&self, id: IssueId) -> Option<(usize, usize)> {
        let children = self.sub_issues(id);
        if children.is_empty() {
            return None;
        }
        let settled = children
            .iter()
            .filter(|issue| issue.status.is_settled())
            .count();
        Some((settled, children.len()))
    }

    fn outstanding_count(&self, id: IssueId) -> usize {
        self.sub_issues(id)
            .iter()
            .filter(|issue| !issue.status.is_settled())
            .count()
    }

    /// Whether `id` may be marked Done right now.
    pub fn can_be_done(&self, id: IssueId) -> bool {
        self.outstanding_count(id) == 0
    }

    /// Issues that could become sub-issues of `parent`.
    ///
    /// Includes Issues that already belong to somebody else — attaching one
    /// moves it — so callers must show the current parent rather than let a
    /// pick quietly empty another Issue.
    pub fn eligible_sub_issues(&self, parent: IssueId) -> Vec<&Issue> {
        let parent_is_done = self
            .get(parent)
            .is_some_and(|issue| issue.status == Status::Done);
        self.issues
            .iter()
            .filter(|issue| issue.id != parent)
            .filter(|issue| issue.parent_id != Some(parent))
            // One level deep: something with its own parts cannot become one.
            .filter(|issue| self.sub_issues(issue.id).is_empty())
            // A Done parent cannot take on outstanding work.
            .filter(|issue| !parent_is_done || issue.status.is_settled())
            .collect()
    }

    /// Issues that `child` could be made part of.
    pub fn eligible_parents(&self, child: IssueId) -> Vec<&Issue> {
        // Something with its own parts cannot be filed under anything.
        if !self.sub_issues(child).is_empty() {
            return Vec::new();
        }
        let child_settled = self
            .get(child)
            .is_some_and(|issue| issue.status.is_settled());
        self.issues
            .iter()
            .filter(|issue| issue.id != child)
            .filter(|issue| issue.parent_id.is_none())
            .filter(|issue| child_settled || issue.status != Status::Done)
            .collect()
    }

    // ---- writes -------------------------------------------------------------

    /// Creates an Issue, then applies any other supplied fields.
    ///
    /// The second step goes through [`Self::patch`] rather than repeating the
    /// field logic, so a Tag arriving at creation time is folded against the
    /// vocabulary exactly as it would be later.
    pub fn create(&mut self, title: &str, rest: IssuePatch) -> Written<Issue> {
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
    pub fn patch(&mut self, id: IssueId, patch: IssuePatch) -> Written<Issue> {
        // Computed before the mutable borrow below, and deliberately from the
        // whole corpus: a Tag's established spelling is a global fact.
        let in_use = self.tags_in_use();
        if self.get(id).is_none() {
            return Err(WriteError::NotFound(id));
        }
        if let Some(status) = patch.status {
            self.check_status_change(id, status)?;
        }

        let Some(issue) = self.issues.iter_mut().find(|issue| issue.id == id) else {
            return Err(WriteError::NotFound(id));
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
            return Ok(before);
        }

        let snapshot = issue.clone();
        let updated_at = self.store.update(&snapshot)?;
        if let Some(issue) = self.issues.iter_mut().find(|issue| issue.id == id) {
            issue.updated_at = updated_at;
        }
        sort_for_display(&mut self.issues);
        Ok(self.get(id).cloned().expect("just patched"))
    }

    /// Guards the two directions of the completion rule.
    ///
    /// A parent may only be marked Done once nothing beneath it is
    /// outstanding; and a sub-issue may not be reopened while its parent is
    /// Done, which would produce that same forbidden state from below. Making
    /// both unreachable is what keeps this an invariant rather than a nudge.
    fn check_status_change(&self, id: IssueId, next: Status) -> std::result::Result<(), Refused> {
        if next == Status::Done && !self.can_be_done(id) {
            return Err(Refused::SubIssuesOutstanding(self.outstanding_count(id)));
        }

        if !next.is_settled()
            && let Some(parent) = self.get(id).and_then(|issue| issue.parent_id)
            && self.get(parent).is_some_and(|p| p.status == Status::Done)
        {
            return Err(Refused::ParentIsDone(parent));
        }

        Ok(())
    }

    /// Files `child` under `parent`, or removes it from whatever holds it.
    ///
    /// Attaching an Issue that already belongs to another parent *moves* it —
    /// there is no separate operation for that, because `set_parent` already
    /// says everything a move needs to say.
    pub fn set_parent(&mut self, child: IssueId, parent: Option<IssueId>) -> Written<Issue> {
        if self.get(child).is_none() {
            return Err(WriteError::NotFound(child));
        }

        if let Some(parent) = parent {
            if parent == child {
                return Err(Refused::SelfParent.into());
            }
            let Some(target) = self.get(parent) else {
                return Err(WriteError::NotFound(parent));
            };
            // One level deep, checked from both ends.
            if target.parent_id.is_some() {
                return Err(Refused::ParentIsSubIssue(parent).into());
            }
            if !self.sub_issues(child).is_empty() {
                return Err(Refused::ChildHasSubIssues(child).into());
            }
            let target_done = target.status == Status::Done;
            let child_settled = self
                .get(child)
                .is_some_and(|issue| issue.status.is_settled());
            if target_done && !child_settled {
                return Err(Refused::ParentAlreadyDone(parent).into());
            }
        }

        let issue = self
            .issues
            .iter_mut()
            .find(|issue| issue.id == child)
            .expect("checked above");
        if issue.parent_id == parent {
            return Ok(issue.clone());
        }
        issue.parent_id = parent;

        let snapshot = issue.clone();
        let updated_at = self.store.update(&snapshot)?;
        if let Some(issue) = self.issues.iter_mut().find(|issue| issue.id == child) {
            issue.updated_at = updated_at;
        }
        sort_for_display(&mut self.issues);
        Ok(self.get(child).cloned().expect("just moved"))
    }

    /// Adds one Tag. Race-free where a read-modify-write from a caller is not:
    /// the read and the write happen together, under the single writer.
    pub fn add_tag(&mut self, id: IssueId, tag: Tag) -> Written<Issue> {
        let Some(issue) = self.get(id) else {
            return Err(WriteError::NotFound(id));
        };
        let mut tags = issue.tags.clone();
        if !tags.contains(&tag) {
            tags.push(tag);
        }
        self.patch(id, IssuePatch::default().tags(tags))
    }

    pub fn remove_tag(&mut self, id: IssueId, tag: &Tag) -> Written<Issue> {
        let Some(issue) = self.get(id) else {
            return Err(WriteError::NotFound(id));
        };
        let tags = issue
            .tags
            .iter()
            .filter(|candidate| *candidate != tag)
            .cloned()
            .collect();
        self.patch(id, IssuePatch::default().tags(tags))
    }

    /// Erases an Issue.
    ///
    /// Its sub-issues survive as ordinary top-level Issues: the foreign key is
    /// `ON DELETE SET NULL`, so orphaning is the schema's doing rather than
    /// something this code has to remember. Deleting a parent is not a licence
    /// to erase work nobody asked to erase.
    pub fn delete(&mut self, id: IssueId) -> Written<()> {
        if self.get(id).is_none() {
            return Err(WriteError::NotFound(id));
        }
        self.store.delete(id)?;
        self.issues.retain(|issue| issue.id != id);
        for issue in self.issues.iter_mut() {
            if issue.parent_id == Some(id) {
                issue.parent_id = None;
            }
        }
        Ok(())
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
        let err = p
            .patch(404, IssuePatch::default().title("ghost"))
            .unwrap_err();
        assert!(matches!(err, WriteError::NotFound(404)));
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

        p.delete(issue.id).unwrap();
        assert!(
            matches!(p.delete(issue.id).unwrap_err(), WriteError::NotFound(_)),
            "already gone"
        );
        assert!(p.issues().is_empty());
    }

    // ---- sub-issues ---------------------------------------------------------

    fn refusal(err: WriteError) -> Refused {
        match err {
            WriteError::Refused(refusal) => refusal,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    /// A parent with `count` sub-issues, all Todo.
    fn family(p: &mut Projection, count: usize) -> (IssueId, Vec<IssueId>) {
        let parent = p.create("parent", IssuePatch::default()).unwrap().id;
        let children = (0..count)
            .map(|n| {
                let child = p
                    .create(&format!("child {n}"), IssuePatch::default())
                    .unwrap();
                p.set_parent(child.id, Some(parent)).unwrap();
                child.id
            })
            .collect();
        (parent, children)
    }

    #[test]
    fn attaching_and_detaching_a_sub_issue() {
        let mut p = projection();
        let (parent, children) = family(&mut p, 2);

        assert_eq!(p.sub_issues(parent).len(), 2);
        assert_eq!(p.get(children[0]).unwrap().parent_id, Some(parent));

        p.set_parent(children[0], None).unwrap();
        assert_eq!(p.sub_issues(parent).len(), 1);
        assert_eq!(p.get(children[0]).unwrap().parent_id, None);
    }

    #[test]
    fn attaching_an_issue_that_already_has_a_parent_moves_it() {
        // There is no separate move operation: setting the parent is one.
        let mut p = projection();
        let (first, children) = family(&mut p, 1);
        let second = p.create("second parent", IssuePatch::default()).unwrap().id;

        p.set_parent(children[0], Some(second)).unwrap();

        assert!(p.sub_issues(first).is_empty(), "moved out of the first");
        assert_eq!(p.sub_issues(second).len(), 1);
    }

    #[test]
    fn hierarchy_is_one_level_deep_from_both_ends() {
        let mut p = projection();
        let (parent, children) = family(&mut p, 1);
        let outsider = p.create("outsider", IssuePatch::default()).unwrap().id;

        // A sub-issue cannot become a parent.
        assert_eq!(
            refusal(p.set_parent(outsider, Some(children[0])).unwrap_err()),
            Refused::ParentIsSubIssue(children[0])
        );
        // Something that already has parts cannot become a part.
        assert_eq!(
            refusal(p.set_parent(parent, Some(outsider)).unwrap_err()),
            Refused::ChildHasSubIssues(parent)
        );
    }

    #[test]
    fn an_issue_cannot_be_its_own_parent() {
        let mut p = projection();
        let issue = p.create("lonely", IssuePatch::default()).unwrap().id;
        assert_eq!(
            refusal(p.set_parent(issue, Some(issue)).unwrap_err()),
            Refused::SelfParent
        );
    }

    #[test]
    fn a_parent_is_not_done_until_nothing_under_it_is_outstanding() {
        let mut p = projection();
        let (parent, children) = family(&mut p, 2);

        assert!(!p.can_be_done(parent));
        assert_eq!(
            refusal(
                p.patch(parent, IssuePatch::default().status(Status::Done))
                    .unwrap_err()
            ),
            Refused::SubIssuesOutstanding(2)
        );

        p.patch(children[0], IssuePatch::default().status(Status::Done))
            .unwrap();
        assert!(!p.can_be_done(parent), "one still open");

        p.patch(children[1], IssuePatch::default().status(Status::Done))
            .unwrap();
        assert!(p.can_be_done(parent));
        p.patch(parent, IssuePatch::default().status(Status::Done))
            .unwrap();
        assert_eq!(p.get(parent).unwrap().status, Status::Done);
    }

    #[test]
    fn a_cancelled_sub_issue_counts_as_settled() {
        // Blocking on it would push you toward deleting a record the glossary
        // says is worth keeping.
        let mut p = projection();
        let (parent, children) = family(&mut p, 1);

        p.patch(children[0], IssuePatch::default().status(Status::Cancelled))
            .unwrap();

        assert!(p.can_be_done(parent));
        assert!(
            p.patch(parent, IssuePatch::default().status(Status::Done))
                .is_ok()
        );
    }

    #[test]
    fn a_parent_may_be_cancelled_with_work_still_open_beneath_it() {
        let mut p = projection();
        let (parent, _) = family(&mut p, 2);
        assert!(
            p.patch(parent, IssuePatch::default().status(Status::Cancelled))
                .is_ok(),
            "cancelling propagates to nothing and is never blocked"
        );
    }

    #[test]
    fn a_sub_issue_cannot_be_reopened_under_a_done_parent() {
        // The invariant has to be unreachable from below too, or it is only
        // true at the instant you press the button.
        let mut p = projection();
        let (parent, children) = family(&mut p, 1);
        p.patch(children[0], IssuePatch::default().status(Status::Done))
            .unwrap();
        p.patch(parent, IssuePatch::default().status(Status::Done))
            .unwrap();

        assert_eq!(
            refusal(
                p.patch(children[0], IssuePatch::default().status(Status::Todo))
                    .unwrap_err()
            ),
            Refused::ParentIsDone(parent)
        );
        // Cancelling it is still settled, so still allowed.
        assert!(
            p.patch(children[0], IssuePatch::default().status(Status::Cancelled))
                .is_ok()
        );
    }

    #[test]
    fn a_done_parent_will_not_take_on_outstanding_work() {
        let mut p = projection();
        let parent = p
            .create("done", IssuePatch::default().status(Status::Done))
            .unwrap()
            .id;
        let open = p.create("open", IssuePatch::default()).unwrap().id;
        let closed = p
            .create("closed", IssuePatch::default().status(Status::Done))
            .unwrap()
            .id;

        assert_eq!(
            refusal(p.set_parent(open, Some(parent)).unwrap_err()),
            Refused::ParentAlreadyDone(parent)
        );
        assert!(
            p.set_parent(closed, Some(parent)).is_ok(),
            "already settled"
        );
    }

    #[test]
    fn detaching_is_always_allowed_even_from_a_done_parent() {
        let mut p = projection();
        let (parent, children) = family(&mut p, 1);
        p.patch(children[0], IssuePatch::default().status(Status::Done))
            .unwrap();
        p.patch(parent, IssuePatch::default().status(Status::Done))
            .unwrap();

        assert!(p.set_parent(children[0], None).is_ok());
    }

    #[test]
    fn deleting_a_parent_orphans_its_children_rather_than_erasing_them() {
        let mut p = projection();
        let (parent, children) = family(&mut p, 2);

        p.delete(parent).unwrap();

        assert_eq!(p.issues().len(), 2, "the work survives");
        for child in children {
            assert_eq!(p.get(child).unwrap().parent_id, None);
        }
    }

    #[test]
    fn orphaning_survives_a_reload() {
        // The foreign key does this, not us — guard that the schema agrees.
        let mut p = projection();
        let (parent, children) = family(&mut p, 1);
        p.delete(parent).unwrap();

        let reloaded = p.store.load_all().unwrap();
        let child = reloaded.iter().find(|i| i.id == children[0]).unwrap();
        assert_eq!(child.parent_id, None);
    }

    #[test]
    fn settled_progress_matches_the_completion_rule() {
        let mut p = projection();
        let (parent, children) = family(&mut p, 3);
        assert_eq!(p.settled_progress(parent), Some((0, 3)));

        p.patch(children[0], IssuePatch::default().status(Status::Done))
            .unwrap();
        p.patch(children[1], IssuePatch::default().status(Status::Cancelled))
            .unwrap();
        assert_eq!(p.settled_progress(parent), Some((2, 3)));
        assert!(!p.can_be_done(parent));

        assert_eq!(
            p.settled_progress(children[0]),
            None,
            "no children of its own"
        );
    }

    #[test]
    fn candidate_lists_exclude_what_would_be_refused() {
        let mut p = projection();
        let (parent, children) = family(&mut p, 1);
        let loose = p.create("loose", IssuePatch::default()).unwrap().id;

        let candidates: Vec<IssueId> = p
            .eligible_sub_issues(parent)
            .iter()
            .map(|issue| issue.id)
            .collect();
        assert!(candidates.contains(&loose));
        assert!(!candidates.contains(&parent), "not itself");
        assert!(!candidates.contains(&children[0]), "already there");

        // A parent cannot itself be filed under anything.
        assert!(p.eligible_parents(parent).is_empty());
        let parents: Vec<IssueId> = p
            .eligible_parents(loose)
            .iter()
            .map(|issue| issue.id)
            .collect();
        assert!(parents.contains(&parent));
        assert!(!parents.contains(&children[0]), "already a sub-issue");
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
