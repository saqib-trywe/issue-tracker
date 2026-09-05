// SPDX-License-Identifier: GPL-3.0-only

//! Pure domain types.
//!
//! This module must not depend on `gpui` or `rusqlite`. Keeping it free of
//! both is what lets it move into its own crate later without a rewrite.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use chrono::{DateTime, Utc};

/// Issues are identified by a sequential integer, shown to the user as `#42`.
pub type IssueId = i64;

/// Returned when a `TEXT` column holds a value outside the known set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub kind: &'static str,
    pub value: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unrecognised {}: {:?}", self.kind, self.value)
    }
}

impl std::error::Error for ParseError {}

/// Where an Issue sits in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Status {
    #[default]
    Todo,
    Doing,
    Blocked,
    Done,
    Cancelled,
}

impl Status {
    pub const ALL: [Status; 5] = [
        Status::Todo,
        Status::Doing,
        Status::Blocked,
        Status::Done,
        Status::Cancelled,
    ];

    /// Whether this Status represents work that is no longer outstanding.
    ///
    /// Done and Cancelled both settle an Issue: one finished, the other was
    /// deliberately abandoned. Neither is waiting on anybody. This is the
    /// predicate a parent is measured against before it may be marked Done —
    /// blocking a parent on a Cancelled child would push you toward deleting
    /// records the glossary says are worth keeping.
    pub fn is_settled(self) -> bool {
        matches!(self, Status::Done | Status::Cancelled)
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Todo => "Todo",
            Status::Doing => "Doing",
            Status::Blocked => "Blocked",
            Status::Done => "Done",
            Status::Cancelled => "Cancelled",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Case-insensitive, so `done` and `Done` are the same Status.
///
/// Tag identity already folds case deliberately; Status matching exactly was
/// the inconsistency. The database only ever holds canonical labels, so this
/// widens what is accepted without changing what is stored.
impl FromStr for Status {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Status::ALL
            .into_iter()
            .find(|status| status.label().eq_ignore_ascii_case(s))
            .ok_or_else(|| ParseError {
                kind: "status",
                value: s.to_owned(),
            })
    }
}

/// How much an Issue matters relative to others.
///
/// Variants are declared lowest-first so the derived `Ord` sorts naturally;
/// the list view then sorts descending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Priority {
    #[default]
    None,
    Low,
    Medium,
    High,
    Urgent,
}

impl Priority {
    pub const ALL: [Priority; 5] = [
        Priority::None,
        Priority::Low,
        Priority::Medium,
        Priority::High,
        Priority::Urgent,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Priority::None => "None",
            Priority::Low => "Low",
            Priority::Medium => "Medium",
            Priority::High => "High",
            Priority::Urgent => "Urgent",
        }
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Case-insensitive, for the reason given on [`Status`]'s implementation.
impl FromStr for Priority {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Priority::ALL
            .into_iter()
            .find(|priority| priority.label().eq_ignore_ascii_case(s))
            .ok_or_else(|| ParseError {
                kind: "priority",
                value: s.to_owned(),
            })
    }
}

/// The longest a Tag may be. Long enough for a short phrase, short enough
/// that a chip never crowds out the title it sits under.
const MAX_TAG_LEN: usize = 50;

/// A user-invented name attached to an Issue.
///
/// Tags are derived rather than declared: one exists exactly as long as some
/// Issue carries it, and vanishes when the last one lets go. See
/// `docs/adr/0004`.
///
/// Identity is case-insensitive — `Bug` and `bug` are one Tag — so the
/// comparison traits are written over [`Tag::key`] rather than derived. That
/// makes it structurally impossible for a case variant to slip past a
/// `contains` check, which no database constraint could catch: without a `tag`
/// table there is nothing for a unique index to be unique *across*.
#[derive(Debug, Clone)]
pub struct Tag(String);

impl Tag {
    /// The name as the user first spelled it.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The comparison identity: case-folded.
    pub fn key(&self) -> String {
        self.0.to_lowercase()
    }

    /// Returns the spelling already established for this name, if there is
    /// one.
    ///
    /// Case-insensitive equality keeps *comparisons* honest, but the junction
    /// table stores whatever was typed. Folding on the way in is what stops
    /// `Bug` and `bug` showing up as two rows in the sidebar.
    pub fn canonicalise(self, in_use: &BTreeSet<Tag>) -> Tag {
        in_use.get(&self).cloned().unwrap_or(self)
    }
}

impl PartialEq for Tag {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for Tag {}

impl PartialOrd for Tag {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Tag {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

impl Hash for Tag {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Tag {
    type Err = ParseError;

