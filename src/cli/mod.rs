// SPDX-License-Identifier: GPL-3.0-only

//! `issue` — the command-line surface.
//!
//! A *client* of the local HTTP API, never a second writer: everything goes
//! through the running app, so the window's projection and the database can
//! never disagree about what happened. The cost is that these commands need
//! Issues to be running, which is reported as its own exit code rather than
//! folded in with genuine failures. See `docs/adr/0008`.

pub mod args;
mod render;

use std::ffi::OsString;
use std::io::{IsTerminal, Read, Write};

use serde_json::json;
use termcolor::{ColorChoice, StandardStream, WriteColor};

use crate::api::wire::{IssueJson, TagJson};
use crate::domain::{IssueId, Priority, Status};

use crate::client::{self, Client, ClientError, Reply, encode};
use args::{Body, Changes, Colour, Command, Filters, NewIssue, ParentFilter};

/// Built from the domain rather than written out, so the list of statuses can
/// never drift from the ones `FromStr` accepts.
fn help() -> String {
    let statuses: Vec<&str> = Status::ALL.iter().map(|status| status.label()).collect();
    let priorities: Vec<&str> = Priority::ALL
        .iter()
        .map(|priority| priority.label())
        .collect();

    format!(
        "\
issue — the command line for the Issues tracker

USAGE
  issue <command> [options]

COMMANDS
  list                        List issues, most pressing first
  show <id>                   Show one issue in full
  new \"<title>\"               File a new issue
  set <id>                    Change an issue's fields
  rm <id>                     Delete an issue permanently
  tag add|rm <id> <name>...   Add or remove tags
  sub add|rm <id> <child>...  Attach or detach sub-issues
  tags                        List the tags in use, with counts

LIST OPTIONS
  --status <status>           {statuses}
  --tag <name>                Only issues carrying this tag
  --parent <id|none>          Sub-issues of an issue, or only unparented ones
  --search <text>             Only issues whose title contains this

NEW AND SET OPTIONS
  --title <text>              set only
  --body <text|->             \"-\" reads the body from standard input
  --status <status>
  --priority <priority>       {priorities}
  --tag <name>                new only, repeatable. Use `issue tag` to change
                              the tags of an issue that already exists
  --parent <id>               new only

GLOBAL OPTIONS
  --json                      Print the API's JSON instead of a table
  --color <auto|always|never>
  -h, --help                  Show this
  -V, --version               Show the version

Status and priority names are case-insensitive. Deleting is permanent; to
record that you decided against an issue instead, use --status Cancelled.

Issues must be running: `issue` talks to it over a local HTTP API rather than
opening the database itself.
",
        statuses = statuses.join(", "),
        priorities = priorities.join(", "),
    )
}

/// Why a command did not finish, and what the shell should be told.
///
/// The three kinds are separated because a caller's next move differs: fix the
/// command, fix the request, or start the app.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// The command line was wrong. Exit 2.
    Usage(String),
    /// The request was made and refused, or something else broke. Exit 1.
    Failed(String),
    /// There is no API to talk to. Exit 3, so a wrapper can tell this apart
    /// from a genuine failure and offer to start the app.
    NotRunning(String),
}

impl Failure {
    pub fn usage(message: impl Into<String>) -> Self {
        Failure::Usage(message.into())
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Failure::Failed(message.into())
    }

    pub fn not_running(message: impl Into<String>) -> Self {
        Failure::NotRunning(message.into())
    }

    pub fn code(&self) -> u8 {
        match self {
            Failure::Failed(_) => 1,
            Failure::Usage(_) => 2,
            Failure::NotRunning(_) => 3,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Failure::Usage(message) | Failure::Failed(message) | Failure::NotRunning(message) => {
                message
            }
        }
    }
}

/// The client knows whether there was an app to talk to; only the CLI knows
/// that this is worth a distinct exit code.
impl From<ClientError> for Failure {
    fn from(err: ClientError) -> Self {
        match err {
            ClientError::NotRunning(message) => Failure::NotRunning(message),
            ClientError::Failed(message) => Failure::Failed(message),
        }
    }
}

