# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`gpui-issue-tracker-ui` — a single-user desktop issue tracker built with [GPUI](https://github.com/zed-industries/zed) (Zed's UI framework) and the [gpui-component](https://github.com/longbridge/gpui-component) widget library. It owns its data in a local SQLite database; it is not a client for GitHub or GitLab.

Read `CONTEXT.md` for the domain vocabulary and `docs/adr/` for the decisions that shaped the architecture.

## Commands

- Build: `cargo build`
- Run: `cargo run`
- Check (fast type-check without codegen): `cargo check`
- Test: `cargo test`, single test with `cargo test <test_name>`
- Format: `cargo fmt`
- Lint: `cargo clippy`

Set `ISSUE_TRACKER_DB` to point at a scratch database when developing, so experiments never touch real issues:

```
ISSUE_TRACKER_DB=/tmp/it-scratch.db cargo run
```

Without it, the database lives in the platform data directory (`~/Library/Application Support/gpui-issue-tracker/issues.db` on macOS).

## Architecture

- **Module seam**: `domain` holds pure types and must not depend on `gpui` or `rusqlite`. `store` may depend on `domain`; `ui` may depend on both. This is what keeps a future crate split mechanical — don't reach for `gpui` types inside `domain`.
- **In-memory projection**: `IssueTracker` (`src/ui/tracker.rs`) owns the `Store` and holds every Issue in a `Vec<Issue>`. `render()` reads only from that Vec and never performs I/O. Mutations write through to SQLite *first*, then update the Vec — updating only one of the two is the characteristic bug here, and it shows up as a change that vanishes on restart. See `docs/adr/0002`.
- **Auto-save**: editing is debounced by storing the spawned `Task<()>` on the view and replacing it on each `InputEvent::Change`; dropping the old task cancels it. `InputState::set_value` deliberately suppresses change events, so loading an Issue into the inputs does not trigger a save.
- **Focus and keys**: navigation bindings (`j`/`k`/`c`/`e`/`x`/`/`) are scoped to the `IssueList` key context on a focusable list container, so typing into an input types text rather than navigating. The list is focused at startup; without that the bindings are dead until something is clicked.
- GPUI apps are built from `Render`-implementing structs whose `render()` returns an element tree built with the `div()` builder API (flex-based layout, Tailwind-like modifiers such as `.size_full()`, `.flex_row()`, `.w(px(..))`).
- Returning elements from a helper that takes `&mut Context<Self>` usually needs `-> impl IntoElement + use<>` under Rust 2024, otherwise the return type captures the context lifetime and the borrow checker rejects the caller.
- `gpui_component::init(cx)` must be called once at startup before creating any windows — it registers the component library's global state/theme.
- The root view of a window must be wrapped in `gpui_component::Root::new(view, window, cx)`; `cx.open_window` takes a closure that constructs this root.
- `gpui-platform` (with the `font-kit` feature) provides the platform application entry point (`gpui_platform::application()`), separate from the core `gpui` crate.
- Both `gpui` and `gpui-component` are pulled directly from their git repos (not crates.io), so APIs may move; check the vendored source under `~/.cargo/git/checkouts/` (or run `cargo doc --open -p gpui -p gpui-component`) when an API isn't obvious from usage.
