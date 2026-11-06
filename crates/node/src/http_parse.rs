//! Reading and parsing one HTTP/1.x request, with hard limits.
//!
//! The head is at most [`MAX_HEAD`] bytes and [`MAX_HEADERS`] lines, the body
//! at most [`MAX_BODY`] bytes and exactly `Content-Length` long, and the whole
//! request must arrive within [`READ_DEADLINE`]. Chunked bodies are refused.
//! Nothing here panics on any input.

use std::io::Read;
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub(crate) const MAX_HEAD: usize = 8 * 1024;
pub(crate) const MAX_HEADERS: usize = 64;
/// Room for one sealed escrow letter (up to 64 KiB) plus JSON slack.
pub(crate) const MAX_BODY: usize = 72 * 1024;
const MAX_TARGET: usize = 2048;
pub(crate) const READ_DEADLINE: Duration = Duration::from_secs(5);

/// One parsed request. `head` is the request line and headers, without the blank line.
pub(crate) struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub head: &'a str,
    pub body: &'a [u8],
}

/// Why a request was refused before routing, as a status line and a message.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub status: &'static str,
    pub message: &'static str,
}

const fn refusal(status: &'static str, message: &'static str) -> Refusal {
    Refusal { status, message }
}

const BAD: Refusal = refusal("400 Bad Request", "bad request");
const HEAD_TOO_LARGE: Refusal = refusal("431 Request Header Fields Too Large", "request head too large");
const BODY_TOO_LARGE: Refusal = refusal("413 Content Too Large", "request body too large");
const CHUNKED: Refusal = refusal("501 Not Implemented", "chunked bodies are not accepted");

pub(crate) fn parse(raw: &[u8]) -> Result<Request<'_>, Refusal> {
    let Some(split) = find_head_end(raw) else {
        return Err(if raw.len() > MAX_HEAD { HEAD_TOO_LARGE } else { BAD });
    };
    if split > MAX_HEAD {
        return Err(HEAD_TOO_LARGE);
    }
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| BAD)?;
    let rest = &raw[split + 4..];
    let mut lines = head.split("\r\n");
    let line = lines.next().ok_or(BAD)?;
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(BAD);
    };
    if method.is_empty() || method.len() > 16 || !method.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(BAD);
    }
    if !target.starts_with('/') || target.len() > MAX_TARGET || !target.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(BAD);
    }
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(BAD);
    }
    let mut count = 0;
    let mut length = None;
    for line in lines {
        count += 1;
        if count > MAX_HEADERS {
            return Err(HEAD_TOO_LARGE);
        }
        let (name, value) = line.split_once(':').ok_or(BAD)?;
        if name.is_empty() || !name.bytes().all(is_token) {
            return Err(BAD);
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(CHUNKED);
        }
        if name.eq_ignore_ascii_case("content-length") {
            let value = value.trim();
            if length.is_some() || value.is_empty() || value.len() > 9 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(BAD);
            }
            length = Some(value.parse::<usize>().map_err(|_| BAD)?);
        }
    }
    let length = length.unwrap_or(0);
    if length > MAX_BODY {
        return Err(BODY_TOO_LARGE);
    }
    if rest.len() < length {
        return Err(BAD);
    }
    let path = target.split('?').next().unwrap_or(target);
    Ok(Request {
        method,
        path,
        head,
        body: &rest[..length],
    })
}

/// The value of the only `name` header, `None` when it is missing.
/// A repeated header reads as missing, so a caller cannot smuggle a second value.
pub(crate) fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    let mut found = None;
    for line in head.split("\r\n").skip(1) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.eq_ignore_ascii_case(name) {
            if found.is_some() {
                return None;
            }
            found = Some(value.trim());
        }
    }
    found
}

/// True when the head has `name` at all, even repeated.
pub(crate) fn has_header(head: &str, name: &str) -> bool {
    head.split("\r\n")
        .skip(1)
        .any(|line| line.split_once(':').is_some_and(|(key, _)| key.eq_ignore_ascii_case(name)))
}

