//! Who is allowed to talk to the API.
//!
//! On loopback a token protects nothing from a process running as you — that
//! process can read the database file directly. What it does defeat is the one
//! threat that cannot: a web page. A malicious site can make your browser
//! issue requests to `127.0.0.1`, but it cannot read `api.json` to learn the
//! token, cannot set an `Origin` header of its choosing, and cannot make the
//! `Host` header lie convincingly. All three checks below exist for that one
//! attacker. See `docs/adr/0006`.

use super::http::{Request, Response};

pub struct Guard {
    token: String,
}

impl Guard {
    pub fn new(token: String) -> Self {
        Self { token }
    }

    pub fn check(&self, request: &Request) -> Result<(), Response> {
        // Only a browser sends `Origin`. Nothing that legitimately talks to
        // this API has one, so its mere presence is disqualifying.
        if request.header("origin").is_some() {
            return Err(Response::error(
                403,
                "this API does not serve browser requests",
            ));
        }

        // Defeats DNS rebinding: an attacker who points their own hostname at
        // 127.0.0.1 still sends that hostname here.
        match request.header("host") {
            Some(host) if is_loopback(host) => {}
            _ => {
                return Err(Response::error(
                    403,
                    "requests must be addressed to localhost",
                ));
            }
        }

        let presented = request
            .header("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or("");
        if !constant_time_eq(presented.as_bytes(), self.token.as_bytes()) {
            return Err(Response::error(401, "a valid bearer token is required")
                .with_header("WWW-Authenticate", "Bearer"));
        }

        Ok(())
    }
}

/// Accepts the loopback spellings, with or without a port.
fn is_loopback(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        // `[::1]:8787` — an IPv6 literal keeps its colons inside the brackets.
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => host.rsplit_once(':').map_or(host, |(name, _)| name),
    };
    matches!(name, "127.0.0.1" | "localhost" | "::1")
}

/// Compares without leaking where the first difference is.
///
/// A timing attack over loopback is a stretch, but the whole comparison is
/// four lines and getting it wrong is the kind of thing that ages badly.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b)
        .fold(0u8, |difference, (x, y)| difference | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::super::http::{Parsed, parse};
    use super::*;

    fn request(raw: &str) -> Request {
        match parse(raw.as_bytes()) {
            Parsed::Complete(request) => request,
            other => panic!("expected a complete request, got {other:?}"),
        }
    }

    fn guard() -> Guard {
        Guard::new("s3cret".into())
    }

    fn with_token(raw_headers: &str) -> Request {
        request(&format!("GET /issues HTTP/1.1\r\n{raw_headers}\r\n"))
    }

    #[test]
    fn a_correct_token_from_localhost_is_admitted() {
        let request = with_token("Host: 127.0.0.1:8787\r\nAuthorization: Bearer s3cret\r\n");
        assert!(guard().check(&request).is_ok());
    }

    #[test]
    fn every_loopback_spelling_is_accepted() {
        for host in [
            "127.0.0.1",
            "127.0.0.1:8787",
            "localhost",
            "localhost:9",
            "[::1]:8787",
        ] {
            let request = with_token(&format!("Host: {host}\r\nAuthorization: Bearer s3cret\r\n"));
            assert!(guard().check(&request).is_ok(), "rejected {host}");
        }
    }

    #[test]
    fn a_wrong_or_missing_token_is_unauthorized() {
        let wrong = with_token("Host: 127.0.0.1\r\nAuthorization: Bearer nope\r\n");
        let response = guard().check(&wrong).unwrap_err();
        assert_eq!(response.status, 401);
        assert!(
            response
                .headers
                .iter()
                .any(|(name, _)| name == "WWW-Authenticate")
        );

        let missing = with_token("Host: 127.0.0.1\r\n");
        assert_eq!(guard().check(&missing).unwrap_err().status, 401);
    }

    #[test]
    fn a_token_of_the_wrong_length_is_rejected_too() {
        // Guards the constant-time comparison's length branch.
        let short = with_token("Host: 127.0.0.1\r\nAuthorization: Bearer s3cre\r\n");
        assert_eq!(guard().check(&short).unwrap_err().status, 401);
        let long = with_token("Host: 127.0.0.1\r\nAuthorization: Bearer s3cretx\r\n");
        assert_eq!(guard().check(&long).unwrap_err().status, 401);
    }

    #[test]
    fn a_browser_is_turned_away_even_with_the_right_token() {
        // The exact attack this API is defended against: a page that somehow
        // learned the token still announces itself with `Origin`.
        let request = with_token(
            "Host: 127.0.0.1\r\nOrigin: https://evil.example\r\nAuthorization: Bearer s3cret\r\n",
        );
        assert_eq!(guard().check(&request).unwrap_err().status, 403);
    }

    #[test]
    fn a_rebound_hostname_is_turned_away() {
        let request = with_token("Host: evil.example\r\nAuthorization: Bearer s3cret\r\n");
        assert_eq!(guard().check(&request).unwrap_err().status, 403);
    }

    #[test]
    fn a_missing_host_is_turned_away() {
        let request = with_token("Authorization: Bearer s3cret\r\n");
        assert_eq!(guard().check(&request).unwrap_err().status, 403);
    }
}
