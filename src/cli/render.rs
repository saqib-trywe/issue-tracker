// SPDX-License-Identifier: GPL-3.0-only

//! Turning API responses into text.
//!
//! Pure with respect to the network: everything here takes already-fetched
//! values, which is what makes it testable without a socket.

use std::collections::HashMap;

use chrono::{DateTime, Local};
use termcolor::{Color, ColorSpec, WriteColor};

use crate::api::wire::{IssueJson, TagJson};
use crate::domain::IssueId;

/// How much of a title survives. Fixed rather than measured: fitting the
/// terminal would mean a `TIOCGWINSZ` ioctl, and an `unsafe` block for column
/// alignment is a poor trade.
const TITLE_WIDTH: usize = 52;

const STATUS_WIDTH: usize = 9;
const PRIORITY_WIDTH: usize = 8;

pub fn list(
    out: &mut dyn WriteColor,
    issues: &[IssueJson],
    settled: &Settled,
) -> std::io::Result<()> {
    if issues.is_empty() {
        return writeln!(out, "No issues.");
    }

    let id_width = issues
        .iter()
        .map(|issue| issue.id.to_string().len())
        .max()
        .unwrap_or(2)
        .max(2);

    dim(out, |out| {
        writeln!(
            out,
            "{:>id_width$}  {:<STATUS_WIDTH$}  {:<PRIORITY_WIDTH$}  {:<TITLE_WIDTH$}  TAGS",
            "ID", "STATUS", "PRIORITY", "TITLE"
        )
    })?;

    for issue in issues {
        write!(out, "{:>id_width$}  ", issue.id)?;
        coloured(out, status_colour(&issue.status), |out| {
            write!(out, "{:<STATUS_WIDTH$}  ", issue.status)
        })?;
        coloured(out, priority_colour(&issue.priority), |out| {
            write!(out, "{:<PRIORITY_WIDTH$}  ", issue.priority)
        })?;
        write!(out, "{:<TITLE_WIDTH$}  ", title_cell(issue, settled))?;
        dim(out, |out| writeln!(out, "{}", issue.tags.join(", ")))?;
    }
    Ok(())
}

pub fn show(
    out: &mut dyn WriteColor,
    issue: &IssueJson,
    parent: Option<&IssueJson>,
    children: &[IssueJson],
) -> std::io::Result<()> {
    writeln!(out, "#{} {}", issue.id, issue.title)?;

    write!(out, "  Status    ")?;
    coloured(out, status_colour(&issue.status), |out| {
        write!(out, "{}", issue.status)
    })?;
    let outstanding = children.iter().filter(|child| !is_settled(child)).count();
    if outstanding > 0 {
        dim(out, |out| {
            write!(out, "  ({outstanding} sub-issue(s) outstanding)")
        })?;
    }
    writeln!(out)?;

    write!(out, "  Priority  ")?;
    coloured(out, priority_colour(&issue.priority), |out| {
        writeln!(out, "{}", issue.priority)
    })?;

    if !issue.tags.is_empty() {
        writeln!(out, "  Tags      {}", issue.tags.join(", "))?;
    }

    // Titles rather than bare ids: an id you then have to look up is not an
    // answer, and both are one request away.
    if let Some(parent) = parent {
        writeln!(out, "  Part of   #{} {}", parent.id, parent.title)?;
    }
    if !children.is_empty() {
        let done = children.len() - outstanding;
        writeln!(out, "  Sub-issues ({done}/{} done)", children.len())?;
        for child in children {
            write!(out, "    #{:<5} ", child.id)?;
            coloured(out, status_colour(&child.status), |out| {
                write!(out, "{:<STATUS_WIDTH$}", child.status)
            })?;
            writeln!(out, "  {}", child.title)?;
        }
    }

    dim(out, |out| {
        writeln!(
            out,
            "  Created   {}\n  Updated   {}",
            timestamp(&issue.created_at),
            timestamp(&issue.updated_at)
        )
    })?;

    if !issue.body.trim().is_empty() {
        writeln!(out, "\n{}", issue.body.trim_end())?;
    }
    Ok(())
}

pub fn tags(out: &mut dyn WriteColor, tags: &[TagJson]) -> std::io::Result<()> {
    if tags.is_empty() {
        return writeln!(out, "No tags.");
    }
    let width = tags.iter().map(|tag| tag.name.len()).max().unwrap_or(0);
    for tag in tags {
        write!(out, "{:<width$}  ", tag.name)?;
        dim(out, |out| writeln!(out, "{}", tag.count))?;
    }
    Ok(())
}

// ---- the sub-issue markers --------------------------------------------------

/// Which Issues are settled, so a parent's progress can be counted.
///
/// Built from whichever Issues were fetched. `list` completes it with a second
/// request when a filter left some children out, rather than printing a
/// fraction that quietly excludes them.
#[derive(Default)]
pub struct Settled(HashMap<IssueId, bool>);

