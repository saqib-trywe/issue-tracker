// SPDX-License-Identifier: GPL-3.0-only

//! The JSON shapes on the wire, and their translation to domain types.
//!
//! Enum values are spelled exactly as [`Status::label`] and
//! [`Priority::label`] produce them — the same text the database stores and
//! the UI displays. One spelling everywhere means no mapping table to keep in
//! step, and `FromStr` already rejects anything else with a usable message.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use crate::domain::{Issue, IssueId, ParseError, Priority, Size, SizeRollup, Status, Tag};
use crate::projection::IssuePatch;

#[derive(Debug, Serialize, Deserialize)]
pub struct IssueJson {
    pub id: IssueId,
    pub title: String,
    pub body: String,
    pub status: String,
    pub priority: String,
    /// This Issue's own Size. For a Parent, the work its parts do not cover.
    pub size: Option<Size>,
    /// Own Size plus its parts', or `null` when nothing in the family carries
    /// one. Read-only and derived: a client cannot compute it, because it does
    /// not hold the parts' Sizes.
    pub total_size: Option<u32>,
    /// How many parts have no Size. The number of parts is
    /// `sub_issue_ids.len()`, so only this half needs sending.
    pub unsized_sub_issues: usize,
    pub tags: Vec<String>,
    /// The Issue this one is part of. Read-only here: attach and detach go
    /// through `PUT`/`DELETE /issues/{parent}/sub-issues/{child}`.
    pub parent_id: Option<IssueId>,
    /// Read-only, and in display order.
    pub sub_issue_ids: Vec<IssueId>,
    /// How many of those are settled — Done or Cancelled. The total is
    /// `sub_issue_ids.len()`, so only this half needs sending; two spellings
    /// of one number is a thing that can disagree with itself.
    pub settled_sub_issues: usize,
    pub created_at: String,
    /// Present from the first release so `If-Match` can be added later without
    /// changing the shape clients already parse.
    pub updated_at: String,
}

impl IssueJson {
    /// The children come from the caller because an Issue does not know its
    /// own — that is a fact about the whole corpus.
    ///
    /// They arrive as Issues rather than as ids and a count, so the two
    /// numbers that go out are derived from one slice and cannot contradict
    /// each other.
    pub fn new(issue: &Issue, sub_issues: &[&Issue]) -> Self {
        let rollup = SizeRollup::of(issue, sub_issues);
        Self {
            sub_issue_ids: sub_issues.iter().map(|child| child.id).collect(),
            settled_sub_issues: sub_issues
                .iter()
                .filter(|child| child.status.is_settled())
                .count(),
            size: issue.size,
            total_size: rollup.total,
            unsized_sub_issues: rollup.unsized_sub_issues,
            parent_id: issue.parent_id,
            id: issue.id,
            title: issue.title.clone(),
            body: issue.body.clone(),
            status: issue.status.label().to_string(),
            priority: issue.priority.label().to_string(),
            tags: issue
                .tags
                .iter()
                .map(|tag| tag.as_str().to_owned())
                .collect(),
            created_at: timestamp(issue.created_at),
            updated_at: timestamp(issue.updated_at),
        }
    }
}

fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Micros, true)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TagJson {
    pub name: String,
    pub count: usize,
}

/// `POST /issues`. Only the title is required; everything else is applied in
/// the same call so automation never needs two round-trips to file a
/// fully-specified Issue.
///
/// `Serialize` because this is also what the clients build: `operations`
/// constructs one and sends it, so the shape of a create body is defined once,
/// by the side that has to parse it.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct NewIssue {
    pub title: String,
    /// Accepted at creation, where "absent" unambiguously means "no parent" —
    /// the ambiguity that keeps it out of `PATCH` does not arise here, and
    /// filing a sub-issue should not need two calls. See `docs/adr/0007`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<IssueId>,
    #[serde(flatten)]
    pub rest: PatchIssue,
    /// Whatever the body said that nothing above claimed, so `create_issue`
    /// can refuse it.
    ///
    /// `PatchIssue` says `deny_unknown_fields` and that is enough for `PATCH`,
    /// but a flattened struct never sees the keys it did not match — serde
    /// hands them to the outer type — so the same `deny` is silently inert
    /// here. Catching them is the only way `POST` can refuse what `PATCH`
    /// refuses. Empty is the normal case, and an empty map serialises to
    /// nothing, so a body built by a client is unchanged by its presence.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

