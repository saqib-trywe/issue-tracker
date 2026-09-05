# The API answers what happened, not what is allowed

There is no endpoint for asking which Issues could become a Parent, or which
could be attached under one. `Projection` knows — the detail pane's pickers are
built from `eligible_sub_issues` and `eligible_parents` — but that knowledge
stays in the process. Every other surface finds out by making the change and
reading the refusal.

This was proposed and declined, which is the reason it is written down: the
duplication that prompted it was real and has been fixed, and a future reader
looking at those two methods will reasonably wonder why they are not on the
wire.

## Considered options

**`GET /issues?eligible_parent_for=7`**, or similar, was the proposal. It would
let the CLI offer a picker and let an agent check before acting rather than
discovering the rule through a 409.

It was rejected on four grounds. The API is deliberately domain-shaped — every
endpoint is an operation on Issues — and this would be the first that asks a
question *about the rules*, which is a different kind of thing and invites more
of them. The refusals already carry the answer in prose: an agent told
"#2 is itself a sub-issue, and sub-issues cannot have sub-issues" has learned
the rule as well as a list would have taught it. Any list is a snapshot, and
the gap between asking and acting means a client must handle the 409 anyway —
so eligibility adds a step without removing one. And the MCP server already
states the rules in its `instructions` at initialize, which teaches an agent
the same thing once instead of per call.

**Making the rules public but in-process** — leaving `can_be_done` `pub` —
was the weaker version. Nothing outside the projection used it, and a public
method with no caller implies an affordance that does not exist.

## Consequences

`Projection::may_attach` is now the single statement of the attachment rules,
private, returning the same `Refused` the writer returns. `set_parent`
enforces it and both `eligible_*` methods are defined as the Issues for which
it returns `Ok`, so the predicate form cannot drift from the refusal form. That
drift had already happened: `eligible_sub_issues` never checked whether the
proposed parent was itself a sub-issue, and was correct only because
`issue_detail` declines to render that picker for an Issue that is already a
part. The projection no longer depends on the view for that.

The rule is still stated in two places, and deliberately so: the projection
decides what is allowed, and `render_relationships` decides which control to
offer an Issue that has committed to a direction. The second is about layout.
What changed is that it is no longer load-bearing for correctness.

If a surface ever genuinely needs to offer a picker — a second GUI, or a CLI
that grew one — this is worth reopening. The cost then is one endpoint; the
cost of adding it now is a wider API answering a question nobody asked.
