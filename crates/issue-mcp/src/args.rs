// SPDX-License-Identifier: GPL-3.0-only

//! The arguments each tool takes, and the schemas an agent reads to learn them.
//!
//! Arguments are an MCP concern with no counterpart in the HTTP API, so they
//! are defined here rather than in the library. What is *not* redefined here
//! is the domain: [`StatusArg`] and [`PriorityArg`] build their schemas from
//! `Status::ALL` and `Priority::ALL`, so the values an agent is offered cannot
//! drift from the values the tracker accepts. A hardcoded list had already
//! drifted once, in the CLI's help text.

use std::str::FromStr;

use issue_tracker::domain::{IssueId, Priority, Status};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Deserialize;
use serde::de::{Error as _, Unexpected};

fn list(labels: &[&str]) -> String {
    labels.join(", ")
}

/// Builds a string schema whose `enum` is the domain's own list of labels.
fn labelled(labels: &[&str], description: &str) -> Schema {
    let json = serde_json::json!({
        "type": "string",
        "enum": labels,
        "description": description,
    });
    Schema::from(
        json.as_object()
            .expect("a JSON object was just built")
            .clone(),
    )
}

/// A Status, parsed the way every other entry point parses one.
#[derive(Debug, Clone, Copy)]
pub struct StatusArg(pub Status);

impl<'de> Deserialize<'de> for StatusArg {
    fn deserialize<D: serde::Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(input)?;
        Status::from_str(&raw).map(StatusArg).map_err(|_| {
            let expected: &str = &format!("one of {}", list(&Status::ALL.map(Status::label)));
            D::Error::invalid_value(Unexpected::Str(&raw), &expected)
        })
    }
}

impl JsonSchema for StatusArg {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Status".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        let labels: Vec<&str> = Status::ALL.iter().map(|status| status.label()).collect();
        labelled(&labels, "Where an issue sits in its lifecycle.")
    }
}

/// A Priority, likewise.
#[derive(Debug, Clone, Copy)]
pub struct PriorityArg(pub Priority);

impl<'de> Deserialize<'de> for PriorityArg {
    fn deserialize<D: serde::Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(input)?;
        Priority::from_str(&raw).map(PriorityArg).map_err(|_| {
            let expected: &str = &format!("one of {}", list(&Priority::ALL.map(Priority::label)));
            D::Error::invalid_value(Unexpected::Str(&raw), &expected)
        })
    }
}

impl JsonSchema for PriorityArg {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Priority".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        let labels: Vec<&str> = Priority::ALL
            .iter()
            .map(|priority| priority.label())
            .collect();
        labelled(&labels, "How much an issue matters relative to others.")
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListIssues {
    /// Only issues with this status.
    pub status: Option<StatusArg>,
    /// Only issues carrying this tag. Tag names ignore case.
    pub tag: Option<String>,
    /// Either "none", for issues that are not sub-issues of anything, or the
    /// id of an issue, for that issue's sub-issues.
    pub parent: Option<String>,
    /// Only issues whose title contains this text. Titles only — bodies and
    /// tags are not searched.
    pub search: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetIssue {
    /// The id of the issue to read.
    pub id: IssueId,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateIssue {
    /// A one-line summary of the work. Required.
    pub title: String,
    /// Free text. Notes, reproduction steps, whatever is worth keeping.
    pub body: Option<String>,
    /// Defaults to Todo.
    pub status: Option<StatusArg>,
    /// Defaults to None.
    pub priority: Option<PriorityArg>,
    /// Tags to attach. Accepted here because at creation there is nothing to
    /// clobber; afterwards use add_tag and remove_tag.
    pub tags: Option<Vec<String>>,
    /// File this as a sub-issue of an existing issue. Accepted here because
    /// at creation "absent" unambiguously means "no parent"; afterwards use
    /// add_sub_issue.
    pub parent_id: Option<IssueId>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateIssue {
    /// The id of the issue to change.
    pub id: IssueId,
    /// A field left out is left alone.
    pub title: Option<String>,
    pub body: Option<String>,
    pub status: Option<StatusArg>,
    pub priority: Option<PriorityArg>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangeTag {
    /// The issue to tag or untag.
    pub id: IssueId,
    /// The tag name. Case-insensitive, and may contain spaces or slashes —
    /// "ui/theme" is one flat name, not a tag inside another.
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangeSubIssue {
    /// The issue that holds the sub-issue.
    pub parent_id: IssueId,
    /// The issue that is part of it.
    pub sub_issue_id: IssueId,
}

/// `list_tags` takes nothing, but MCP still wants a schema for it.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoArguments {}
