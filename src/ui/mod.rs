//! The view layer.
//!
//! `tracker` owns all state; the other modules are `impl IssueTracker` blocks
//! split by column so each file stays readable.

pub mod api_server;
mod issue_detail;
mod issue_list;
pub mod menus;
mod sidebar;
mod tag_colour;
mod theme;
mod theme_catalogue;
mod tracker;

pub use theme::apply_system_appearance;
pub use tracker::{IssueTracker, init};