/// Runs one invocation and returns the process exit code.
///
/// Errors go to stderr as prose whatever `--json` says, so stdout is always
/// either valid JSON or empty and `| jq` never chokes on an explanation.
pub fn run(argv: Vec<OsString>) -> u8 {
    // Resolved before parsing so a failure is reported to a stream that has
    // already decided about colour; `--color` is read twice, harmlessly.
    let colour = args::colour_of(&argv);
    let mut out = stream(colour);

    match run_into(argv, &mut out) {
        Ok(()) => 0,
        Err(failure) => {
            eprintln!("issue: {}", failure.message());
            failure.code()
        }
    }
}

/// The whole command, writing to a caller-supplied stream.
///
/// Separate from [`run`] so an integration test can drive a real request over
/// a real socket and read what came out, without spawning a process.
pub fn run_into(argv: Vec<OsString>, out: &mut dyn WriteColor) -> Result<(), Failure> {
    let invocation = args::parse(argv)?;

    match invocation.command {
        Command::Help => write!(out, "{}", help()).map_err(broken_pipe),
        Command::Version => {
            writeln!(out, "issue {}", env!("CARGO_PKG_VERSION")).map_err(broken_pipe)
        }
        command => {
            let client = client::connect()?;
            dispatch(&client, out, invocation.json, command)
        }
    }
}

fn dispatch(
    client: &Client,
    out: &mut dyn WriteColor,
    json_only: bool,
    command: Command,
) -> Result<(), Failure> {
    match command {
        Command::Help | Command::Version => unreachable!("handled before connecting"),

        Command::List(filters) => {
            let reply = client.get(&list_path(&filters))?;
            let issues: Vec<IssueJson> = decode(&ok(reply)?)?;
            if json_only {
                return emit(out, &issues);
            }

            // A filter can hide a parent's children, which would make its
            // progress fraction quietly wrong. One extra request fixes it; the
            // unfiltered case never needs it.
            let mut settled = render::Settled::from(&issues);
            if !settled.covers(&issues) {
                let everything: Vec<IssueJson> = decode(&ok(client.get("/issues")?)?)?;
                settled = render::Settled::from(&everything);
            }
            render::list(out, &issues, &settled).map_err(broken_pipe)
        }

        Command::Show(id) => {
            let reply = ok(client.get(&format!("/issues/{id}"))?)?;
            if json_only {
                return raw(out, &reply);
            }
            let issue: IssueJson = decode(&reply)?;
            present(client, out, &issue)
        }

        Command::New(new) => {
            let reply = ok(client.post("/issues", &create_body(*new)?)?)?;
            if json_only {
                return raw(out, &reply);
            }
            let issue: IssueJson = decode(&reply)?;
            present(client, out, &issue)
        }

        Command::Set(id, changes) => {
            let reply = ok(client.patch(&format!("/issues/{id}"), &patch_body(*changes)?)?)?;
            if json_only {
                return raw(out, &reply);
            }
            let issue: IssueJson = decode(&reply)?;
            present(client, out, &issue)
        }

        Command::Remove { id, force } => remove(client, out, id, force),

        Command::Tags => {
            let reply = ok(client.get("/tags")?)?;
            if json_only {
                return raw(out, &reply);
            }
            let tags: Vec<TagJson> = decode(&reply)?;
            render::tags(out, &tags).map_err(broken_pipe)
        }

        Command::Tag { id, names, add } => {
            let paths: Vec<(String, String)> = names
                .iter()
                .map(|tag| {
                    (
                        tag.as_str().to_string(),
                        format!("/issues/{id}/tags/{}", encode(tag.as_str(), true)),
                    )
                })
                .collect();
            each(client, out, json_only, add, paths)
        }

        Command::Sub { id, children, add } => {
            let paths: Vec<(String, String)> = children
                .iter()
                .map(|child| {
                    (
                        format!("#{child}"),
                        format!("/issues/{id}/sub-issues/{child}"),
                    )
                })
                .collect();
            each(client, out, json_only, add, paths)
        }
    }
}

