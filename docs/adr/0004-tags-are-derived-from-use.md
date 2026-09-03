# Tags are derived from use, not records

A Tag is the set of names currently written on Issues, not a thing that exists
in its own right. There is an `issue_tag` junction table and deliberately no
`tag` table: a Tag comes into being when an Issue is first given it and ceases
to exist when the last Issue lets it go.

This keeps the claim at the top of `CONTEXT.md` true — the Issue remains the
only first-class record — and it removes an entire category of work that a
single-user tracker does not earn: no registry to seed, no orphan Tags to
garbage-collect, no management screen, and no decision about what deleting a
Tag does to the Issues wearing it. Tag colours are derived from the name
instead (`src/ui/tag_colour.rs`), which is arbitrary but stable and needs no
administration.

## Considered options

**First-class Tag records** were rejected. They would allow a *chosen* colour,
a description, and a rename in one place, but every one of those benefits
arrives with a registry to maintain and a management UI to build. The pattern
this project has followed since the beginning is to refuse machinery whose
payoff depends on coordination between people, and a Tag registry is that.

**A delimited `tags` column** on `issue` was rejected in favour of the junction
table. It is about a third of the code and makes orphans impossible by
construction, but Tag names may contain spaces, so it needs a delimiter rule
plus input validation to enforce it. The junction table needs no such rule and
leaves the data answerable to plain SQL in a `sqlite3` session, which matters
for a database you own and will poke at by hand.

## Consequences

`Store::update` is now transactional: the `issue` row and its `issue_tag` rows
must move together, or a crash between them leaves an Issue wearing the Tags of
its previous revision. This is the ADR-0002 write-through discipline with a
second way to trip it.

Case-insensitive identity cannot be enforced by the schema. A unique index on
`issue_tag` is per-Issue, so `Bug` on one Issue and `bug` on another is legal
SQL that would render as two sidebar rows. The invariant is held in the domain
instead — `Tag`'s comparison traits are written over its folded key, and
`normalise_tags` folds new names against those already in use.

There is no rename and no remove-from-every-Issue. Correcting a misspelled Tag
means retagging each Issue that carries it. Autocomplete in the Tag editor is
what keeps that rare; if it stops being rare, this decision is what to revisit.