impl Settled {
    pub fn from(issues: &[IssueJson]) -> Self {
        Settled(
            issues
                .iter()
                .map(|issue| (issue.id, is_settled(issue)))
                .collect(),
        )
    }

    /// True when every sub-issue named by `issues` was among them.
    pub fn covers(&self, issues: &[IssueJson]) -> bool {
        issues
            .iter()
            .flat_map(|issue| issue.sub_issue_ids.iter())
            .all(|id| self.0.contains_key(id))
    }

    fn progress(&self, issue: &IssueJson) -> Option<String> {
        if issue.sub_issue_ids.is_empty() {
            return None;
        }
        let done = issue
            .sub_issue_ids
            .iter()
            .filter(|id| self.0.get(id).copied().unwrap_or(false))
            .count();
        Some(format!("[{done}/{}]", issue.sub_issue_ids.len()))
    }
}

/// The two markers the UI's row carries, in the UI's own spelling.
///
/// A sub-issue names its parent by id rather than showing a bare arrow, so the
/// obvious next command (`issue show 12`) is already on the screen.
fn title_cell(issue: &IssueJson, settled: &Settled) -> String {
    let prefix = match issue.parent_id {
        Some(parent) => format!("↳ #{parent} "),
        None => String::new(),
    };
    let suffix = settled
        .progress(issue)
        .map(|progress| format!(" {progress}"))
        .unwrap_or_default();

    let budget = TITLE_WIDTH.saturating_sub(width(&prefix) + width(&suffix));
    format!("{prefix}{}{suffix}", truncate(&issue.title, budget))
}

fn truncate(text: &str, budget: usize) -> String {
    if width(text) <= budget {
        return text.to_string();
    }
    // One column is spent on the ellipsis that says something was cut.
    let keep = budget.saturating_sub(1);
    text.chars().take(keep).collect::<String>() + "…"
}

/// Character count, not byte length — `format!`'s width counts characters too,
/// so alignment holds for a title containing anything non-ASCII.
fn width(text: &str) -> usize {
    text.chars().count()
}

fn is_settled(issue: &IssueJson) -> bool {
    // Parsed rather than string-matched, so this cannot drift from the domain.
    issue
        .status
        .parse::<crate::domain::Status>()
        .map(|status| status.is_settled())
        .unwrap_or(false)
}

/// RFC3339 with microseconds is right on the wire and noise on a screen.
fn timestamp(raw: &str) -> String {
    match DateTime::parse_from_rfc3339(raw) {
        Ok(at) => at
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => raw.to_string(),
    }
}

// ---- colour -----------------------------------------------------------------

fn status_colour(status: &str) -> Option<Color> {
    match status {
        "Todo" => None,
        "Doing" => Some(Color::Blue),
        "Blocked" => Some(Color::Red),
        "Done" => Some(Color::Green),
        "Cancelled" => Some(Color::Magenta),
        _ => None,
    }
}

fn priority_colour(priority: &str) -> Option<Color> {
    match priority {
        "Urgent" => Some(Color::Red),
        "High" => Some(Color::Yellow),
        _ => None,
    }
}

fn coloured(
    out: &mut dyn WriteColor,
    colour: Option<Color>,
    body: impl FnOnce(&mut dyn WriteColor) -> std::io::Result<()>,
) -> std::io::Result<()> {
    match colour {
        Some(colour) => {
            out.set_color(ColorSpec::new().set_fg(Some(colour)))?;
            let result = body(out);
            out.reset()?;
            result
        }
        None => body(out),
    }
}

