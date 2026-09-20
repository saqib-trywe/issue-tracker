// SPDX-License-Identifier: GPL-3.0-only

//! A small HTTP/1.1 request reader and response writer.
//!
//! Deliberately hand-rolled and synchronous. The API has a handful of
//! endpoints on a loopback socket, which does not pay for an async runtime —
//! see `docs/adr/0006`. Parsing is a pure function over bytes so it can be
//! tested without a socket.

use std::fmt::Display;

use serde::Serialize;

/// Refuse bodies larger than this. Local or not, an unbounded read is an
/// invitation to exhaust memory.
pub const MAX_BODY: usize = 1 << 20;

/// The most headers we will look at. Anything beyond is a malformed request.
const MAX_HEADERS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// Percent-decoded, query stripped.
    pub path: String,
    query: Vec<(String, String)>,
    /// Names lowercased, values trimmed.
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// What [`parse`] made of the bytes so far.
#[derive(Debug)]
pub enum Parsed {
    Complete(Request),
    /// Headers or body are still arriving; read more and try again.
    Incomplete,
    Malformed(&'static str),
}

/// Reads a request out of whatever has arrived so far.
pub fn parse(buffer: &[u8]) -> Parsed {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut parser = httparse::Request::new(&mut headers);

    let header_len = match parser.parse(buffer) {
        Ok(httparse::Status::Complete(len)) => len,
        Ok(httparse::Status::Partial) => return Parsed::Incomplete,
        Err(_) => return Parsed::Malformed("could not parse the request"),
    };

    let headers: Vec<(String, String)> = parser
        .headers
        .iter()
        .map(|header| {
            (
                header.name.to_ascii_lowercase(),
                String::from_utf8_lossy(header.value).trim().to_string(),
            )
        })
        .collect();

    // Only `Content-Length` delimits a body here. Said rather than assumed:
    // a chunked body would otherwise be read as no body at all, and the
    // caller would be told their `title` was missing rather than that their
    // body never arrived.
    if headers.iter().any(|(key, _)| key == "transfer-encoding") {
        return Parsed::Malformed("chunked bodies are not supported; send Content-Length");
    }

    let content_length = match headers.iter().find(|(key, _)| key == "content-length") {
        Some((_, value)) => match value.parse::<usize>() {
            Ok(length) => length,
            Err(_) => return Parsed::Malformed("invalid Content-Length"),
        },
        None => 0,
    };
    if content_length > MAX_BODY {
        return Parsed::Malformed("request body is too large");
    }
    if buffer.len() < header_len + content_length {
        return Parsed::Incomplete;
    }

    let target = parser.path.unwrap_or("/");
    let (raw_path, raw_query) = match target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (target, None),
    };

    Parsed::Complete(Request {
        method: parser.method.unwrap_or_default().to_ascii_uppercase(),
        path: percent_decode(raw_path),
        query: raw_query.map(parse_query).unwrap_or_default(),
        headers,
        body: buffer[header_len..header_len + content_length].to_vec(),
    })
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (percent_decode_plus(key), percent_decode_plus(value)),
            None => (percent_decode_plus(pair), String::new()),
        })
        .collect()
}

/// Decodes `%XX` escapes. Invalid escapes are left as written rather than
/// rejected — a stray `%` in a Tag name should not be a protocol error.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Query strings additionally spell a space as `+`.
fn percent_decode_plus(input: &str) -> String {
    percent_decode(&input.replace('+', " "))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// The error shape every failure uses: one human-readable sentence.
#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
}

