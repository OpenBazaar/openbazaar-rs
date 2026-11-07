//! One HTTP request to another node's sale routes.
//!
//! A follow on this machine is loopback HTTP, the same as a local demo.
//! A follow of `http://….onion` is one request through this process's Tor
//! client. The name is not resolved on the clearnet, and a 3xx is not a
//! second connection: the `Location` is never dialed, even when it names
//! another onion or a loopback address.

use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;

use tormail::OnionAddress;

use crate::escrow_mail::{check_loopback, loopback_request};

/// How this process reaches an onion. Set when `--remote` starts Tor.
pub(crate) struct PeerLink {
    tor: OnceLock<std::sync::Arc<crate::remote::Tor>>,
    #[cfg(test)]
    fake: OnceLock<std::sync::Arc<dyn OnionDial>>,
}

#[cfg(test)]
pub(crate) trait OnionDial: Send + Sync {
    fn exchange(&self, onion: &str, request: &[u8], max: usize) -> Result<Vec<u8>, String>;
}

impl Default for PeerLink {
    fn default() -> Self {
        Self {
            tor: OnceLock::new(),
            #[cfg(test)]
            fake: OnceLock::new(),
        }
    }
}

impl PeerLink {
    pub(crate) fn attach(&self, tor: std::sync::Arc<crate::remote::Tor>) {
        let _ = self.tor.set(tor);
    }

    #[cfg(test)]
    pub(crate) fn use_fake(&self, dial: std::sync::Arc<dyn OnionDial>) {
        let _ = self.fake.set(dial);
    }

    fn exchange(&self, onion: &OnionAddress, request: &[u8], max: usize) -> Result<Vec<u8>, String> {
        #[cfg(test)]
        if let Some(fake) = self.fake.get() {
            return fake.exchange(onion.as_str(), request, max);
        }
        match self.tor.get() {
            Some(tor) => tor.exchange(onion, request, max),
            None => Err("This node is not on Tor. Start it with --remote so a sale to an onion stays on Tor.".into()),
        }
    }
}

/// A completed HTTP answer. A redirect is not represented: [`call`] returns
/// an error instead, and does not open `Location`.
#[derive(Debug)]
pub(crate) struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
}

enum Kind {
    Loopback,
    Onion { host: OnionAddress, path: String },
}

fn not_sale(url: &str) -> String {
    format!("{url} is not a loopback address or a Tor onion")
}

fn classify(url: &str) -> Result<Kind, String> {
    let url = url.trim();
    let rest = url.strip_prefix("http://").ok_or_else(|| not_sale(url))?;
    if rest.contains('@') {
        return Err(not_sale(url));
    }
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    if authority.is_empty() || path.contains("..") {
        return Err(not_sale(url));
    }
    if check_loopback(&format!("http://{authority}")).is_ok() {
        return Ok(Kind::Loopback);
    }
    let host = match authority.rsplit_once(':') {
        Some((host, "80")) => host,
        Some(_) => return Err(not_sale(url)),
        None => authority,
    };
    let onion = OnionAddress::parse(host).map_err(|_| not_sale(url))?;
    Ok(Kind::Onion { host: onion, path })
}

/// A store or buyer base URL: loopback `http://`, or `http://` plus a v3
/// onion, with no path. `:80` on an onion is dropped. Anything else, including
/// a clearnet host, is refused before a connection.
pub(crate) fn sale_base(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/').to_string();
    if url.trim_start_matches("http://").contains('/') {
        return Err(format!("{url} is a base URL with no path"));
    }
    match classify(&url)? {
        Kind::Loopback => Ok(url),
        Kind::Onion { host, .. } => Ok(format!("http://{}", host.as_str())),
    }
}

/// One request. `max` is the largest body that is accepted. A 3xx is an
/// error and is not followed.
pub(crate) fn call(
    peers: &PeerLink,
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    content_type: Option<&str>,
    timeout: Duration,
    max: usize,
) -> Result<Answer, String> {
    let answer = match classify(url)? {
        Kind::Loopback => call_loopback(method, url, body, content_type, timeout, max)?,
        Kind::Onion { host, path } => call_onion(peers, method, url, &host, &path, body, content_type, max)?,
    };
    if (300..400).contains(&answer.status) {
        return Err(format!("{url}: {}, a redirect, which is not followed", answer.status));
    }
    Ok(answer)
}

