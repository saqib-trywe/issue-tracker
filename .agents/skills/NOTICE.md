# Third-party skills

Everything under this directory is vendored from
[`mattpocock/skills`](https://github.com/mattpocock/skills) and is **not**
covered by this repository's licence. It is MIT, `Copyright (c) 2026 Matt
Pocock`; the full text is in [LICENSE](LICENSE) beside this file.

The rest of the repository is GPL-3.0-only (see `COPYING` at the root). The two
do not conflict — MIT is compatible with the GPL, and in any case nothing here
is linked into the binaries. These are instructions read by coding agents, not
code that ships.

`.claude/skills/` holds symlinks into this directory rather than a second copy,
so there is one set of files and one place this notice has to be true.

Per-skill provenance — which upstream path each came from, and the hash it was
vendored at — is recorded in `skills-lock.json` at the repository root. That
file is how the skills tool tracks them; this file is the attribution MIT asks
for, which a lock file does not provide.