impl Response {
    pub fn empty(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn json<T: Serialize>(status: u16, value: &T) -> Self {
        match serde_json::to_vec(value) {
            Ok(body) => Self {
                status,
                headers: vec![("Content-Type".into(), "application/json".into())],
                body,
            },
            // Serialising our own types cannot realistically fail, but
            // panicking inside a request handler would take the whole app down.
            Err(err) => Self::error(500, format_args!("could not serialise the response: {err}")),
        }
    }

    pub fn error(status: u16, message: impl Display) -> Self {
        let message = message.to_string();
        let body = serde_json::to_vec(&ErrorBody { error: &message })
            .unwrap_or_else(|_| b"{\"error\":\"unknown\"}".to_vec());
        Self {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body,
        }
    }

    pub fn with_header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Serialises to the bytes that go on the wire.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status)).into_bytes();
        for (name, value) in &self.headers {
            out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        out.extend_from_slice(format!("Content-Length: {}\r\n", self.body.len()).as_bytes());
        // One request per connection: no keep-alive bookkeeping, and the
        // thread serving it can exit as soon as the body is flushed.
        out.extend_from_slice(b"Connection: close\r\n\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(raw: &str) -> Request {
        match parse(raw.as_bytes()) {
            Parsed::Complete(request) => request,
            other => panic!("expected a complete request, got {other:?}"),
        }
    }

    #[test]
    fn parses_a_bare_get() {
        let request = complete("GET /issues HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/issues");
        assert_eq!(request.header("host"), Some("127.0.0.1"));
        assert!(request.body.is_empty());
    }

    #[test]
    fn header_names_are_matched_without_regard_to_case() {
        let request = complete("GET /issues HTTP/1.1\r\nAuThOrIzAtIoN: Bearer x\r\n\r\n");
        assert_eq!(request.header("authorization"), Some("Bearer x"));
    }

    #[test]
    fn reads_a_body_of_the_declared_length() {
        let request = complete("POST /issues HTTP/1.1\r\nContent-Length: 9\r\n\r\n{\"a\":\"b\"}");
        assert_eq!(request.body, b"{\"a\":\"b\"}");
    }

    #[test]
    fn a_half_arrived_request_asks_for_more() {
        assert!(matches!(parse(b"GET /iss"), Parsed::Incomplete));
        // Headers complete, body still in flight.
        assert!(matches!(
            parse(b"POST /issues HTTP/1.1\r\nContent-Length: 20\r\n\r\nshort"),
            Parsed::Incomplete
        ));
    }

    #[test]
    fn an_oversized_body_is_refused_rather_than_buffered() {
        let raw = format!(
            "POST /issues HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert!(matches!(parse(raw.as_bytes()), Parsed::Malformed(_)));
    }

    /// Without this a chunked `POST` parses as a request with no body at all,
    /// and the caller is told their `title` is missing when what actually
    /// happened is that nothing here knows how to read what they sent.
    #[test]
    fn a_chunked_body_is_refused_in_terms_of_what_went_wrong() {
        let raw = "POST /issues HTTP/1.1\r\nHost: 127.0.0.1\r\n\
                   Transfer-Encoding: chunked\r\n\r\n1e\r\n{\"title\":\"filed by a stranger\"}\r\n0\r\n\r\n";
        match parse(raw.as_bytes()) {
            Parsed::Malformed(why) => assert!(why.contains("chunked"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn query_parameters_are_decoded() {
        let request =
            complete("GET /issues?status=Todo&q=two+words&tag=needs%20design HTTP/1.1\r\n\r\n");
        assert_eq!(request.path, "/issues");
        assert_eq!(request.param("status"), Some("Todo"));
        assert_eq!(request.param("q"), Some("two words"));
        assert_eq!(request.param("tag"), Some("needs design"));
        assert_eq!(request.param("absent"), None);
    }

    #[test]
    fn a_tag_path_keeps_its_slashes_and_decodes_its_spaces() {
        // The reason Tag names live in the path remainder: `ui/theme` needs
        // no encoding at all, and a space costs only %20.
        let request = complete("PUT /issues/3/tags/ui/theme HTTP/1.1\r\n\r\n");
        assert_eq!(request.path, "/issues/3/tags/ui/theme");

        let spaced = complete("PUT /issues/3/tags/needs%20design HTTP/1.1\r\n\r\n");
        assert_eq!(spaced.path, "/issues/3/tags/needs design");
    }

    #[test]
    fn a_stray_percent_is_left_alone_rather_than_rejected() {
        let request = complete("PUT /issues/3/tags/100%25 HTTP/1.1\r\n\r\n");
        assert_eq!(request.path, "/issues/3/tags/100%");
        let dangling = complete("PUT /issues/3/tags/50%zz HTTP/1.1\r\n\r\n");
        assert_eq!(dangling.path, "/issues/3/tags/50%zz");
    }

    #[test]
    fn responses_carry_a_length_and_close_the_connection() {
        let wire = String::from_utf8(Response::empty(204).to_bytes()).unwrap();
        assert!(wire.starts_with("HTTP/1.1 204 No Content\r\n"));
        assert!(wire.contains("Content-Length: 0\r\n"));
        assert!(wire.contains("Connection: close\r\n"));
    }

    #[test]
    fn errors_serialise_to_one_sentence() {
        let response = Response::error(404, "no issue #7");
        assert_eq!(response.status, 404);
        assert_eq!(response.body, br#"{"error":"no issue #7"}"#);
    }
}
