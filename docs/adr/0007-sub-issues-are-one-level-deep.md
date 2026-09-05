# Sub-issues are one level deep, and a parent waits for its parts

An Issue may hold other Issues as sub-issues. The hierarchy is a tree with a
single parent per Issue and **exactly one level**: a sub-issue never has
sub-issues of its own. A parent may only be marked Done once nothing beneath it
is outstanding, and deleting a parent releases its sub-issues rather than
erasing them.

## Why one level

Arbitrary depth is where this feature stops being cheap. It brings indentation,
collapse state that has to be owned and persisted, recursive counts, subtree
queries, and a rewrite of the list rendering — `render_issue_list` is a
hand-rolled `div` of two-line rows, and both `gpui-component`'s `Tree` and its
`List` are virtualized and want uniform row heights, so nesting means replacing
that wholesale rather than adding an indent.

One level buys the thing actually wanted — several parts under a piece of work
— and defers all of it. The cap is enforced from both ends in `Projection`: the
proposed parent must not itself be a sub-issue, and the Issue being attached
must not already have parts. That also makes cycles unreachable without a graph
walk, since a cycle needs a chain of two.

## Why the completion rule is an invariant, not a nudge

Most trackers let a parent close over open children and show a warning. Here it
is refused, and refused symmetrically: you cannot mark a parent Done while work
is outstanding beneath it, *and* you cannot reopen a sub-issue whose parent is
Done. Only guarding the forward transition would make the rule true at the
instant you press the button and false a moment later, which is not a rule.

The predicate is "nothing outstanding", where **Done and Cancelled both
settle** an Issue. Requiring literal Done would make a Cancelled sub-issue block
its parent forever, and the only escape would be deleting a record `CONTEXT.md`
says is worth keeping. Cancelling a *parent* is never blocked — abandoning a
piece of work should not require first tidying its parts.

The rejected alternative was to reopen the parent automatically when a child
reopens. It is friendlier, but there is no defensible answer to which Status the
parent should then take — Todo? Doing? Whatever it was before? — and nothing
else in this app changes a Status the user did not set.

## Why deleting orphans rather than cascades

`CONTEXT.md` is careful that Delete erases a mistake while Cancel records a
decision. That is a statement about *that* Issue, not about everything attached
to it. Erasing work nobody asked to erase is the one unrecoverable action here,
so the foreign key is `ON DELETE SET NULL` and the children survive as ordinary
top-level Issues. The confirmation dialog says how many will be released.

## Consequences

The relation is a nullable `parent_id` column rather than a junction table,
because it is single-valued — `issue_tag` exists because Tags are many-to-many
and this is not. Orphaning is therefore the schema's doing rather than
something the code has to remember.

Mutations gained a typed refusal. `Projection` returns `WriteError`, which
separates *not found* (404) from *the data forbids this* (409) from *the
database failed* (500). Before this, every failure was an `anyhow::Error` and
would have become a 500.

Attaching an Issue that already has a parent **moves** it; there is no separate
operation, because `set_parent` already says everything a move needs to say.
Since that can quietly empty another Issue, the picker annotates candidates
with the parent they would be taken from.

On the wire, attach and detach are `PUT` and `DELETE` on
`/issues/{parent}/sub-issues/{child}`, mirroring Tags. `parent_id` is accepted
on `POST` — where absent unambiguously means "no parent", so ADR-0006's
one-call rule is honoured — but rejected with a 400 on `PATCH`, where our
`Option<T>` patch shape cannot tell an absent key from an explicit `null`.
Silently ignoring it would leave a caller believing they had moved something
they had not.
