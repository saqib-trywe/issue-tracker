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

- **Module seam**: `domain` holds pure types and must not depend on `gpui` or `rusqlite`. `store` may depend on `domain`. `projection` and `api` may depend on both but **not** on `gpui` — that is what makes routing, auth and every mutation testable without a window or a socket. Only `ui` may reach for `gpui`. Don't erode this: `Entity<T>` needs `T: 'static`, not a `T` that knows about GPUI, so shared state can stay gpui-free and still be observable.
- **In-memory projection**: `Projection` (`src/projection.rs`) owns the `Store` and every Issue in a `Vec<Issue>`. `render()` reads only from it and never performs I/O. Mutations write through to SQLite *first*, then update the Vec — updating only one of the two is the characteristic bug here, and it shows up as a change that vanishes on restart. See `docs/adr/0002`.
- **The projection is app-level, not window-level** (`docs/adr/0005`): it lives in a GPUI global (`src/app_state.rs`) so it outlives the window and the HTTP API can reach it. `IssueTracker` borrows it, renders from it, and `observe_in`s it. Reads therefore take `&App` — that is why `visible_issues`, `selected_issue` and `count_for*` all carry a `cx`, and why `issue_list` collects owned `Row` values before building elements: the borrow must end before the element tree needs `cx` mutably.
- **One settle path**: every mutation ends in `cx.notify()` on the Projection, which runs `IssueTracker::on_projection_changed` — prune the Tag filter, re-settle the selection, refresh the inputs. There is deliberately no separate branch for API-initiated changes. If you add a mutation, route it through `Projection` and let the observer do the rest rather than re-settling by hand.
- **Field-level patches**: `IssuePatch` names only the fields it changes, so two writers touching different fields can't clobber each other. A patch that changes nothing writes nothing, so `updated_at` doesn't move for a no-op.
- **Auto-save**: editing is debounced by storing the spawned `Task<()>` on the view and replacing it on each `InputEvent::Change`; dropping the old task cancels it. `InputState::set_value` deliberately suppresses change events, so loading an Issue into the inputs does not trigger a save.
- **Theme**: `gpui_component::init` pins the theme to Light unconditionally, so `src/ui/theme.rs` re-syncs it to the OS appearance — once inside the `open_window` closure before the first paint, then continuously via `observe_window_appearance`. Never hardcode a colour; every colour must come from `cx.theme()` or it won't follow the system into dark mode.
- **Named themes**: 21 theme files are vendored into `assets/themes/` and embedded with `rust-embed`; `src/ui/theme_catalogue.rs` parses them into per-mode lists. Light and dark are chosen *independently* because over half the upstream families ship only one mode — see `assets/themes/README.md`. Choices persist in the `setting` table (`docs/adr/0003`) and are applied at startup in `IssueTracker::new`. Selecting a theme never changes *when* dark applies; that still follows the OS.
- **Local HTTP API** (`src/api/`, `src/ui/api_server.rs`, `docs/adr/0006`): domain operations only — no view state. Hosted on a blocking `TcpListener` thread, one thread per connection, marshalled onto the main thread; there is no tokio runtime in this process and GPUI's executor has no I/O reactor, so don't reach for axum. `src/api/` is gpui-free and holds all the decisions; `api_server.rs` only moves bytes. A Tag name is the whole path remainder (`/issues/3/tags/ui/theme`), which is why routing is hand-written. Port and token are published to `api.json` beside the database, `0600`, token regenerated every launch — clients must read the file, not hardcode it.
- **Running the app while testing**: kill old instances first (`pkill -f "target/debug/Issues"` — keep it path-qualified, since bare `Issues` matches far too much). A stale instance keeps its window on top and will silently be the thing you screenshot, which looks exactly like a bug in the code you just changed. Also run `cargo build`, not just `cargo check`, before launching the binary.
- **Focus and keys**: navigation bindings (`j`/`k`/`c`/`e`/`x`/`/`) are scoped to the `IssueList` key context on a focusable list container, so typing into an input types text rather than navigating. The list is focused at startup; without that the bindings are dead until something is clicked. `escape` and `cmd-b` are deliberately *unscoped* so they work while an input has focus.
- **macOS lifecycle**: the app follows macOS convention — closing the window does *not* quit (`QuitMode::Default` is `Explicit` on macOS). `main.rs` extracts `open_main_window` so `Application::on_reopen` can rebuild a window when the Dock icon is clicked. `cmd-q` flushes pending writes before quitting; so does `cx.on_release` when the window closes. Both matter because issue auto-save is debounced 400ms and would otherwise lose the last edit.
- **Menus** (`src/ui/menus.rs`): menu items dispatch actions, so anything in a menu needs a handler on `IssueTracker`. The View menu carries a checkmark on the active View, which means `menus::rebuild` must be called whenever the View changes — `set_menus` replaces the whole tree. The application menu's title comes from the **process name**, not from the first `Menu`'s name — which is why `Cargo.toml` declares `[[bin]] name = "Issues"`. Renaming that binary renames the menu.
- **Tags**: derived, not records — a Tag exists exactly as long as some Issue carries it, so there is an `issue_tag` junction table and no `tag` table (`docs/adr/0004`). Two consequences bite. `Store::update` is transactional because the Issue row and its Tag rows must move together — the ADR-0002 half-written-change bug, with a second way to reach it. And case-insensitive identity (`Bug` *is* `bug`) cannot be a schema constraint, since a unique index on `issue_tag` is per-Issue; it lives in `domain::Tag`, whose `Eq`/`Ord`/`Hash` are hand-written over the folded key rather than derived. Route new names through `normalise_tags` or case variants will appear as separate sidebar rows.
- **Tag filter vs View**: a Tag filter is *not* a `View` variant. Views are the five predefined Status slices; a Tag filter is user-invented and narrows within whichever View is active. Because a derived Tag can vanish when its last Issue is retagged or deleted, `prune_tag_filter` runs on both paths — otherwise the list empties with the sidebar row you'd clear it from already gone.
- **Working state**: View, selection and filter persist as `ui.*` keys and are restored on launch, debounced ~400ms because they change on every keypress. A stored selection that no longer resolves falls back to the first visible issue.
- **Sidebar toggle**: `cmd-b` or the `SidebarToggleButton` in the issue-list header drops the sidebar out of the element tree entirely (rendered as `Option`, not zero width), persisted as `sidebar.hidden`. The toggle button lives in the list header rather than the sidebar so it survives hiding. Because the sidebar owns both View navigation and the theme pickers, the header always shows the active View name — with the sidebar hidden it is the only thing naming it.
- GPUI apps are built from `Render`-implementing structs whose `render()` returns an element tree built with the `div()` builder API (flex-based layout, Tailwind-like modifiers such as `.size_full()`, `.flex_row()`, `.w(px(..))`).
- Returning elements from a helper that takes `&mut Context<Self>` usually needs `-> impl IntoElement + use<>` under Rust 2024, otherwise the return type captures the context lifetime and the borrow checker rejects the caller.
- `gpui_component::init(cx)` must be called once at startup before creating any windows — it registers the component library's global state/theme.
- The root view of a window must be wrapped in `gpui_component::Root::new(view, window, cx)`; `cx.open_window` takes a closure that constructs this root.
- `gpui-platform` (with the `font-kit` feature) provides the platform application entry point (`gpui_platform::application()`), separate from the core `gpui` crate.
- Both `gpui` and `gpui-component` are pulled directly from their git repos (not crates.io), so APIs may move; check the vendored source under `~/.cargo/git/checkouts/` (or run `cargo doc --open -p gpui -p gpui-component`) when an API isn't obvious from usage.
