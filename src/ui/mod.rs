// SPDX-License-Identifier: GPL-3.0-only

//! The view layer.
//!
//! `tracker` owns the view; the modules beside it are further `impl
//! IssueTracker` blocks split by column so each file stays readable.
//! `working_state` is the exception — it is a module in its own right, free of
//! `gpui`, holding what this window is looking at.

pub mod api_server;
mod issue_detail;
mod issue_list;
pub mod menus;
mod sidebar;
mod tag_colour;
mod theme;
mod theme_catalogue;
mod tracker;
mod working_state;

pub use theme::apply_system_appearance;
pub use tracker::{IssueTracker, init};
