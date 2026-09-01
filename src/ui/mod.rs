//! The view layer.
//!
//! `tracker` owns all state; the other modules are `impl IssueTracker` blocks
//! split by column so each file stays readable.

mod issue_detail;
mod issue_list;
mod sidebar;
mod tracker;

pub use tracker::{IssueTracker, init};
