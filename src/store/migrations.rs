//! Schema migrations.
//!
//! `rusqlite_migration` tracks the applied version in SQLite's `user_version`
//! pragma. Migrations are append-only: never edit one that has shipped, add a
//! new one instead.

use rusqlite_migration::{M, Migrations};

pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(
            "CREATE TABLE issue (
             id         INTEGER PRIMARY KEY,
             title      TEXT NOT NULL,
             body       TEXT NOT NULL DEFAULT '',
             status     TEXT NOT NULL DEFAULT 'Todo',
             priority   TEXT NOT NULL DEFAULT 'None',
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL
         );",
        ),
        // UI preferences live here rather than in a config file so there is a
        // single persistence mechanism, one file location, and one set of
        // failure modes. See docs/adr/0003.
        M::up(
            "CREATE TABLE setting (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );",
        ),
        // Tags are derived from use, so there is no `tag` table: a Tag exists
        // exactly as long as some Issue carries it. See docs/adr/0004.
        // `foreign_keys` is ON (see `Store::prepare`), so erasing an Issue
        // takes its Tags with it.
        M::up(
            "CREATE TABLE issue_tag (
             issue_id INTEGER NOT NULL REFERENCES issue(id) ON DELETE CASCADE,
             name     TEXT NOT NULL,
             PRIMARY KEY (issue_id, name)
         ) WITHOUT ROWID;",
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_valid() {
        assert!(migrations().validate().is_ok());
    }
}