fn call_loopback(
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    content_type: Option<&str>,
    timeout: Duration,
    max: usize,
) -> Result<Answer, String> {
    let request = loopback_request(method, url, timeout)?;
    let sent = match body {
        Some(bytes) => {
            let request = match content_type {
                Some(content_type) => request.set("Content-Type", content_type),
                None => request,
            };
            request.send_bytes(bytes)
        }
        None => request.call(),
    };
    match sent {
        Ok(response) => read_answer(response, max),
        Err(ureq::Error::Status(status, response)) => {
            let mut answer = read_answer(response, max)?;
            answer.status = status;
            Ok(answer)
        }
        Err(err) => Err(err.to_string()),
    }
}

fn read_answer(response: ureq::Response, max: usize) -> Result<Answer, String> {
    let status = response.status();
    let mut body = Vec::new();
    response
        .into_reader()
        .take(max as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|err| err.to_string())?;
    if body.len() > max {
        return Err("the peer's answer is too large".into());
    }
    Ok(Answer { status, body })
}

fn call_onion(
    peers: &PeerLink,
    method: &str,
    url: &str,
    host: &OnionAddress,
    path: &str,
    body: Option<&[u8]>,
    content_type: Option<&str>,
    max: usize,
) -> Result<Answer, String> {
    let request = request_bytes(method, host.as_str(), path, body, content_type);
    let raw = peers.exchange(host, &request, max.saturating_add(8 * 1024))?;
    interpret(url, &raw, max)
}

fn request_bytes(method: &str, host: &str, path: &str, body: Option<&[u8]>, content_type: Option<&str>) -> Vec<u8> {
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    if let Some(bytes) = body {
        if let Some(content_type) = content_type {
            head.push_str(&format!("Content-Type: {content_type}\r\n"));
        }
        head.push_str(&format!("Content-Length: {}\r\n", bytes.len()));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    if let Some(bytes) = body {
        out.extend_from_slice(bytes);
    }
    out
}

/// Parse one answer. Extra bytes after `Content-Length` are ignored. A 3xx
/// is returned as an error so the caller has no `Location` to dial.
fn interpret(url: &str, raw: &[u8], max: usize) -> Result<Answer, String> {
    let head_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| format!("{url}: the peer's answer was not HTTP"))?;
    let head = std::str::from_utf8(&raw[..head_end]).map_err(|_| format!("{url}: the peer's answer was not HTTP"))?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| format!("{url}: the peer's answer was not HTTP"))?;
    let mut length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err(format!("{url}: the peer's answer was not HTTP"));
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| format!("{url}: the peer's answer was not HTTP"))?,
            );
        }
    }
    let body_at = head_end + 4;
    let available = raw.len().saturating_sub(body_at);
    let take = length.unwrap_or(available).min(available);
    if take > max {
        return Err("the peer's answer is too large".into());
    }
    if (300..400).contains(&status) {
        return Err(format!("{url}: {status}, a redirect, which is not followed"));
    }
    Ok(Answer { status, body: raw[body_at..body_at + take].to_vec() })
}

