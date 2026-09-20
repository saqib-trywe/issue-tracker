// SPDX-License-Identifier: GPL-3.0-only

//! Every request the API understands, built once.
//!
//! This module knows the API's vocabulary — which path addresses a Tag, that
//! `parent=none` is spelled out, that a Tag name keeps its slashes — and
//! nothing about sockets. Every function here is pure: it returns a [`Call`]
//! describing a request, and [`crate::client::Client::send`] performs it.
//! That is what lets the exact bytes of a path and a body be asserted in a
//! unit test, which is where these had no coverage at all.
//!
//! Both clients go through here. They had already begun to drift — the
//! command line sent a Status unencoded where the MCP server encoded it —
//! and that is the cheaper of the two failures this shape prevents.

use serde::Serialize;

use crate::api::wire::{NewIssue, PatchIssue};
use crate::domain::{IssueId, Narrowing, ParentFilter, Tag, View};

/// The method, carrying the body — because only two methods have one, and
/// pairing them makes it impossible to send a body nowhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    Get,
    Post(String),
    Patch(String),
    Put,
    Delete,
}

impl Method {
    pub fn name(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post(_) => "POST",
            Method::Patch(_) => "PATCH",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }

    pub fn body(&self) -> Option<&str> {
        match self {
            Method::Post(body) | Method::Patch(body) => Some(body),
            _ => None,
        }
    }
}

/// One request, described but not yet made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub method: Method,
    pub path: String,
}

// ---- issues -----------------------------------------------------------------

/// `GET /issues`, narrowed.
pub fn list(narrowing: &Narrowing) -> Call {
    let mut query: Vec<String> = Vec::new();

    // `View::All` is the absence of a Status filter, not a Status called
    // "All", so it contributes nothing to the query.
    if let View::WithStatus(status) = narrowing.view {
        query.push(format!("status={}", encode(status.label(), false)));
    }
    if let Some(tag) = &narrowing.tag {
        query.push(format!("tag={}", encode(tag.as_str(), false)));
    }
    if let Some(parent) = narrowing.parent {
        let value = match parent {
            ParentFilter::Unparented => "none".to_string(),
            ParentFilter::Under(id) => id.to_string(),
        };
        query.push(format!("parent={value}"));
    }
    if let Some(title) = &narrowing.title {
        query.push(format!("q={}", encode(title, false)));
    }

    Call {
        method: Method::Get,
        path: match query.is_empty() {
            true => "/issues".to_string(),
            false => format!("/issues?{}", query.join("&")),
        },
    }
}

pub fn get(id: IssueId) -> Call {
    Call {
        method: Method::Get,
        path: format!("/issues/{id}"),
    }
}

pub fn create(new: &NewIssue) -> Call {
    Call {
        method: Method::Post(encode_body(new)),
        path: "/issues".to_string(),
    }
}

/// `PATCH /issues/{id}`. A field the patch does not name is left alone, which
/// is why absent fields are omitted rather than sent as null.
pub fn update(id: IssueId, patch: &PatchIssue) -> Call {
    Call {
        method: Method::Patch(encode_body(patch)),
        path: format!("/issues/{id}"),
    }
}

pub fn delete(id: IssueId) -> Call {
    Call {
        method: Method::Delete,
        path: format!("/issues/{id}"),
    }
}

// ---- tags -------------------------------------------------------------------

pub fn add_tag(id: IssueId, tag: &Tag) -> Call {
    Call {
        method: Method::Put,
        path: tag_path(id, tag),
    }
}

pub fn remove_tag(id: IssueId, tag: &Tag) -> Call {
    Call {
        method: Method::Delete,
        path: tag_path(id, tag),
    }
}

pub fn tags() -> Call {
    Call {
        method: Method::Get,
        path: "/tags".to_string(),
    }
}

// ---- sub-issues -------------------------------------------------------------

pub fn add_sub_issue(parent: IssueId, child: IssueId) -> Call {
    Call {
        method: Method::Put,
        path: sub_issue_path(parent, child),
    }
}

pub fn remove_sub_issue(parent: IssueId, child: IssueId) -> Call {
    Call {
        method: Method::Delete,
        path: sub_issue_path(parent, child),
    }
}

// ---- paths ------------------------------------------------------------------

/// A Tag name is the whole remainder of the path, slashes included, so
/// `ui/theme` addresses one Tag rather than four segments.
fn tag_path(id: IssueId, tag: &Tag) -> String {
    format!("/issues/{id}/tags/{}", encode(tag.as_str(), true))
}

fn sub_issue_path(parent: IssueId, child: IssueId) -> String {
    format!("/issues/{parent}/sub-issues/{child}")
}

fn encode_body<T: Serialize>(body: &T) -> String {
    // Serialising our own types cannot realistically fail; an empty object is
    // a harmless no-op body if it somehow did.
    serde_json::to_string(body).unwrap_or_else(|_| "{}".to_string())
}