/// Applies one request per named thing, in order.
///
/// A refusal partway through leaves the earlier ones applied, so the error
/// says which landed — otherwise `issue tag add 7 a b c` failing on `b` looks
/// like it did nothing.
fn each(
    client: &Client,
    out: &mut dyn WriteColor,
    json_only: bool,
    add: bool,
    paths: Vec<(String, String)>,
) -> Result<(), Failure> {
    if paths.is_empty() {
        return Err(Failure::usage("nothing to add or remove"));
    }

    let mut applied: Vec<String> = Vec::new();
    let mut last = None;

    for (name, path) in paths {
        let reply = if add {
            client.put(&path)?
        } else {
            client.delete(&path)?
        };
        match ok(reply) {
            Ok(reply) => {
                applied.push(name);
                last = Some(reply);
            }
            Err(failure) => {
                let verb = if add { "applied" } else { "removed" };
                let already = if applied.is_empty() {
                    String::new()
                } else {
                    format!(" (already {verb}: {})", applied.join(", "))
                };
                return Err(Failure::failed(format!(
                    "{name}: {}{already}",
                    failure.message()
                )));
            }
        }
    }

    // The final reply is the Issue's finished state, which is what was asked
    // about however many requests it took to get there.
    let reply = last.expect("a non-empty list applied at least one");
    if json_only {
        return raw(out, &reply);
    }
    let issue: IssueJson = decode(&reply)?;
    present(client, out, &issue)
}

fn remove(
    client: &Client,
    out: &mut dyn WriteColor,
    id: IssueId,
    force: bool,
) -> Result<(), Failure> {
    // Fetched first so the prompt can name what is about to go, and say what
    // happens to its sub-issues — the one thing about deleting that surprises
    // people. `--force` skips both the prompt and the request.
    if !force && std::io::stdin().is_terminal() {
        let issue: IssueJson = decode(&ok(client.get(&format!("/issues/{id}"))?)?)?;
        let released = match issue.sub_issue_ids.len() {
            0 => String::new(),
            1 => " Its 1 sub-issue will be kept, no longer part of anything.".to_string(),
            many => format!(" Its {many} sub-issues will be kept, no longer part of anything."),
        };
        eprintln!(
            "#{} \u{201c}{}\u{201d} will be erased.{released}\n\
             To abandon it but keep the record, use `issue set {} --status Cancelled`.",
            issue.id, issue.title, issue.id
        );
        eprint!("Delete it? [y/N] ");
        std::io::stderr().flush().ok();

        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(|err| Failure::failed(format!("reading your answer: {err}")))?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "Yes") {
            return Err(Failure::failed("cancelled"));
        }
    } else if !force {
        return Err(Failure::usage(
            "deleting is permanent and this is not a terminal: pass --force to mean it",
        ));
    }

    ok(client.delete(&format!("/issues/{id}"))?)?;
    writeln!(out, "Deleted #{id}.").map_err(broken_pipe)
}

/// Renders one Issue with its neighbours named rather than numbered.
///
/// At most three requests: the Issue itself, its parent, and — thanks to
/// `?parent=`, added for exactly this — all of its children in one go, rather
/// than one request per child.
fn present(client: &Client, out: &mut dyn WriteColor, issue: &IssueJson) -> Result<(), Failure> {
    let parent = match issue.parent_id {
        Some(parent) => Some(decode::<IssueJson>(&ok(
            client.get(&format!("/issues/{parent}"))?
        )?)?),
        None => None,
    };
    let children: Vec<IssueJson> = if issue.sub_issue_ids.is_empty() {
        Vec::new()
    } else {
        decode(&ok(client.get(&format!("/issues?parent={}", issue.id))?)?)?
    };

    render::show(out, issue, parent.as_ref(), &children).map_err(broken_pipe)
}

// ---- request and response plumbing ------------------------------------------

