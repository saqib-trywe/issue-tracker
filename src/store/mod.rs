//! SQLite persistence for Issues.
//!
//! The UI keeps every Issue in memory and calls through here on mutation, so
//! nothing on the render path ever touches I/O. Queries are synchronous
//! because a local SQLite read of a few thousand rows is sub-millisecond and
//! `rusqlite` needs no async runtime alongside GPUI's own executor.

mod migrations;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, Row};

use crate::domain::{Issue, IssueId, Priority, Status};

/// Points the store at a scratch database during development so experiments
/// never touch real data.
pub const DB_PATH_ENV: &str = "ISSUE_TRACKER_DB";

const SELECT_COLUMNS: &str = "id, title, body, status, priority, created_at, updated_at";

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens the database at [`db_path`], creating parent directories and
    /// applying migrations.
    pub fn open() -> Result<Self> {
        let path = db_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating data directory {}", parent.display()))?;
        }
        Self::open_at(&path)
    }

    pub fn open_at(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("opening database at {}", path.display()))?;
        Self::prepare(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrations::migrations()
            .to_latest(&mut conn)
            .context("applying schema migrations")?;
        Ok(Self { conn })
    }

    /// Loads every Issue. The caller owns the result and renders from it.
    pub fn load_all(&self) -> Result<Vec<Issue>> {
        let mut statement = self
            .conn
            .prepare(&format!("SELECT {SELECT_COLUMNS} FROM issue"))?;
        let issues = statement
            .query_map([], read_issue)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(issues)
    }

    /// Creates a title-only Issue at the default Status and Priority.
    pub fn insert(&self, title: &str) -> Result<Issue> {
        let now = Utc::now();
        let issue = self.conn.query_row(
            &format!(
                "INSERT INTO issue (title, body, status, priority, created_at, updated_at)
                 VALUES (?1, '', ?2, ?3, ?4, ?4)
                 RETURNING {SELECT_COLUMNS}"
            ),
            rusqlite::params![
                title,
                Status::default().label(),
                Priority::default().label(),
                format_timestamp(now),
            ],
            read_issue,
        )?;
        Ok(issue)
    }

    /// Writes every mutable field and stamps `updated_at`, returning the new
    /// timestamp so the caller can keep its in-memory copy in step.
    pub fn update(&self, issue: &Issue) -> Result<DateTime<Utc>> {
        let now = Utc::now();
        self.conn.execute(
            "UPDATE issue
                SET title = ?2, body = ?3, status = ?4, priority = ?5, updated_at = ?6
              WHERE id = ?1",
            rusqlite::params![
                issue.id,
                issue.title,
                issue.body,
                issue.status.label(),
                issue.priority.label(),
                format_timestamp(now),
            ],
        )?;
        Ok(now)
    }

    /// Erases an Issue outright. Cancelling is a Status change, not this.
    pub fn delete(&self, id: IssueId) -> Result<()> {
        self.conn
            .execute("DELETE FROM issue WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }
}

/// The database file location: `$ISSUE_TRACKER_DB` when set, otherwise the
/// platform data directory.
pub fn db_path() -> Result<PathBuf> {
    if let Some(override_path) = std::env::var_os(DB_PATH_ENV) {
        return Ok(PathBuf::from(override_path));
    }

    let dirs = directories::ProjectDirs::from("", "", "gpui-issue-tracker")
        .context("locating the platform data directory")?;
    Ok(dirs.data_dir().join("issues.db"))
}

fn format_timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn parse_timestamp(raw: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, err.into())
        })
}

fn parse_column<T>(raw: &str, index: usize) -> rusqlite::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    raw.parse().map_err(|err: T::Err| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn read_issue(row: &Row<'_>) -> rusqlite::Result<Issue> {
    Ok(Issue {
        id: row.get(0)?,
        title: row.get(1)?,
        body: row.get(2)?,
        status: parse_column(&row.get::<_, String>(3)?, 3)?,
        priority: parse_column(&row.get::<_, String>(4)?, 4)?,
        created_at: parse_timestamp(&row.get::<_, String>(5)?)?,
        updated_at: parse_timestamp(&row.get::<_, String>(6)?)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().expect("in-memory store")
    }

    #[test]
    fn a_new_store_is_empty() {
        assert!(store().load_all().unwrap().is_empty());
    }

    #[test]
    fn insert_defaults_to_todo_and_no_priority() {
        let store = store();
        let issue = store.insert("Wire up the sidebar").unwrap();

        assert_eq!(issue.title, "Wire up the sidebar");
        assert_eq!(issue.body, "");
        assert_eq!(issue.status, Status::Todo);
        assert_eq!(issue.priority, Priority::None);
        assert_eq!(issue.created_at, issue.updated_at);
    }

    #[test]
    fn ids_are_sequential() {
        let store = store();
        let first = store.insert("first").unwrap();
        let second = store.insert("second").unwrap();
        assert_eq!(second.id, first.id + 1);
    }

    #[test]
    fn inserted_issues_survive_reload() {
        let store = store();
        store.insert("persisted").unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].title, "persisted");
    }

    #[test]
    fn update_writes_every_mutable_field() {
        let store = store();
        let mut issue = store.insert("draft").unwrap();

        issue.title = "sharpened".into();
        issue.body = "with a body".into();
        issue.status = Status::Blocked;
        issue.priority = Priority::Urgent;
        store.update(&issue).unwrap();

        let reloaded = store.load_all().unwrap().remove(0);
        assert_eq!(reloaded.title, "sharpened");
        assert_eq!(reloaded.body, "with a body");
        assert_eq!(reloaded.status, Status::Blocked);
        assert_eq!(reloaded.priority, Priority::Urgent);
    }

    #[test]
    fn update_advances_updated_at_but_not_created_at() {
        let store = store();
        let issue = store.insert("draft").unwrap();

        let updated_at = store.update(&issue).unwrap();

        let reloaded = store.load_all().unwrap().remove(0);
        assert_eq!(reloaded.created_at, issue.created_at);
        assert!(reloaded.updated_at >= issue.updated_at);
        assert_eq!(
            format_timestamp(reloaded.updated_at),
            format_timestamp(updated_at)
        );
    }

    #[test]
    fn delete_removes_only_its_own_issue() {
        let store = store();
        let doomed = store.insert("typo").unwrap();
        let kept = store.insert("real work").unwrap();

        store.delete(doomed.id).unwrap();

        let remaining = store.load_all().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, kept.id);
    }

    #[test]
    fn timestamps_round_trip_to_microsecond_precision() {
        let store = store();
        let issue = store.insert("precise").unwrap();
        let reloaded = store.load_all().unwrap().remove(0);
        assert_eq!(
            format_timestamp(issue.created_at),
            format_timestamp(reloaded.created_at)
        );
    }

    #[test]
    fn env_override_takes_precedence_over_platform_dir() {
        // Guards the dev-safety property: with the override set, we never
        // resolve to the real data directory.
        unsafe { std::env::set_var(DB_PATH_ENV, "/tmp/scratch-issues.db") };
        let path = db_path().unwrap();
        unsafe { std::env::remove_var(DB_PATH_ENV) };

        assert_eq!(path, PathBuf::from("/tmp/scratch-issues.db"));
    }
}