impl crate::Node {
    /// Where the seller sends this order's reply: the public sale onion when
    /// `--remote` has published one, otherwise this node's loopback mailbox.
    pub(crate) fn sale_reply_url(&self) -> Option<String> {
        self.remote
            .services
            .sale_onion()
            .map(|onion| format!("http://{onion}"))
            .or_else(|| self.own_url().map(str::to_string))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::escrow_outbox::Transport;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Instant;

    const ONION: &str = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid.onion";

    struct Script {
        hits: AtomicUsize,
        trap: std::net::SocketAddr,
        status: &'static str,
        location: String,
        seen: Mutex<Vec<Vec<u8>>>,
    }

    impl OnionDial for Script {
        fn exchange(&self, onion: &str, request: &[u8], _max: usize) -> Result<Vec<u8>, String> {
            let n = self.hits.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().unwrap().push(request.to_vec());
            let text = std::str::from_utf8(request).unwrap_or("");
            assert!(text.contains(onion), "{text}");
            assert!(!text.contains("127.0.0.1"), "{text}");
            if n > 0 {
                if self.status.starts_with("302") {
                    let _ = std::net::TcpStream::connect(self.trap);
                }
                return Ok(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_vec());
            }
            let body = format!(
                "HTTP/1.1 {}\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                self.status, self.location
            );
            Ok(body.into_bytes())
        }
    }

    fn trap() -> (std::net::SocketAddr, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                counted.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 256];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        });
        (addr, hits)
    }

    fn peers(script: Arc<Script>) -> PeerLink {
        let peers = PeerLink::default();
        peers.use_fake(script);
        peers
    }

    #[test]
    fn an_onion_sale_is_one_request_and_a_redirect_is_not_followed() {
        for (method, path, to_onion) in [("GET", "/public/store", false), ("POST", "/mail", true), ("POST", "/public/reviews", false)] {
            let (trap_addr, hits) = trap();
            let location = if to_onion {
                format!("http://{ONION}/stolen")
            } else {
                format!("http://127.0.0.1:{}/stolen", trap_addr.port())
            };
            let script = Arc::new(Script {
                hits: AtomicUsize::new(0),
                trap: trap_addr,
                status: "302 Found",
                location,
                seen: Mutex::new(Vec::new()),
            });
            let link = peers(Arc::clone(&script));
            let url = format!("http://{ONION}{path}");
            let started = Instant::now();
            let err = call(&link, method, &url, Some(b"letter"), Some("application/octet-stream"), Duration::from_secs(1), 1024)
                .unwrap_err();
            assert!(started.elapsed() < Duration::from_millis(500), "the redirect was dialed: {err}");
            thread::sleep(Duration::from_millis(150));
            assert!(err.contains("302") && err.contains("redirect"), "{err}");
            assert_eq!(script.hits.load(Ordering::SeqCst), 1, "a second dial followed the redirect: {err}");
            assert_eq!(hits.load(Ordering::SeqCst), 0, "the redirect target was dialed: {err}");
            let sent = String::from_utf8(script.seen.lock().unwrap()[0].clone()).unwrap();
            assert!(sent.starts_with(&format!("{method} {path} ")), "{sent}");
            assert!(sent.contains(&format!("Host: {ONION}")), "{sent}");
        }
    }

    #[test]
    fn an_onion_order_post_uses_the_dial_once() {
        let (trap_addr, hits) = trap();
        let script = Arc::new(Script {
            hits: AtomicUsize::new(0),
            trap: trap_addr,
            status: "200 OK",
            location: format!("http://{ONION}/again"),
            seen: Mutex::new(Vec::new()),
        });
        let link = Arc::new(peers(Arc::clone(&script)));
        let url = format!("http://{ONION}/mail");
        let answer = call(&link, "POST", &url, Some(b"sealed-letter"), Some("application/octet-stream"), Duration::from_secs(1), 1024)
            .unwrap();
        assert_eq!(answer.status, 200);
        assert_eq!(script.hits.load(Ordering::SeqCst), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        let sent = String::from_utf8(script.seen.lock().unwrap()[0].clone()).unwrap();
        assert!(sent.contains("sealed-letter"), "{sent}");
        crate::escrow_outbox::UrlTransport::new(link)
            .deliver(&url, b"sealed-letter")
            .expect("a 200 on the onion is delivery");
        assert_eq!(script.hits.load(Ordering::SeqCst), 2, "each post is one dial");
    }

    #[test]
    fn a_clearnet_sale_url_is_refused_before_a_connection() {
        let script = Arc::new(Script {
            hits: AtomicUsize::new(0),
            trap: std::net::SocketAddr::from(([127, 0, 0, 1], 9)),
            status: "200 OK",
            location: String::new(),
            seen: Mutex::new(Vec::new()),
        });
        let link = peers(script.clone());
        let started = Instant::now();
        let err = call(
            &link,
            "GET",
            "http://seller.example/public/store",
            None,
            None,
            Duration::from_secs(1),
            1024,
        )
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(200), "clearnet was dialed");
        assert!(err.contains("loopback"), "{err}");
        assert_eq!(script.hits.load(Ordering::SeqCst), 0);
        let err = call(&PeerLink::default(), "GET", &format!("http://{ONION}/public/store"), None, None, Duration::from_secs(1), 64)
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(200), "the onion was dialed in the clear");
        assert!(err.contains("not on Tor"), "{err}");
    }

    #[test]
    fn sale_bases_are_loopback_or_a_v3_onion() {
        assert_eq!(sale_base(" http://127.0.0.1:9000/ ").unwrap(), "http://127.0.0.1:9000");
        assert_eq!(sale_base(&format!("http://{ONION}:80/")).unwrap(), format!("http://{ONION}"));
        for refused in [
            "http://203.0.113.9:9000",
            "http://10.0.0.1:9000",
            "https://127.0.0.1:9000",
            "http://127.0.0.1:9000/public",
            &format!("https://{ONION}"),
            &format!("http://{ONION}/public"),
            &format!("http://{ONION}:81"),
            "http://example.onion",
            &format!("http://buyer@{ONION}"),
        ] {
            assert!(sale_base(refused).is_err(), "{refused}");
        }
    }
}