    /// Trims, then rejects anything that would make a Tag unusable: an empty
    /// name, a control character (which is also what keeps names free of
    /// newlines), or a name too long to render.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty()
            || trimmed.chars().any(char::is_control)
            || trimmed.chars().count() > MAX_TAG_LEN
        {
            return Err(ParseError {
                kind: "tag",
                value: s.to_owned(),
            });
        }
        Ok(Tag(trimmed.to_owned()))
    }
}

/// Prepares Tags for storage: folded against the spellings already in use,
/// deduplicated, and sorted so display order never depends on entry order.
pub fn normalise_tags(
    candidates: impl IntoIterator<Item = Tag>,
    in_use: &BTreeSet<Tag>,
) -> Vec<Tag> {
    candidates
        .into_iter()
        .map(|tag| tag.canonicalise(in_use))
        // A `BTreeSet` keeps the first of any pair it considers equal and
        // iterates in order, so this dedupes and sorts in one pass.
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// A single unit of tracked work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub id: IssueId,
    pub title: String,
    pub body: String,
    pub status: Status,
    pub priority: Priority,
    /// Sorted, and free of case-variant duplicates. Maintained through
    /// [`normalise_tags`].
    pub tags: Vec<Tag>,
    /// The Issue this one is part of, if any.
    ///
    /// Hierarchy is exactly one level deep: an Issue with a parent has no
    /// sub-issues of its own. Nothing in this type enforces that — it is a
    /// condition across rows, so it lives in `Projection`. See
    /// `docs/adr/0007`.
    pub parent_id: Option<IssueId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Issue {
    /// The title as displayed when the user hasn't typed one yet.
    pub fn display_title(&self) -> &str {
        if self.title.trim().is_empty() {
            "Untitled"
        } else {
            &self.title
        }
    }
}

/// A named, predefined slice of all Issues, shown in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum View {
    #[default]
    All,
    WithStatus(Status),
}

impl View {
    pub const ALL: [View; 6] = [
        View::All,
        View::WithStatus(Status::Todo),
        View::WithStatus(Status::Doing),
        View::WithStatus(Status::Blocked),
        View::WithStatus(Status::Done),
        View::WithStatus(Status::Cancelled),
    ];

    pub fn label(self) -> &'static str {
        match self {
            View::All => "All Issues",
            View::WithStatus(status) => status.label(),
        }
    }

    pub fn contains(self, issue: &Issue) -> bool {
        match self {
            View::All => true,
            View::WithStatus(status) => issue.status == status,
        }
    }
}

/// The persisted form of a View.
///
/// Deliberately distinct from [`View::label`], which is human-facing text —
/// `All` persists as `"All"` but displays as `"All Issues"`. Keeping them
/// separate means the display text can be reworded without invalidating
/// anything already stored.
impl fmt::Display for View {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            View::All => f.write_str("All"),
            View::WithStatus(status) => f.write_str(status.label()),
        }
    }
}

impl FromStr for View {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "All" {
            return Ok(View::All);
        }
        s.parse::<Status>()
            .map(View::WithStatus)
            .map_err(|_| ParseError {
                kind: "view",
                value: s.to_owned(),
            })
    }
}