/// `PATCH /issues/{id}`. An absent field is left alone; this is what stops a
/// caller setting a Status from rewriting a title someone else is editing.
///
/// Every field skips serialising when absent. Without that, a patch built by
/// a client would send `"title": null` for a field it does not touch, and
/// "names only what it changes" would be true only by accident.
///
/// Unknown fields are refused rather than ignored, for the reason the CLI
/// calls `finish()` and the MCP server's arguments say the same thing: without
/// it, `PATCH {"statuss": "Done"}` answers `200 OK` and changes nothing, which
/// is the one failure a caller cannot see.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchIssue {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Three states, which is why this is nested and not `Option<Size>`:
    /// absent leaves the Size alone, an explicit `null` clears it, a number
    /// sets it. `0` cannot mean "unsized" — it is a real Size meaning no work.
    /// `Option<Size>` alone could not tell an absent key from a `null`, which
    /// is the same trap `parent_id` sits beside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(deserialize_with = "present_but_maybe_null")]
    pub size: Option<Option<Size>>,
    /// Present only so it can be *rejected* with a pointer to the right
    /// endpoint. Typed as a raw value because `Option<IssueId>` cannot tell an
    /// absent key from an explicit `null`, and silently ignoring an attempted
    /// re-parent would be a trap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<serde_json::Value>,
}

/// Distinguishes an absent key from an explicit `null`.
///
/// With `#[serde(default)]` an absent key never reaches this function and
/// yields `None` — leave it alone. A key that *is* present does reach it, so a
/// `null` becomes `Some(None)` — clear it — and a number becomes
/// `Some(Some(n))`. Plain `Option<Option<T>>` collapses the first two into
/// `None`, which is the whole problem.
///
/// Six lines rather than a dependency, for the same reason the HTTP client is
/// hand-rolled.
fn present_but_maybe_null<'de, D, T>(input: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(input).map(Some)
}

