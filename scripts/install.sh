#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Builds and installs everything this repository ships, to ~/.cargo/bin.
#
# There are two installs rather than one, and that is the whole reason this
# script exists. `cargo install --path .` installs the root package's binaries
# — `Issues` and `issue` — and stops there, because `issue-mcp` is a *workspace
# member* rather than a third `[[bin]]` (docs/adr/0009). Installing the app and
# wondering why an agent cannot reach the tracker is the failure this prevents:
# the MCP client looks for `issue-mcp` on PATH by name and reports only that it
# could not spawn it.

set -euo pipefail

# Run from the repository root whatever directory this was invoked from, so
# `--path .` means what it says.
cd "$(dirname "${BASH_SOURCE[0]}")/.."

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }

# `--locked` because `gpui` and `gpui-component` are git dependencies: without
# it a fresh resolve can install against a different upstream commit from the
# one the tests just passed on, which is a difference you find out about at
# runtime.
#
# `--force` so that re-running this replaces the binaries rather than declining
# as already-installed. Iterating is the normal case.
install_from() {
    cargo install --path "$1" --locked --force
}

say "Installing Issues and issue (the app and the CLI)"
install_from .

say "Installing issue-mcp (separate: it is a workspace member, not a third binary)"
install_from crates/issue-mcp

# Report what actually landed, by resolving each name the way the things that
# call them do. A successful `cargo install` followed by a binary that is not
# on PATH is the other half of the same confusion.
say "Installed"
missing=0
for binary in Issues issue issue-mcp; do
    if path="$(command -v "$binary" 2>/dev/null)"; then
        printf '  %-10s %s\n' "$binary" "$path"
    else
        printf '  %-10s NOT ON PATH\n' "$binary"
        missing=1
    fi
done

if [ "$missing" -ne 0 ]; then
    cat >&2 <<'HINT'

Something installed but is not resolvable by name. `cargo install` writes to
~/.cargo/bin; add it to PATH:

    export PATH="$HOME/.cargo/bin:$PATH"

HINT
    exit 1
fi

cat <<'NEXT'

  Issues     the app. Its window is the tracker; it also hosts the local API.
  issue      the CLI. A client of that API, so it needs the app running and
             exits 3 when it is not.
  issue-mcp  the MCP server. Also a client of the API. It speaks JSON-RPC on
             stdin/stdout, so running it in a terminal looks like a hang —
             that is it waiting for a client. Configured in .mcp.json by bare
             name, which is why it has to be on PATH rather than anywhere.

Restart any MCP client that was already running: it resolved issue-mcp at
startup and will not look again.
NEXT