fn is_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn find_head_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Read one request within [`READ_DEADLINE`]. Stops early once the head says
/// the request is too large; [`parse`] then refuses what was read.
pub(crate) fn read(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let deadline = Instant::now() + READ_DEADLINE;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match wanted(&buf) {
            Wanted::Done => break,
            Wanted::More => {}
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        stream.set_read_timeout(Some(left))?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => buf.extend_from_slice(&chunk[..read]),
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                if buf.is_empty() {
                    return Err(err);
                }
                break;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(buf)
}

enum Wanted {
    More,
    Done,
}

fn wanted(buf: &[u8]) -> Wanted {
    let Some(split) = find_head_end(buf) else {
        return if buf.len() > MAX_HEAD { Wanted::Done } else { Wanted::More };
    };
    if split > MAX_HEAD {
        return Wanted::Done;
    }
    let head = String::from_utf8_lossy(&buf[..split]);
    let length = head
        .split("\r\n")
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    if length > MAX_BODY || buf.len() - split - 4 >= length {
        Wanted::Done
    } else {
        Wanted::More
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn a_plain_request_parses() {
        let raw = b"POST /orders?x=1 HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nContent-Length: 2\r\n\r\n{}extra";
        let request = parse(raw).unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/orders");
        assert_eq!(request.body, b"{}");
        assert_eq!(header(request.head, "host"), Some("127.0.0.1:8787"));
    }

    #[test]
    fn malformed_and_oversized_requests_are_refused() {
        let long_head = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD));
        let many = format!("GET / HTTP/1.1\r\n{}\r\n", "X: 1\r\n".repeat(MAX_HEADERS + 1));
        let big = format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        let cases: [(&[u8], &str); 12] = [
            (b"", "400"),
            (b"GET / HTTP/1.1\r\n", "400"),
            (b"GET /\r\n\r\n", "400"),
            (b"get / HTTP/1.1\r\n\r\n", "400"),
            (b"GET  / HTTP/1.1\r\n\r\n", "400"),
            (b"GET / HTTP/2\r\n\r\n", "400"),
            (b"GET / HTTP/1.1\r\nno colon\r\n\r\n", "400"),
            (b"POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\nab", "400"),
            (b"POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\na", "400"),
            (b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", "501"),
            (long_head.as_bytes(), "431"),
            (many.as_bytes(), "431"),
        ];
        for (raw, status) in cases {
            let refused = parse(raw).err().unwrap_or_else(|| panic!("{:?}", String::from_utf8_lossy(raw)));
            assert!(refused.status.starts_with(status), "{:?}: {}", String::from_utf8_lossy(raw), refused.status);
        }
        assert!(parse(big.as_bytes()).err().unwrap().status.starts_with("413"));
    }

    #[test]
    fn a_repeated_header_reads_as_missing() {
        let head = "GET / HTTP/1.1\r\nHost: 127.0.0.1:1\r\nHost: evil.example";
        assert_eq!(header(head, "host"), None);
        assert!(has_header(head, "HOST"));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(4000))]

        #[test]
        fn parse_never_panics_on_bytes(raw in proptest::collection::vec(any::<u8>(), 0..2048)) {
            let _ = parse(&raw);
            let _ = wanted(&raw);
        }

        #[test]
        fn parse_never_panics_on_request_shapes(
            method in "[A-Za-z]{0,8}",
            target in "[ -~]{0,64}",
            headers in proptest::collection::vec(("[ -~]{0,16}", "[ -~\\x80-\\xff]{0,32}"), 0..8),
            length in proptest::option::of("[0-9+-]{0,12}"),
            body in proptest::collection::vec(any::<u8>(), 0..256),
        ) {
            let mut raw = format!("{method} {target} HTTP/1.1\r\n").into_bytes();
            for (name, value) in headers {
                raw.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
            }
            if let Some(length) = length {
                raw.extend_from_slice(format!("Content-Length: {length}\r\n").as_bytes());
            }
            raw.extend_from_slice(b"\r\n");
            raw.extend_from_slice(&body);
            if let Ok(request) = parse(&raw) {
                prop_assert!(request.body.len() <= MAX_BODY);
                prop_assert!(request.path.starts_with('/'));
            }
        }
    }
}