impl PatchIssue {
    /// Translates to a domain patch, rejecting unknown enum values and
    /// unusable Tag names rather than silently dropping them.
    pub fn into_patch(self) -> Result<IssuePatch, ParseError> {
        let tags = match self.tags {
            Some(names) => Some(
                names
                    .iter()
                    .map(|name| name.parse::<Tag>())
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            None => None,
        };

        Ok(IssuePatch {
            size: self.size,
            title: self.title,
            body: self.body,
            status: self
                .status
                .as_deref()
                .map(str::parse::<Status>)
                .transpose()?,
            priority: self
                .priority
                .as_deref()
                .map(str::parse::<Priority>)
                .transpose()?,
            tags,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_patch_changes_nothing() {
        let patch: PatchIssue = serde_json::from_str("{}").unwrap();
        assert!(patch.into_patch().unwrap().is_empty());
    }

    #[test]
    fn a_patch_carries_only_the_fields_it_names() {
        let patch: PatchIssue = serde_json::from_str(r#"{"status":"Done"}"#).unwrap();
        let patch = patch.into_patch().unwrap();
        assert_eq!(patch.status, Some(Status::Done));
        assert_eq!(patch.title, None);
        assert_eq!(patch.tags, None);
    }

    #[test]
    fn an_unknown_enum_value_is_an_error_not_a_default() {
        let patch: PatchIssue = serde_json::from_str(r#"{"status":"Wontfix"}"#).unwrap();
        let err = patch.into_patch().unwrap_err();
        assert_eq!(err.kind, "status");
        assert!(err.to_string().contains("Wontfix"));

        let patch: PatchIssue = serde_json::from_str(r#"{"priority":"Critical"}"#).unwrap();
        assert_eq!(patch.into_patch().unwrap_err().kind, "priority");
    }

    #[test]
    fn an_unusable_tag_name_is_an_error() {
        let patch: PatchIssue = serde_json::from_str(r#"{"tags":["ok","  "]}"#).unwrap();
        assert_eq!(patch.into_patch().unwrap_err().kind, "tag");
    }

    #[test]
    fn clearing_the_tags_is_distinct_from_leaving_them_alone() {
        let cleared: PatchIssue = serde_json::from_str(r#"{"tags":[]}"#).unwrap();
        assert_eq!(cleared.into_patch().unwrap().tags, Some(Vec::new()));

        let untouched: PatchIssue = serde_json::from_str("{}").unwrap();
        assert_eq!(untouched.into_patch().unwrap().tags, None);
    }

    #[test]
    fn a_create_body_takes_the_other_fields_inline() {
        let new: NewIssue =
            serde_json::from_str(r#"{"title":"t","status":"Doing","tags":["Bug"]}"#).unwrap();
        assert_eq!(new.title, "t");
        let patch = new.rest.into_patch().unwrap();
        assert_eq!(patch.status, Some(Status::Doing));
        assert_eq!(patch.tags.unwrap()[0].as_str(), "Bug");
    }

    #[test]
    fn enum_values_go_out_spelled_as_the_ui_spells_them() {
        let issue = Issue {
            id: 1,
            title: "t".into(),
            body: String::new(),
            status: Status::Cancelled,
            priority: Priority::Urgent,
            tags: vec!["Bug".parse().unwrap()],
            parent_id: None,
            size: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let child = Issue {
            id: 9,
            status: Status::Done,
            ..issue.clone()
        };
        let json = IssueJson::new(&issue, &[&child]);
        assert_eq!(json.status, "Cancelled");
        assert_eq!(json.priority, "Urgent");
        assert_eq!(json.tags, vec!["Bug".to_string()]);
        assert_eq!(json.parent_id, None);
        assert_eq!(json.sub_issue_ids, vec![9]);
        assert_eq!(json.settled_sub_issues, 1);
        // Round-trips back through the same parser the store uses.
        assert_eq!(json.status.parse::<Status>().unwrap(), Status::Cancelled);
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;

    fn patch(raw: &str) -> PatchIssue {
        serde_json::from_str(raw).expect("a patch")
    }

    #[test]
    fn a_patch_tells_absent_apart_from_null() {
        // The distinction the whole nested Option exists for. `0` cannot
        // stand in for "unsized": it is a real Size meaning no work.
        assert_eq!(patch("{}").size, None, "absent leaves it alone");
        assert_eq!(patch(r#"{"size":null}"#).size, Some(None), "null clears it");
        assert_eq!(
            patch(r#"{"size":7}"#).size,
            Some(Some(7)),
            "a number sets it"
        );
        assert_eq!(patch(r#"{"size":0}"#).size, Some(Some(0)), "zero is a size");
    }

    #[test]
    fn a_size_outside_a_u8_is_refused_rather_than_clamped() {
        // 255 is where an Issue should have become a tree of Issues.
        assert!(serde_json::from_str::<PatchIssue>(r#"{"size":256}"#).is_err());
        assert!(serde_json::from_str::<PatchIssue>(r#"{"size":-1}"#).is_err());
    }

    #[test]
    fn the_three_states_survive_a_round_trip() {
        for raw in ["{}", r#"{"size":null}"#, r#"{"size":7}"#] {
            let there_and_back = serde_json::to_string(&patch(raw)).expect("serialisable");
            assert_eq!(there_and_back, raw, "{raw}");
        }
    }
}