/// Percent-encodes a path segment or query value.
///
/// Private, and the only encoder: a slash survives in a path because a Tag
/// name is the whole remainder there, and does not survive in a query value,
/// where it is not. Two callers encoding that differently would disagree
/// about which Tag they meant.
fn encode(raw: &str, path_segment: bool) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        let safe = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (path_segment && byte == b'/');
        if safe {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Status;

    fn tag(name: &str) -> Tag {
        name.parse().expect("a usable tag")
    }

    #[test]
    fn an_unnarrowed_listing_asks_no_questions() {
        let call = list(&Narrowing::default());
        assert_eq!(call.method, Method::Get);
        assert_eq!(call.path, "/issues");
    }

    #[test]
    fn every_narrowing_reaches_the_query_string() {
        let call = list(&Narrowing {
            view: View::WithStatus(Status::Doing),
            tag: Some(tag("ui")),
            title: Some("flash".into()),
            parent: Some(ParentFilter::Under(7)),
        });
        assert_eq!(call.path, "/issues?status=Doing&tag=ui&parent=7&q=flash");
    }

    #[test]
    fn view_all_is_the_absence_of_a_status() {
        // Not a Status called "All": the server would reject that.
        let call = list(&Narrowing::for_view(View::All));
        assert_eq!(call.path, "/issues");
    }

    #[test]
    fn unparented_is_spelled_out() {
        let call = list(&Narrowing {
            parent: Some(ParentFilter::Unparented),
            ..Default::default()
        });
        assert_eq!(call.path, "/issues?parent=none");
    }

    #[test]
    fn query_values_are_encoded_and_a_slash_is_not_special_there() {
        let call = list(&Narrowing {
            tag: Some(tag("ui/theme")),
            title: Some("needs design & care".into()),
            ..Default::default()
        });
        assert_eq!(
            call.path,
            "/issues?tag=ui%2Ftheme&q=needs%20design%20%26%20care"
        );
    }

    #[test]
    fn a_tag_name_keeps_its_slashes_but_loses_its_spaces() {
        // The API reads everything after `/tags/` as the name, so a slash
        // must survive there; a space must not.
        assert_eq!(add_tag(3, &tag("ui/theme")).path, "/issues/3/tags/ui/theme");
        assert_eq!(
            remove_tag(3, &tag("needs design")).path,
            "/issues/3/tags/needs%20design"
        );
        assert_eq!(add_tag(3, &tag("ui")).method, Method::Put);
        assert_eq!(remove_tag(3, &tag("ui")).method, Method::Delete);
    }

    #[test]
    fn a_patch_names_only_what_it_changes() {
        // The whole point of PATCH: a body carrying `"title": null` would be
        // a caller claiming to change a title it never mentioned.
        let call = update(
            7,
            &PatchIssue {
                status: Some("Done".into()),
                ..Default::default()
            },
        );
        assert_eq!(call.path, "/issues/7");
        assert_eq!(call.method, Method::Patch(r#"{"status":"Done"}"#.into()));
    }

    #[test]
    fn an_empty_patch_says_nothing_at_all() {
        assert_eq!(
            update(7, &PatchIssue::default()).method,
            Method::Patch("{}".into())
        );
    }

    #[test]
    fn a_create_body_carries_the_title_and_whatever_else_was_given() {
        let call = create(&NewIssue {
            title: "Ship it".into(),
            parent_id: Some(2),
            rest: PatchIssue {
                status: Some("Doing".into()),
                tags: Some(vec!["ui".into()]),
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(call.path, "/issues");
        assert_eq!(
            call.method,
            Method::Post(
                r#"{"title":"Ship it","parent_id":2,"status":"Doing","tags":["ui"]}"#.into()
            )
        );
    }

    #[test]
    fn a_bare_create_sends_only_a_title() {
        let call = create(&NewIssue {
            title: "Just this".into(),
            ..Default::default()
        });
        assert_eq!(call.method, Method::Post(r#"{"title":"Just this"}"#.into()));
    }

    #[test]
    fn the_remaining_endpoints_are_addressed_as_the_api_spells_them() {
        assert_eq!(get(7).path, "/issues/7");
        assert_eq!(get(7).method, Method::Get);
        assert_eq!(delete(7).method, Method::Delete);
        assert_eq!(tags().path, "/tags");
        assert_eq!(add_sub_issue(1, 2).path, "/issues/1/sub-issues/2");
        assert_eq!(add_sub_issue(1, 2).method, Method::Put);
        assert_eq!(remove_sub_issue(1, 2).method, Method::Delete);
    }

    #[test]
    fn a_method_carries_its_own_body_and_name() {
        assert_eq!(Method::Get.name(), "GET");
        assert_eq!(Method::Get.body(), None);
        assert_eq!(Method::Put.body(), None);
        assert_eq!(Method::Post("{}".into()).body(), Some("{}"));
        assert_eq!(Method::Patch("{}".into()).name(), "PATCH");
    }
}
