//! The local HTTP API.
//!
//! Everything in this module is free of `gpui`: it depends on `domain` and
//! `store` only, takes a parsed request plus `&mut Projection`, and returns a
//! response value. That is what lets routing, auth, filtering and patch
//! application be tested with ordinary `cargo test` — no window, no socket.
//! The thread that owns the socket lives in `ui::api_server`, and does nothing
//! but move bytes. See `docs/adr/0006`.

mod auth;
mod http;
mod routes;
pub mod wire;

pub use http::{Parsed, Request, Response, parse};

use crate::projection::Projection;

/// Everything needed to answer a request.
pub struct Api {
    guard: auth::Guard,
}

impl Api {
    pub fn new(token: String) -> Self {
        Self {
            guard: auth::Guard::new(token),
        }
    }

    /// Authenticates, then routes. The only entry point.
    pub fn handle(&self, request: &Request, projection: &mut Projection) -> Response {
        if let Err(refusal) = self.guard.check(request) {
            return refusal;
        }
        routes::dispatch(request, projection)
    }
}
