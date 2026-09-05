//! The JSON shapes on the wire, and their translation to domain types.
//!
//! Enum values are spelled exactly as [`Status::label`] and
//! [`Priority::label`] produce them — the same text the database stores and
//! the UI displays. One spelling everywhere means no mapping table to keep in
//! step, and `FromStr` already rejects anything else with a usable message.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::{Issue, IssueId, ParseError, Priority, Status, Tag};
use crate::projection::IssuePatch;

#[derive(Debug, Serialize, Deserialize)]
pub struct IssueJson {
    pub id: IssueId,
    pub title: String,
    pub body: String,
    pub status: String,
    pub priority: String,
    pub tags: Vec<String>,
    /// The Issue this one is part of. Read-only here: attach and detach go
    /// through `PUT`/`DELETE /issues/{parent}/sub-issues/{child}`.
    pub parent_id: Option<IssueId>,
    /// Read-only, and in display order.
    pub sub_issue_ids: Vec<IssueId>,
    pub created_at: String,
    /// Present from the first release so `If-Match` can be added later without
    /// changing the shape clients already parse.
    pub updated_at: String,
}

impl IssueJson {
    /// Sub-issue ids come from the caller because an Issue does not know its
    /// own children — that is a fact about the whole corpus.
    pub fn new(issue: &Issue, sub_issue_ids: Vec<IssueId>) -> Self {
        Self {
            sub_issue_ids,
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
#[derive(Debug, Deserialize)]
pub struct NewIssue {
    pub title: String,
    /// Accepted at creation, where "absent" unambiguously means "no parent" —
    /// the ambiguity that keeps it out of `PATCH` does not arise here, and
    /// filing a sub-issue should not need two calls. See `docs/adr/0007`.
    #[serde(default)]
    pub parent_id: Option<IssueId>,
    #[serde(flatten)]
    pub rest: PatchIssue,
}

/// `PATCH /issues/{id}`. An absent field is left alone; this is what stops a
/// caller setting a Status from rewriting a title someone else is editing.
#[derive(Debug, Default, Deserialize)]
pub struct PatchIssue {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Present only so it can be *rejected* with a pointer to the right
    /// endpoint. Typed as a raw value because `Option<IssueId>` cannot tell an
    /// absent key from an explicit `null`, and silently ignoring an attempted
    /// re-parent would be a trap.
    #[serde(default)]
    pub parent_id: Option<serde_json::Value>,
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
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = IssueJson::new(&issue, vec![9]);
        assert_eq!(json.status, "Cancelled");
        assert_eq!(json.priority, "Urgent");
        assert_eq!(json.tags, vec!["Bug".to_string()]);
        assert_eq!(json.parent_id, None);
        assert_eq!(json.sub_issue_ids, vec![9]);
        // Round-trips back through the same parser the store uses.
        assert_eq!(json.status.parse::<Status>().unwrap(), Status::Cancelled);
    }
}
