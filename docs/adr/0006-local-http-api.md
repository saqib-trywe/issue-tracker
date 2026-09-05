# A local HTTP API, on a blocking thread, behind a bearer token

The app serves an HTTP API on loopback for the length of the process, so
scripts and agents can do anything to an Issue that the window can. It is
hosted on a blocking `std::net::TcpListener` on its own thread, and
authenticated with a bearer token regenerated each launch.

## Scope

The API covers the domain and nothing else: create, read, patch and delete
Issues, and add or remove Tags. It deliberately does *not* expose view state —
the active View, the selection, the filters, the sidebar, the theme. Those
persist as `ui.*` and `theme.*` keys, but they are where you happen to be
standing, not facts about your work, and a remote control for a GUI is a
different product. The testable version of the goal is: **anything you can do
to an Issue in the UI, you can do over HTTP.**

Filtering is included, because narrowing a query by Status or Tag is a domain
read; `GET /issues?status=Todo&tag=bug&q=side` is the same three narrowings the
list offers.

## Considered options

**axum or hyper** was rejected. It would bring routing, extractors and JSON for
free, but GPUI has no I/O reactor of any kind — it schedules `async-task`
futures onto Grand Central Dispatch — and no tokio runtime runs in this process.
Using axum means standing up a second multi-threaded runtime alongside GCD and
bridging at every call boundary, to serve six endpoints on a socket that will
see one connection at a time. Zed faced this exact decision for its own
in-process HTTP server (`crates/http_proxy`) and also chose a blocking listener
with `httparse`; that crate depends on neither tokio nor smol nor gpui.

**`async-io` on GPUI's `BackgroundExecutor`** was the middle option and was
rejected for buying little: it still rules out axum, still needs hand-rolled
HTTP, and adds a reactor thread anyway.

**SSH-key authentication** was the original intent and was rejected in favour
of a token. On loopback the two are equal in strength — any process running as
you can read `issues.db` directly, and no credential changes that. The one
attacker a credential genuinely defeats is a web page, which can make your
browser issue requests to `127.0.0.1` but cannot read a `0600` file. A token
defeats that in thirty lines; keys would buy named, individually revocable
clients at the cost of every client hand-implementing request signing, since no
standard for SSH-key auth over HTTP exists. Keys become the right answer the
day this leaves the machine.

## Consequences

The socket threads decide nothing. Parsing, authentication and routing live in
`crate::api`, which depends on `domain` and `store` but **not** `gpui`, takes a
parsed request and `&mut Projection`, and returns a response value — so all of
it is covered by ordinary tests with no window and no socket. `ui::api_server`
only moves bytes and marshals jobs onto the main thread, where they are applied
one at a time through the same `Projection` the UI writes to.

Three guards, all aimed at the browser: a request carrying `Origin` is refused,
`Host` must be a loopback spelling (which defeats DNS rebinding), and the token
is compared in constant time. Connections are capped at 16, because
thread-per-connection with no ceiling is an unbounded thread spawn.

`PATCH` writes only the fields it names, and pending debounced UI edits are
flushed before any API write. Together those mean a request setting a Status
cannot clobber a title being typed, and a keystroke landing 400ms later cannot
silently undo the request. `updated_at` is in every response so `If-Match` can
be added later without changing a shape clients already parse.

The port and token are published to `api.json` beside the database, mode
`0600`, written only once the listener is bound and removed on clean shutdown —
so its presence means the app is up. A crash leaves it behind, and a client
will then find a port that refuses connections. The token changes every launch,
so clients must read the file rather than hardcoding a secret.

If the port cannot be bound the app runs normally without an API, because the
API is a convenience and losing it should never cost you the app you opened.