fn list_path(filters: &Filters) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(status) = filters.status {
        query.push(format!("status={}", status.label()));
    }
    if let Some(tag) = &filters.tag {
        query.push(format!("tag={}", encode(tag.as_str(), false)));
    }
    if let Some(search) = &filters.search {
        query.push(format!("q={}", encode(search, false)));
    }
    match filters.parent {
        Some(ParentFilter::Unparented) => query.push("parent=none".to_string()),
        Some(ParentFilter::Under(id)) => query.push(format!("parent={id}")),
        None => {}
    }

    if query.is_empty() {
        "/issues".to_string()
    } else {
        format!("/issues?{}", query.join("&"))
    }
}

fn create_body(new: NewIssue) -> Result<String, Failure> {
    let mut body = json!({ "title": new.title });
    let map = body.as_object_mut().expect("an object");
    if let Some(source) = new.body {
        map.insert("body".into(), json!(read_body(source)?));
    }
    if let Some(status) = new.status {
        map.insert("status".into(), json!(status.label()));
    }
    if let Some(priority) = new.priority {
        map.insert("priority".into(), json!(priority.label()));
    }
    if !new.tags.is_empty() {
        let tags: Vec<&str> = new.tags.iter().map(|tag| tag.as_str()).collect();
        map.insert("tags".into(), json!(tags));
    }
    if let Some(parent) = new.parent {
        map.insert("parent_id".into(), json!(parent));
    }
    Ok(body.to_string())
}

fn patch_body(changes: Changes) -> Result<String, Failure> {
    let mut body = json!({});
    let map = body.as_object_mut().expect("an object");
    if let Some(title) = changes.title {
        map.insert("title".into(), json!(title));
    }
    if let Some(source) = changes.body {
        map.insert("body".into(), json!(read_body(source)?));
    }
    if let Some(status) = changes.status {
        map.insert("status".into(), json!(status.label()));
    }
    if let Some(priority) = changes.priority {
        map.insert("priority".into(), json!(priority.label()));
    }
    Ok(body.to_string())
}

fn read_body(source: Body) -> Result<String, Failure> {
    match source {
        Body::Text(text) => Ok(text),
        Body::Stdin => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|err| Failure::failed(format!("reading the body from stdin: {err}")))?;
            Ok(text)
        }
    }
}

/// Turns a non-2xx reply into a failure carrying the API's own sentence,
/// including the 409 refusals the domain rules produce.
fn ok(reply: Reply) -> Result<Reply, Failure> {
    if (200..300).contains(&reply.status) {
        Ok(reply)
    } else {
        Err(Failure::failed(reply.error_message()))
    }
}

fn decode<T: serde::de::DeserializeOwned>(reply: &Reply) -> Result<T, Failure> {
    serde_json::from_slice(&reply.body)
        .map_err(|err| Failure::failed(format!("could not read the API's response: {err}")))
}

fn emit<T: serde::Serialize>(out: &mut dyn WriteColor, value: &T) -> Result<(), Failure> {
    let text = serde_json::to_string(value)
        .map_err(|err| Failure::failed(format!("could not write JSON: {err}")))?;
    writeln!(out, "{text}").map_err(broken_pipe)
}

/// Passes the API's bytes through untouched, so `--json` is exactly what the
/// API said rather than a re-serialisation of it.
fn raw(out: &mut dyn WriteColor, reply: &Reply) -> Result<(), Failure> {
    out.write_all(&reply.body).map_err(broken_pipe)?;
    if !reply.body.ends_with(b"\n") {
        writeln!(out).map_err(broken_pipe)?;
    }
    Ok(())
}

/// `issue list | head` closes the pipe early. That is the reader's decision,
/// not an error worth reporting.
fn broken_pipe(err: std::io::Error) -> Failure {
    if err.kind() == std::io::ErrorKind::BrokenPipe {
        std::process::exit(0);
    }
    Failure::failed(format!("writing output: {err}"))
}

/// termcolor's own `Auto` inspects `TERM` but not whether stdout is a
/// terminal, so the decision is made here.
fn stream(colour: Colour) -> StandardStream {
    let choice = match colour {
        Colour::Always => ColorChoice::Always,
        Colour::Never => ColorChoice::Never,
        Colour::Auto => {
            if std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
                ColorChoice::Auto
            } else {
                ColorChoice::Never
            }
        }
    };
    StandardStream::stdout(choice)
}
