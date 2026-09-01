# Issue Tracking

A single-user desktop issue tracker that owns its own data. There is no sync,
no network, and no second user — several conventions common to issue trackers
carry no information here and are deliberately absent.

## Language

**Issue**:
A single unit of tracked work. The only first-class record in the system.
_Avoid_: Ticket, Task, Card, Story

**Status**:
Where an Issue sits in its lifecycle. One of exactly five values.
_Avoid_: State, Stage, Column

**Blocked**:
An Issue that is still intended but cannot progress right now. Distinct from
Todo, which is merely not started.

**Cancelled**:
An Issue that was deliberately abandoned. The work is not going to happen, and
that decision is worth keeping.
_Avoid_: Rejected, Won't fix, Closed

**Priority**:
How much an Issue matters relative to others. Defaults to None.
_Avoid_: Severity, Importance, Rank

**View**:
A named, predefined slice of all Issues, shown in the sidebar. Currently a
Status filter. Not user-created.
_Avoid_: Filter, Query, Smart list, Folder

## Cancelling vs deleting

These are different acts and both exist:

- **Cancel** records a decision. The Issue was real, and the choice to abandon
  it is history worth keeping.
- **Delete** erases a mistake. The Issue should never have existed — a typo, a
  stray keystroke. Nothing is worth preserving.

## Deliberately absent

- **Assignee / Reporter / Watcher**: one user, no sync. Fields whose only job
  is to coordinate between people carry no information here.
- **Project**: no grouping concept above Issue.
- **Comment**: a single user talking to themselves. The Issue body serves
  instead.
- **Label**: not modelled yet.
