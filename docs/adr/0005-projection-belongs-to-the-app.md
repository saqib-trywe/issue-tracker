# The issue projection belongs to the app, not the window

`Projection` — the `Store` plus every Issue in memory — lives in an app-level
`Entity<Projection>` held in a GPUI global. Windows borrow it. `IssueTracker`
renders from it and observes it, but does not own it.

It used to be a field on `IssueTracker`, constructed inside `open_main_window`.
That was fine while the window was the only way in. It stopped being fine when
the HTTP API arrived, for two reasons. The API has to answer whether or not a
window is open, and on macOS this app deliberately outlives its last window —
so window-owned data meant no data at all much of the time. And two owners of
the same database would each hold a private `Vec<Issue>` that the other's
writes would silently invalidate, which is exactly the failure ADR-0002 warns
about, reached by a new route.

## Considered options

**A `Mutex` around a shared `Store` and `Vec`** was rejected. It works, but it
discards GPUI's observation machinery — so every writer would have to remember
to notify every window — and it puts a lock on the render path, which is the
thing ADR-0002 exists to keep clear.

**Leaving ownership on the window** and giving the API its own connection was
rejected outright: two in-memory copies of one database is the bug this project
already decided it did not want.

**A separate daemon process** owning the data, with the UI as a client, was
rejected as a different application. It would make parity structural, but it
puts network I/O behind `render()` and is a rewrite rather than a change.

## Consequences

`IssueTracker` no longer holds the Issues it draws. Reads take `&App` and
borrow from the shared entity, which is why `visible_issues`, `selected_issue`
and the `count_for*` helpers all grew a `cx` parameter, and why the list
collects owned `Row` values before building elements — the borrow has to end
before the element tree needs `cx` mutably.

Every mutation, from the UI or the API, goes through `Projection` and ends with
`cx.notify()`. That drives `IssueTracker::on_projection_changed`, which prunes
a vanished Tag filter, re-settles the selection and refreshes the inputs. There
is deliberately no separate path for "the API changed something": a window
re-settles the same way whoever wrote.

Because a window is no longer the thing that opens the database, failing to
open it is now fatal at startup rather than merely window-less. `main` exits
non-zero, which beats a running process with no window and no explanation.
