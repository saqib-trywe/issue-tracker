# Vendored themes

These 21 JSON files are a point-in-time copy of the `themes/` directory in
[gpui-component](https://github.com/longbridge/gpui-component), taken at commit
`a8d1d26`. They are embedded into the binary at compile time (see
`src/ui/theme_catalogue.rs`), because the originals live in a Cargo git
checkout that is a build artifact rather than something we can rely on at
runtime.

Each file names its own author and upstream URL; leave that metadata intact.

Upstream may have added, changed, or removed themes since this snapshot. To
refresh, re-copy from the checkout under `~/.cargo/git/checkouts/`.

Note that the catalogue is uneven: only 10 of the 21 families ship both a light
and a dark variant, ten are dark-only, and `aurora` is light-only. That is why
the app lets you choose a light theme and a dark theme independently rather
than picking a single "family".
