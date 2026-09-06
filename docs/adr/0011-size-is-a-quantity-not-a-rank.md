# Size is a quantity, not a rank

An Issue's `size` is `Option<u8>` — a number — where `Status` and `Priority`
are ordinal enums. It is the only scalar on an Issue that is not a fixed list
of labels, which is the reason this is written down.

The requirement forced it. A Parent's Total size is its own Size plus its
parts', and ordinals do not add: `S + S` is not `M`, and there is no answer to
what `XL + XS` comes to. Anything that adds has to be a quantity.

## Considered options

**An ordinal enum**, `XS/S/M/L/XL`, matching `Priority` exactly. Rejected
because it cannot satisfy the rollup, which was the point of the feature.

**An enum with numeric weights** — `S = 2`, `M = 3`, `L = 5` — so leaves are
chosen from a list and totals are numbers. Rejected because the field would
then mean different things at different levels: a leaf's Size is one of five
labels and a Parent's total is `13`, which is not one of them, so it can no
longer be parsed or rendered uniformly. It also imports a scale, and a scale
implies velocity and reports, which this tracker deliberately does not have.

**Max-of-children instead of a sum.** Rejected: it answers "how big is the
biggest part", which nobody asked, and makes a Parent's own Size meaningless.

**A Parent's own Size being the whole job**, with its parts as a breakdown.
Rejected because then the two must not be added, and every reader who sees both
numbers assumes they add. Making the Parent's Size mean *the work its parts do
not cover* is what makes the arithmetic honest.

## Consequences

`u8` is the constraint rather than a check beside it, and it states a rule in
the signature: 255 is where an Issue should have become a tree of Issues. A
total is `u32`, because the ceiling applies to what a person types, not to what
the arithmetic produces — so a Size and a Total size are different types as
well as different concepts, and neither can be mistaken for the other.

Absent and zero are different, so `size` is `Option<u8>` and clearing one needs
a third state. `IssuePatch::size` is `Option<Option<Size>>` and `wire::PatchIssue`
distinguishes an absent key from an explicit `null` — the trap `parent_id` sits
beside, met head-on this time because a Size genuinely needs removing. On the
command line that is `--size none`, matching `--parent none`.

Total size is derived on every read and never stored, like the settled count
and like Tags. A stored total is a second copy of something the Issues already
say, and it goes wrong the moment a part is moved, deleted or resized.

`sort_for_display` is untouched. Size is not pressingness, and that function
decides what "the first one" means in the window, the API, the CLI and the MCP
server at once.

There is no remaining-size, though it would be useful and is cheap to add later
since nothing is stored. `settled_sub_issues` already answers "how far along",
and two different answers to one question is worse than one.