/// Orders Issues for display: highest priority first, then most recently
/// updated. Ties break on `id` so the order is never ambiguous.
pub fn sort_for_display(issues: &mut [Issue]) {
    issues.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then(b.updated_at.cmp(&a.updated_at))
            .then(b.id.cmp(&a.id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(id: IssueId, priority: Priority, updated_at: &str) -> Issue {
        let at = DateTime::parse_from_rfc3339(updated_at)
            .unwrap()
            .with_timezone(&Utc);
        Issue {
            id,
            title: format!("issue {id}"),
            body: String::new(),
            status: Status::Todo,
            priority,
            tags: Vec::new(),
            parent_id: None,
            created_at: at,
            updated_at: at,
        }
    }

    #[test]
    fn status_round_trips_through_text() {
        for status in Status::ALL {
            assert_eq!(status.to_string().parse::<Status>().unwrap(), status);
        }
    }

    #[test]
    fn priority_round_trips_through_text() {
        for priority in Priority::ALL {
            assert_eq!(priority.to_string().parse::<Priority>().unwrap(), priority);
        }
    }

    #[test]
    fn status_and_priority_fold_case() {
        // What a terminal user types, and what the API therefore accepts.
        assert_eq!("done".parse::<Status>().unwrap(), Status::Done);
        assert_eq!("CANCELLED".parse::<Status>().unwrap(), Status::Cancelled);
        assert_eq!("urgent".parse::<Priority>().unwrap(), Priority::Urgent);
        // Folding must not make two variants collide.
        for status in Status::ALL {
            assert_eq!(
                status.label().to_lowercase().parse::<Status>().unwrap(),
                status
            );
        }
        for priority in Priority::ALL {
            assert_eq!(
                priority.label().to_lowercase().parse::<Priority>().unwrap(),
                priority
            );
        }
    }

    #[test]
    fn unknown_text_is_rejected_rather_than_defaulted() {
        assert!("Wontfix".parse::<Status>().is_err());
        assert!("Critical".parse::<Priority>().is_err());
    }

    #[test]
    fn done_and_cancelled_are_settled_the_rest_are_not() {
        assert!(Status::Done.is_settled());
        assert!(Status::Cancelled.is_settled(), "abandoned is still decided");
        assert!(!Status::Todo.is_settled());
        assert!(!Status::Doing.is_settled());
        assert!(!Status::Blocked.is_settled());
    }

    #[test]
    fn priority_orders_none_lowest_and_urgent_highest() {
        assert!(Priority::None < Priority::Low);
        assert!(Priority::Urgent > Priority::High);
    }

    #[test]
    fn all_view_contains_every_status() {
        for status in Status::ALL {
            let mut candidate = issue(1, Priority::None, "2026-01-01T00:00:00Z");
            candidate.status = status;
            assert!(View::All.contains(&candidate));
        }
    }

    #[test]
    fn status_view_contains_only_its_own_status() {
        let view = View::WithStatus(Status::Blocked);
        let mut blocked = issue(1, Priority::None, "2026-01-01T00:00:00Z");
        blocked.status = Status::Blocked;
        let todo = issue(2, Priority::None, "2026-01-01T00:00:00Z");

        assert!(view.contains(&blocked));
        assert!(!view.contains(&todo));
    }

    #[test]
    fn view_round_trips_through_text() {
        for view in View::ALL {
            assert_eq!(view.to_string().parse::<View>().unwrap(), view);
        }
    }

    #[test]
    fn view_persistence_is_independent_of_display_text() {
        // "All Issues" is what the user sees; "All" is what gets stored.
        // Rewording the label must not invalidate stored values.
        assert_eq!(View::All.to_string(), "All");
        assert_eq!(View::All.label(), "All Issues");
    }

    #[test]
    fn unknown_view_is_rejected() {
        assert!("Archived".parse::<View>().is_err());
        assert!("All Issues".parse::<View>().is_err());
    }

    #[test]
    fn display_orders_by_priority_then_recency() {
        let mut issues = vec![
            issue(1, Priority::Low, "2026-01-03T00:00:00Z"),
            issue(2, Priority::Urgent, "2026-01-01T00:00:00Z"),
            issue(3, Priority::Low, "2026-01-05T00:00:00Z"),
        ];

        sort_for_display(&mut issues);

        // Urgent first despite being the oldest, then Low by recency.
        assert_eq!(
            issues.iter().map(|i| i.id).collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
    }

    fn tag(name: &str) -> Tag {
        name.parse().expect("valid tag")
    }

    #[test]
    fn tags_trim_and_preserve_their_spelling() {
        assert_eq!(tag("  Needs design  ").as_str(), "Needs design");
    }

    #[test]
    fn unusable_tag_names_are_rejected() {
        assert!("".parse::<Tag>().is_err());
        assert!("   ".parse::<Tag>().is_err());
        // Control characters are barred, which is also what guarantees a name
        // can never contain a newline.
        assert!("two\nlines".parse::<Tag>().is_err());
        assert!("x".repeat(MAX_TAG_LEN + 1).parse::<Tag>().is_err());
        assert!("x".repeat(MAX_TAG_LEN).parse::<Tag>().is_ok());
    }

    #[test]
    fn slashes_and_spaces_are_ordinary_characters() {
        // Deliberately flat: `ui/theme` is one name, not a child of `ui`.
        assert_eq!(tag("ui/theme").as_str(), "ui/theme");
        assert_ne!(tag("ui/theme"), tag("ui"));
    }

    #[test]
    fn tags_compare_without_regard_to_case() {
        assert_eq!(tag("Bug"), tag("bug"));
        assert_eq!(tag("Bug").cmp(&tag("bug")), Ordering::Equal);

        // The property that motivates the hand-written impls: a case variant
        // must never slip past a membership test.
        assert!([tag("Bug")].contains(&tag("bug")));
    }

    #[test]
    fn a_set_keeps_the_first_spelling_it_saw() {
        let mut in_use = BTreeSet::new();
        in_use.insert(tag("Bug"));
        in_use.insert(tag("bug"));

        assert_eq!(in_use.len(), 1);
        assert_eq!(in_use.iter().next().unwrap().as_str(), "Bug");
    }

    #[test]
    fn canonicalising_adopts_the_established_spelling() {
        let in_use = BTreeSet::from([tag("Bug")]);

        assert_eq!(tag("bug").canonicalise(&in_use).as_str(), "Bug");
        // An unrelated name keeps the spelling it arrived with.
        assert_eq!(tag("UI").canonicalise(&in_use).as_str(), "UI");
    }

    #[test]
    fn normalising_folds_dedupes_and_sorts() {
        let in_use = BTreeSet::from([tag("Bug")]);
        let tags = normalise_tags([tag("ui"), tag("bug"), tag("BUG")], &in_use);

        assert_eq!(
            tags.iter().map(Tag::as_str).collect::<Vec<_>>(),
            vec!["Bug", "ui"]
        );
    }

    #[test]
    fn blank_titles_display_as_untitled() {
        let mut candidate = issue(1, Priority::None, "2026-01-01T00:00:00Z");
        candidate.title = "   ".into();
        assert_eq!(candidate.display_title(), "Untitled");
    }
}
