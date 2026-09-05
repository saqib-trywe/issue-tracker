// SPDX-License-Identifier: GPL-3.0-only

//! The `issue` command. Everything is in `issue_tracker::cli`, so the same
//! code can be driven from an integration test without spawning a process.

use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::from(issue_tracker::cli::run(
        std::env::args_os().skip(1).collect(),
    ))
}
