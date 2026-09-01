# Synchronous rusqlite with a full in-memory projection

`IssueTracker` loads every Issue into a `Vec<Issue>` at startup and renders
only from that. Mutations write through to SQLite first, then update the
in-memory copy. Queries use `rusqlite` synchronously.

This exists because GPUI's `render()` runs on the UI thread every frame and
must never block on I/O. Reading from an in-memory projection makes that
guarantee structural rather than something we have to remember. A personal
tracker will not exceed a few thousand Issues, so the whole dataset is well
under a megabyte.

## Considered options

`sqlx` was rejected because it drags a tokio runtime in alongside GPUI's own
executor and its compile-time-checked macros want a live database at build
time. `diesel` was rejected as heavy ORM machinery for a single table.
Query-on-demand was rejected because it puts I/O on the render path, which is
the specific thing this design is avoiding.

## Consequences

Every mutation path must update both SQLite and the in-memory `Vec`, and a
bug that updates only one of them shows up as a change that vanishes on
restart. Anything that grows the dataset by orders of magnitude — bulk import,
sync, or attachments — reopens this decision.