fn dim(
    out: &mut dyn WriteColor,
    body: impl FnOnce(&mut dyn WriteColor) -> std::io::Result<()>,
) -> std::io::Result<()> {
    out.set_color(ColorSpec::new().set_dimmed(true))?;
    let result = body(out);
    out.reset()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use termcolor::Buffer;

    fn issue(id: IssueId, title: &str, status: &str) -> IssueJson {
        IssueJson {
            id,
            title: title.to_string(),
            body: String::new(),
            status: status.to_string(),
            priority: "Medium".to_string(),
            tags: Vec::new(),
            parent_id: None,
            sub_issue_ids: Vec::new(),
            created_at: "2026-09-05T14:23:11.482913Z".to_string(),
            updated_at: "2026-09-05T14:23:11.482913Z".to_string(),
        }
    }

    fn text(render: impl FnOnce(&mut Buffer) -> std::io::Result<()>) -> String {
        let mut buffer = Buffer::no_color();
        render(&mut buffer).unwrap();
        String::from_utf8(buffer.into_inner()).unwrap()
    }

    #[test]
    fn an_empty_list_says_so_rather_than_printing_a_header() {
        let out = text(|buffer| list(buffer, &[], &Settled::default()));
        assert_eq!(out, "No issues.\n");
    }

    #[test]
    fn a_parent_shows_progress_and_a_sub_issue_names_its_parent() {
        let mut parent = issue(1, "Ship the CLI", "Doing");
        parent.sub_issue_ids = vec![2, 3];
        let mut done = issue(2, "Parse arguments", "Done");
        done.parent_id = Some(1);
        let mut todo = issue(3, "Write the client", "Todo");
        todo.parent_id = Some(1);

        let issues = vec![parent, done, todo];
        let settled = Settled::from(&issues);
        let out = text(|buffer| list(buffer, &issues, &settled));

        assert!(out.contains("Ship the CLI [1/2]"), "{out}");
        assert!(out.contains("↳ #1 Parse arguments"), "{out}");
    }

    #[test]
    fn cancelled_counts_as_settled_for_progress() {
        let mut parent = issue(1, "p", "Todo");
        parent.sub_issue_ids = vec![2];
        let mut child = issue(2, "c", "Cancelled");
        child.parent_id = Some(1);

        let issues = vec![parent, child];
        let out = text(|buffer| list(buffer, &issues, &Settled::from(&issues)));
        assert!(out.contains("[1/1]"), "{out}");
    }

    #[test]
    fn a_filtered_list_that_hides_a_child_is_detected() {
        let mut parent = issue(1, "p", "Todo");
        parent.sub_issue_ids = vec![2, 3];
        // Only the parent came back; the fraction would be wrong.
        let partial = vec![parent];
        assert!(!Settled::from(&partial).covers(&partial));

        let whole = vec![issue(2, "a", "Done"), issue(3, "b", "Todo")];
        let mut both = whole;
        both.extend(partial.iter().map(|issue| IssueJson {
            id: issue.id,
            title: issue.title.clone(),
            body: String::new(),
            status: issue.status.clone(),
            priority: issue.priority.clone(),
            tags: Vec::new(),
            parent_id: None,
            sub_issue_ids: issue.sub_issue_ids.clone(),
            created_at: issue.created_at.clone(),
            updated_at: issue.updated_at.clone(),
        }));
        assert!(Settled::from(&both).covers(&both));
    }

    #[test]
    fn a_long_title_is_cut_without_losing_the_markers() {
        let mut long = issue(1, &"x".repeat(200), "Todo");
        long.parent_id = Some(9);
        long.sub_issue_ids = vec![2];

        let cell = title_cell(&long, &Settled::default());
        assert_eq!(width(&cell), TITLE_WIDTH);
        assert!(cell.starts_with("↳ #9 "), "{cell}");
        assert!(cell.ends_with("[0/1]"), "{cell}");
        assert!(cell.contains('…'), "{cell}");
    }

    #[test]
    fn a_short_title_is_left_alone() {
        assert_eq!(truncate("brief", 20), "brief");
        assert_eq!(truncate("exactly-ten", 11), "exactly-ten");
    }

    #[test]
    fn show_names_the_related_issues_rather_than_their_ids_alone() {
        let mut child = issue(5, "the child", "Todo");
        child.parent_id = Some(1);
        child.body = "some prose".to_string();
        let parent = issue(1, "the parent", "Doing");

        let out = text(|buffer| show(buffer, &child, Some(&parent), &[]));
        assert!(out.contains("#5 the child"), "{out}");
        assert!(out.contains("Part of   #1 the parent"), "{out}");
        assert!(out.contains("some prose"), "{out}");
    }

    #[test]
    fn show_counts_outstanding_work_under_a_parent() {
        let mut parent = issue(1, "the parent", "Todo");
        parent.sub_issue_ids = vec![2, 3];
        let children = vec![issue(2, "done one", "Done"), issue(3, "not yet", "Todo")];

        let out = text(|buffer| show(buffer, &parent, None, &children));
        assert!(out.contains("1 sub-issue(s) outstanding"), "{out}");
        assert!(out.contains("Sub-issues (1/2 done)"), "{out}");
    }

    #[test]
    fn timestamps_lose_their_microseconds() {
        let rendered = timestamp("2026-09-05T14:23:11.482913Z");
        assert!(!rendered.contains('.'), "{rendered}");
        assert!(rendered.starts_with("2026-09-05") || rendered.starts_with("2026-09-0"));
        // Anything unparseable is passed through rather than hidden.
        assert_eq!(timestamp("not a date"), "not a date");
    }

    #[test]
    fn tags_are_listed_with_their_counts() {
        let out = text(|buffer| {
            tags(
                buffer,
                &[
                    TagJson {
                        name: "bug".into(),
                        count: 3,
                    },
                    TagJson {
                        name: "ui".into(),
                        count: 1,
                    },
                ],
            )
        });
        assert_eq!(out, "bug  3\nui   1\n");
    }
}
