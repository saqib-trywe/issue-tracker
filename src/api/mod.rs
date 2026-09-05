// SPDX-License-Identifier: GPL-3.0-only

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn projection() -> Projection {
        Projection::load(Store::open_in_memory().expect("store")).expect("projection")
    }

    fn request(head: &str, body: &str) -> Request {
        let raw = format!("{head}\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        match parse(raw.as_bytes()) {
            Parsed::Complete(request) => request,
            other => panic!("expected a complete request, got {other:?}"),
        }
    }

    const CREATE: &str = "POST /issues HTTP/1.1\r\nHost: 127.0.0.1";
    const FILED: &str = r#"{"title":"filed by a stranger"}"#;

    /// The composition *is* this module, and it had no test: `routes` is
    /// exercised through `dispatch` and `auth` through `Guard::check`, so
    /// nothing asserted that the guard runs first.
    #[test]
    fn a_request_without_a_token_is_turned_away_before_it_is_routed() {
        let mut p = projection();
        let api = Api::new("s3cret".into());

        let response = api.handle(&request(CREATE, FILED), &mut p);

        assert_eq!(response.status, 401);
        assert!(
            p.issues().is_empty(),
            "an unauthenticated write must not reach the projection"
        );
    }

    #[test]
    fn a_request_with_the_token_is_routed() {
        let mut p = projection();
        let api = Api::new("s3cret".into());
        let head = format!("{CREATE}\r\nAuthorization: Bearer s3cret");

        let response = api.handle(&request(&head, FILED), &mut p);

        assert_eq!(response.status, 201);
        assert_eq!(p.issues().len(), 1);
        assert_eq!(p.issues()[0].title, "filed by a stranger");
    }

    #[test]
    fn a_browser_is_turned_away_even_holding_the_token() {
        let mut p = projection();
        let api = Api::new("s3cret".into());
        let head =
            format!("{CREATE}\r\nAuthorization: Bearer s3cret\r\nOrigin: https://evil.example");

        let response = api.handle(&request(&head, FILED), &mut p);

        assert_eq!(response.status, 403);
        assert!(p.issues().is_empty());
    }
}
