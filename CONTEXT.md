# Issue Tracking

A single-user desktop issue tracker that owns its own data. There is no sync
and no second user — several conventions common to issue trackers carry no
information here and are deliberately absent.

The tracker can be driven from outside the window as well as through it, by
scripts and agents acting on your behalf. That is still one user: they are
alternative ways into the same person's issues, not two participants, and
nothing in the language below exists to tell them apart.

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

**Tag**:
A name the user invents and attaches to an Issue. A Tag exists exactly as long
as some Issue carries it: there is no list of Tags apart from the Issues
wearing them, and the last Issue to let go of one takes it with it.
Case-insensitive, so `Bug` and `bug` are one Tag. Flat — `ui/theme` is a name
that happens to contain a slash, not a child of `ui`.
_Avoid_: Label, Category, Topic, Keyword

**Sub-issue**:
An Issue that is part of another Issue. Exactly one level deep: a Sub-issue has
no Sub-issues of its own, and belongs to at most one Parent. Being a Sub-issue
changes nothing else about it — it keeps its own Status, Priority and Tags, and
appears in the list like anything else.
_Avoid_: Subtask, Child issue, Checklist item

**Parent**:
The Issue a Sub-issue is part of. Not a new kind of record: a Parent is an
ordinary Issue that happens to hold others.
_Avoid_: Epic, Story, Container

**Tag filter**:
Narrowing the list to a single Tag. Distinct from a View: a View is predefined
and a Tag filter is whatever the user invented, and the two compose — a Tag
filter narrows *within* whichever View is active.

**Working state**:
What a window is currently looking at: the active View, the Tag filter and
title filter narrowing it, and which Issue is selected. It belongs to the
window rather than to the Issues — nothing about an Issue changes because you
looked at it — and it outlives the window, so reopening the app puts you back
where you were. A selection that no longer resolves, or a Tag filter whose Tag
no longer exists, is dropped rather than restored.
_Avoid_: Session, Context, Mode

## Cancelling vs deleting

These are different acts and both exist:

- **Cancel** records a decision. The Issue was real, and the choice to abandon
  it is history worth keeping.
- **Delete** erases a mistake. The Issue should never have existed — a typo, a
  stray keystroke. Nothing is worth preserving.

## Completing a Parent

A Parent is Done only once nothing beneath it is outstanding. Done and
Cancelled both **settle** a Sub-issue: one finished, the other was deliberately
abandoned, and neither is waiting on anybody.

This holds in both directions — a Sub-issue cannot be reopened while its Parent
is Done — so the state "a Parent finished above unfinished work" simply does
not occur. Cancelling a Parent is never blocked: abandoning a piece of work
does not require tidying its parts first.

Deleting a Parent releases its Sub-issues, which survive as ordinary Issues. A
Sub-issue may be moved to another Parent, or removed from one entirely.

## Deliberately absent

- **Assignee / Reporter / Watcher**: one user, no sync. Fields whose only job
  is to coordinate between people carry no information here.
- **Project**: no *kind* of record above Issue. Sub-issues group Issues under
  an Issue, but a Parent is an ordinary Issue — there is still nothing of a
  different sort sitting above one.
- **Nesting beyond one level**: a Sub-issue cannot have Sub-issues.
- **Comment**: a single user talking to themselves. The Issue body serves
  instead.
- **Renaming a Tag**, or removing one from every Issue at once. A misspelled
  Tag is corrected by retagging the Issues that carry it. There is no registry
  to rename it in, because a Tag is not a record.
