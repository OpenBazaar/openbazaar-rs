//! The public sale onion's loopback side.
//!
//! Tor accepts the connection and hands the bytes here. Only the catalog,
//! media, review, moderator-profile, order-mail, and escrow-mail routes are forwarded to the
//! shop listener,
//! with `Host` rewritten to that listener and `Origin` removed. Every other
//! path is answered here and is not opened on the shop. The shop's response
//! is copied back as bytes. A `Location` header is not a new connection.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::http_parse;

const ANSWER_MAX: usize = 4 * 1024 * 1024 + 8 * 1024;

pub(crate) fn bind() -> std::io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
}

pub(crate) fn serve(listener: TcpListener, main_port: u16, web_dir: Option<PathBuf>, stop: &AtomicBool) {
    listener.set_nonblocking(true).ok();
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                handle(stream, main_port, web_dir.as_deref());
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(50)),
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// The catalog a buyer's page may read: the signed store and the images it
/// names. Mail, reviews, and moderator routes stay between nodes.
pub(crate) fn catalog_read(method: &str, path: &str) -> bool {
    method == "GET" && (path == "/public/store" || path == "/public/catalog" || path == "/public/admission-index" || path == "/public/admission-addresses" || media(path))
}

pub(crate) fn allowed(method: &str, path: &str) -> bool {
    match (method, path) {
        ("GET", "/public/store" | "/public/catalog" | "/public/admission-index" | "/public/admission-addresses" | "/public/moderator" | "/public/moderators" | "/public/classifieds-board") => true,
        ("GET", path) if media(path) => true,
        ("GET" | "POST", "/public/reviews") => true,
        ("POST", "/mail" | "/mail/reply" | "/mail/pull" | "/escrow/mail") => true,
        ("OPTIONS", "/mail" | "/mail/pull") => true,
        _ => false,
    }
}

/// Routes where cross-origin browser requests (with `Origin` header) are
/// permitted. The moderator shop page posts sealed orders to the vendor's
/// `/mail` and pulls status via `/mail/pull` as cross-origin fetches.
fn cors_mail(method: &str, path: &str) -> bool {
    matches!((method, path), ("POST" | "OPTIONS", "/mail" | "/mail/pull"))
}

/// Read-only signed catalog endpoints the moderator shop may fetch cross-origin.
fn cors_public_read(method: &str, path: &str) -> bool {
    method == "GET" && (path == "/public/store" || media(path))
}

fn cors_cross_origin(method: &str, path: &str) -> bool {
    cors_mail(method, path) || cors_public_read(method, path)
}

const CORS_HEADERS: &str = "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nAccess-Control-Max-Age: 86400\r\n";

fn media(path: &str) -> bool {
    let Some(hash) = path.strip_prefix("/public/media/") else {
        return false;
    };
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn handle(mut client: TcpStream, main_port: u16, web_dir: Option<&Path>) {
    let _ = client.set_read_timeout(Some(http_parse::READ_DEADLINE));
    let raw = match http_parse::read(&mut client) {
        Ok(raw) => raw,
        Err(_) => {
            let _ = client.write_all(&plain("400 Bad Request", "bad request"));
            return;
        }
    };
    let request = match http_parse::parse(&raw) {
        Ok(request) => request,
        Err(refusal) => {
            let _ = client.write_all(&plain(refusal.status, refusal.message));
            return;
        }
    };
    if !allowed(request.method, request.path) {
        // The shop page, when this node has one. Closed routes stay closed:
        // a missing file is still 404, and it is not forwarded to the shop.
        if request.method == "GET" {
            if let Some(page) = web_dir.and_then(|dir| crate::remote::bare_shop_page(dir, request.path)) {
                let _ = client.write_all(&page);
                return;
            }
        }
        let _ = client.write_all(&plain("404 Not Found", "not found"));
        return;
    }
    // CORS preflight for /mail and /mail/pull (browser checkout from moderator page).
    if request.method == "OPTIONS" && cors_mail(request.method, request.path) {
        let _ = client.write_all(
            format!("HTTP/1.1 204 No Content\r\n{CORS_HEADERS}Connection: close\r\n\r\n").as_bytes(),
        );
        return;
    }
    // Refuse browser-originated requests on all routes EXCEPT mail and the
    // read-only public store/media catalog. Other routes stay locked (#247).
    if http_parse::header(request.head, "origin").is_some() && !cors_cross_origin(request.method, request.path) {
        let _ = client.write_all(&plain("403 Forbidden", "pages may not use this endpoint"));
        return;
    }
    let content_type = http_parse::header(request.head, "content-type").map(str::to_string);
    let outbound = rebuild(request.method, request.path, content_type.as_deref(), request.body, main_port);
    match forward(&outbound, main_port) {
        Ok(mut bytes) => {
            // Append CORS header for cross-origin mail and public read responses.
            if cors_cross_origin(request.method, request.path) {
                bytes = append_cors_header(bytes);
            }
            let _ = client.write_all(&bytes);
        }
        Err(_) => {
            let _ = client.write_all(&plain("502 Bad Gateway", "the shop did not answer"));
        }
    }
}

/// Inject `Access-Control-Allow-Origin: *` into an HTTP response (after the
/// status line, before the body) unless the upstream response already includes
/// an `Access-Control-Allow-Origin` header. If the response is not parseable,
/// return it unchanged — the browser will reject the opaque response on its own.
fn append_cors_header(raw: Vec<u8>) -> Vec<u8> {
    const INJECT: &[u8] = b"Access-Control-Allow-Origin: *\r\n";
    let Some(pos) = raw.windows(2).position(|w| w == b"\r\n") else {
        return raw;
    };
    let insert = pos + 2; // right after the status line
    let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(raw.len());
    let headers = if head_end > insert { &raw[insert..head_end] } else { &[] };
    if has_header(headers, b"access-control-allow-origin") {
        return raw;
    }
    let mut out = Vec::with_capacity(raw.len() + INJECT.len());
    out.extend_from_slice(&raw[..insert]);
    out.extend_from_slice(INJECT);
    out.extend_from_slice(&raw[insert..]);
    out
}

fn has_header(headers: &[u8], name: &[u8]) -> bool {
    headers.split(|&b| b == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            return false;
        };
        line[..colon].eq_ignore_ascii_case(name)
    })
}

fn rebuild(method: &str, path: &str, content_type: Option<&str>, body: &[u8], main_port: u16) -> Vec<u8> {
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{main_port}\r\nConnection: close\r\n");
    if !body.is_empty() {
        if let Some(content_type) = content_type {
            head.push_str(&format!("Content-Type: {content_type}\r\n"));
        }
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

fn forward(request: &[u8], main_port: u16) -> std::io::Result<Vec<u8>> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, main_port));
    let mut shop = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    shop.set_read_timeout(Some(Duration::from_secs(10)))?;
    shop.write_all(request)?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    while buf.len() < ANSWER_MAX {
        match shop.read(&mut chunk) {
            Ok(0) => return Ok(buf),
            Ok(n) => {
                let room = ANSWER_MAX - buf.len();
                buf.extend_from_slice(&chunk[..n.min(room)]);
                if n > room {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "answer too large"));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock || err.kind() == std::io::ErrorKind::TimedOut => {
                return if buf.is_empty() { Err(err) } else { Ok(buf) };
            }
            Err(err) => return if buf.is_empty() { Err(err) } else { Ok(buf) },
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "answer too large"))
}

fn plain(status: &str, message: &str) -> Vec<u8> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        message.len()
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(message.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};
    use std::thread;

    #[test]
    fn the_allowlist_is_the_sale_routes_only() {
        assert!(catalog_read("GET", "/public/store"));
        assert!(catalog_read("GET", "/public/catalog"));
        assert!(catalog_read("GET", "/public/admission-index"));
        assert!(catalog_read("GET", "/public/admission-addresses"));
        assert!(catalog_read("GET", "/public/media/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"));
        assert!(!catalog_read("POST", "/public/store"));
        assert!(!catalog_read("POST", "/public/catalog"));
        assert!(!catalog_read("GET", "/public/reviews"));
        assert!(!catalog_read("GET", "/public/media/abcd"));
        assert!(!catalog_read("GET", "/wallet"));
        assert!(allowed("GET", "/public/store"));
        assert!(allowed("GET", "/public/catalog"));
        assert!(allowed("GET", "/public/admission-index"));
        assert!(allowed("GET", "/public/admission-addresses"));
        assert!(allowed("GET", "/public/moderator"));
        assert!(allowed("GET", "/public/moderators"));
        assert!(!allowed("GET", "/moderator/profile"));
        assert!(!allowed("PUT", "/public/moderator"));
        assert!(!allowed("POST", "/moderators"));
        assert!(allowed("GET", "/public/media/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"));
        assert!(allowed("POST", "/public/reviews"));
        assert!(allowed("GET", "/public/reviews"));
        assert!(allowed("POST", "/mail"));
        assert!(allowed("POST", "/mail/reply"));
        assert!(allowed("POST", "/mail/pull"));
        assert!(!allowed("GET", "/mail/pull"));
        assert!(!catalog_read("POST", "/mail/pull"));
        assert!(allowed("POST", "/escrow/mail"));
        assert!(!allowed("GET", "/escrow/mail"));
        assert!(!allowed("GET", "/escrow/sales"));
        assert!(!allowed("POST", "/escrow/bump"));
        assert!(allowed("GET", "/public/classifieds-board"));
        assert!(!allowed("POST", "/public/classifieds-board"));
        assert!(!allowed("POST", "/wallet"));
        assert!(!allowed("POST", "/orders/x/pay"));
        assert!(!allowed("GET", "/wallet"));
        assert!(!allowed("GET", "/catalog"));
        assert!(!allowed("GET", "/follows"));
        assert!(!allowed("POST", "/public/store"));
        assert!(!allowed("GET", "/mail"));
        assert!(!allowed("GET", "/public/media/../wallet"));
        assert!(!allowed("GET", "/public/media/abcd"));
    }

    #[test]
    fn the_sale_gate_does_not_open_the_rest_of_the_node_or_follow_a_redirect() {
        let trap = TcpListener::bind("127.0.0.1:0").unwrap();
        let trap_port = trap.local_addr().unwrap().port();
        let trap_hits = Arc::new(AtomicUsize::new(0));
        let trap_counted = trap_hits.clone();
        thread::spawn(move || {
            for mut stream in trap.incoming().flatten() {
                trap_counted.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 128];
                let _ = stream.read(&mut buf);
            }
        });
        let backend = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = backend.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let record = seen.clone();
        let opened = Arc::new(AtomicUsize::new(0));
        let counting = opened.clone();
        thread::spawn(move || {
            for mut stream in backend.incoming().flatten() {
                counting.fetch_add(1, Ordering::SeqCst);
                let mut buf = vec![0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                record.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).into_owned());
                let body = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{trap_port}/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(body.as_bytes());
            }
        });
        let gate = bind().unwrap();
        let gate_addr = gate.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        thread::spawn(move || serve(gate, port, None, &stopping));

        let wallet = talk(gate_addr, "GET /wallet HTTP/1.1\r\nHost: example.onion\r\nConnection: close\r\n\r\n");
        assert!(wallet.starts_with("HTTP/1.1 404"), "{wallet}");
        let catalog = talk(gate_addr, "GET /catalog HTTP/1.1\r\nHost: example.onion\r\nConnection: close\r\n\r\n");
        assert!(catalog.starts_with("HTTP/1.1 404"), "{catalog}");
        assert_eq!(opened.load(Ordering::SeqCst), 0, "a closed path was forwarded");

        // GET /public/catalog with Origin is still refused at the gate (not store/media).
        let catalog_origin = talk(
            gate_addr,
            "GET /public/catalog HTTP/1.1\r\nHost: example.onion\r\nOrigin: http://evil.example\r\nConnection: close\r\n\r\n",
        );
        assert!(
            catalog_origin.starts_with("HTTP/1.1 403"),
            "Origin on non-store catalog should be refused: {catalog_origin}"
        );
        assert_eq!(opened.load(Ordering::SeqCst), 0, "catalog Origin request was forwarded");

        // GET /public/store with Origin is forwarded like a normal catalog read.
        let with_origin = talk(
            gate_addr,
            "GET /public/store HTTP/1.1\r\nHost: example.onion\r\nOrigin: http://evil.example\r\nConnection: close\r\n\r\n",
        );
        thread::sleep(Duration::from_millis(150));
        assert!(with_origin.starts_with("HTTP/1.1 302"), "store with Origin should forward: {with_origin}");
        assert!(
            with_origin.contains("Access-Control-Allow-Origin: *"),
            "store with Origin should include CORS: {with_origin}"
        );
        assert_eq!(opened.load(Ordering::SeqCst), 1, "store with Origin should be forwarded");

        // Without Origin, the request is forwarded normally.
        let store = talk(
            gate_addr,
            "GET /public/store HTTP/1.1\r\nHost: example.onion\r\nConnection: close\r\n\r\n",
        );
        thread::sleep(Duration::from_millis(150));
        assert!(store.starts_with("HTTP/1.1 302"), "{store}");
        assert_eq!(opened.load(Ordering::SeqCst), 2);
        assert_eq!(trap_hits.load(Ordering::SeqCst), 0, "the gate followed Location");
        let forwarded = seen.lock().unwrap()[0].clone();
        assert!(forwarded.contains(&format!("Host: 127.0.0.1:{port}")), "{forwarded}");
        assert!(!forwarded.to_ascii_lowercase().contains("origin"), "{forwarded}");
        assert!(forwarded.starts_with("GET /public/store "), "{forwarded}");
        stop.store(true, Ordering::SeqCst);
    }

    #[test]
    fn the_store_onion_serves_the_shop_page_and_not_the_wallet() {
        let dir = crate::harness::TempDir::new();
        let web = dir.0.join("web");
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(web.join("index.html"), "<html><head></head><body>shop</body></html>").unwrap();
        let backend = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = backend.local_addr().unwrap().port();
        let opened = Arc::new(AtomicUsize::new(0));
        let counting = opened.clone();
        thread::spawn(move || {
            for mut stream in backend.incoming().flatten() {
                counting.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 64];
                let _ = stream.read(&mut buf);
            }
        });
        let gate = bind().unwrap();
        let gate_addr = gate.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        thread::spawn(move || serve(gate, port, Some(web), &stopping));
        let page = talk(gate_addr, "GET / HTTP/1.1\r\nHost: example.onion\r\nConnection: close\r\n\r\n");
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(page.contains("shop") && !page.contains("ob-home-node") && !page.contains("ob-local-shop"), "{page}");
        let wallet = talk(gate_addr, "GET /wallet HTTP/1.1\r\nHost: example.onion\r\nConnection: close\r\n\r\n");
        assert!(wallet.starts_with("HTTP/1.1 404"), "{wallet}");
        thread::sleep(Duration::from_millis(80));
        assert_eq!(opened.load(Ordering::SeqCst), 0, "the shop page was forwarded");
        stop.store(true, Ordering::SeqCst);
    }

    #[test]
    fn cors_preflight_and_mail_origin_allowed_while_other_routes_blocked() {
        let backend = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = backend.local_addr().unwrap().port();
        let opened = Arc::new(AtomicUsize::new(0));
        let counting = opened.clone();
        thread::spawn(move || {
            for mut stream in backend.incoming().flatten() {
                counting.fetch_add(1, Ordering::SeqCst);
                let mut buf = vec![0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let _ = n;
                let reply = "HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        let gate = bind().unwrap();
        let gate_addr = gate.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        thread::spawn(move || serve(gate, port, None, &stopping));

        // OPTIONS /mail preflight returns 204 with CORS headers, not forwarded.
        let preflight = talk(
            gate_addr,
            "OPTIONS /mail HTTP/1.1\r\nHost: vendor.onion\r\nOrigin: http://moderator.onion\r\nConnection: close\r\n\r\n",
        );
        assert!(preflight.starts_with("HTTP/1.1 204"), "{preflight}");
        assert!(preflight.contains("Access-Control-Allow-Origin: *"), "{preflight}");
        assert!(preflight.contains("Access-Control-Allow-Methods:"), "{preflight}");
        thread::sleep(Duration::from_millis(50));
        assert_eq!(opened.load(Ordering::SeqCst), 0, "OPTIONS preflight was forwarded");

        // OPTIONS /mail/pull preflight also works.
        let preflight2 = talk(
            gate_addr,
            "OPTIONS /mail/pull HTTP/1.1\r\nHost: vendor.onion\r\nOrigin: http://moderator.onion\r\nConnection: close\r\n\r\n",
        );
        assert!(preflight2.starts_with("HTTP/1.1 204"), "{preflight2}");
        assert_eq!(opened.load(Ordering::SeqCst), 0, "OPTIONS /mail/pull was forwarded");

        // POST /mail with Origin succeeds and response has ACAO header.
        let mail_with_origin = talk(
            gate_addr,
            "POST /mail HTTP/1.1\r\nHost: vendor.onion\r\nOrigin: http://moderator.onion\r\nContent-Length: 4\r\nConnection: close\r\n\r\ntest",
        );
        assert!(mail_with_origin.starts_with("HTTP/1.1 202"), "{mail_with_origin}");
        assert!(mail_with_origin.contains("Access-Control-Allow-Origin: *"), "{mail_with_origin}");
        assert_eq!(opened.load(Ordering::SeqCst), 1, "POST /mail should be forwarded");

        // POST /mail/pull with Origin also succeeds.
        let pull_with_origin = talk(
            gate_addr,
            "POST /mail/pull HTTP/1.1\r\nHost: vendor.onion\r\nOrigin: http://moderator.onion\r\nContent-Length: 4\r\nConnection: close\r\n\r\ntest",
        );
        assert!(pull_with_origin.starts_with("HTTP/1.1 202"), "{pull_with_origin}");
        assert!(pull_with_origin.contains("Access-Control-Allow-Origin: *"), "{pull_with_origin}");
        assert_eq!(opened.load(Ordering::SeqCst), 2);

        // POST /wallet with Origin still blocked.
        let wallet_origin = talk(
            gate_addr,
            "POST /wallet HTTP/1.1\r\nHost: vendor.onion\r\nOrigin: http://evil.example\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        assert!(wallet_origin.starts_with("HTTP/1.1 404"), "{wallet_origin}");
        assert_eq!(opened.load(Ordering::SeqCst), 2, "/wallet must not be forwarded");

        // GET /public/store with Origin succeeds with CORS header.
        let store_origin = talk(
            gate_addr,
            "GET /public/store HTTP/1.1\r\nHost: vendor.onion\r\nOrigin: http://moderator.onion\r\nConnection: close\r\n\r\n",
        );
        assert!(store_origin.starts_with("HTTP/1.1 202"), "{store_origin}");
        assert!(store_origin.contains("Access-Control-Allow-Origin: *"), "{store_origin}");
        assert_eq!(opened.load(Ordering::SeqCst), 3, "/public/store with Origin should be forwarded");

        let media_hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let media_origin = talk(
            gate_addr,
            &format!(
                "GET /public/media/{media_hash} HTTP/1.1\r\nHost: vendor.onion\r\nOrigin: http://moderator.onion\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(media_origin.starts_with("HTTP/1.1 202"), "{media_origin}");
        assert!(media_origin.contains("Access-Control-Allow-Origin: *"), "{media_origin}");
        assert_eq!(opened.load(Ordering::SeqCst), 4, "/public/media with Origin should be forwarded");

        // GET /wallet with Origin still blocked at the gate (not on allowlist).
        let wallet_get_origin = talk(
            gate_addr,
            "GET /wallet HTTP/1.1\r\nHost: vendor.onion\r\nOrigin: http://evil.example\r\nConnection: close\r\n\r\n",
        );
        assert!(wallet_get_origin.starts_with("HTTP/1.1 404"), "{wallet_get_origin}");
        assert_eq!(opened.load(Ordering::SeqCst), 4, "/wallet must not be forwarded");

        // GET /public/catalog with Origin blocked (allowed route, no CORS).
        let catalog_origin = talk(
            gate_addr,
            "GET /public/catalog HTTP/1.1\r\nHost: vendor.onion\r\nOrigin: http://evil.example\r\nConnection: close\r\n\r\n",
        );
        assert!(catalog_origin.starts_with("HTTP/1.1 403"), "{catalog_origin}");
        assert_eq!(opened.load(Ordering::SeqCst), 4, "/public/catalog with Origin must not be forwarded");

        stop.store(true, Ordering::SeqCst);
    }

    #[test]
    fn append_cors_header_does_not_duplicate_existing_acao() {
        let backend = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = backend.local_addr().unwrap().port();
        thread::spawn(move || {
            for mut stream in backend.incoming().flatten() {
                let mut buf = vec![0u8; 2048];
                let _ = stream.read(&mut buf);
                let reply = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        let gate = bind().unwrap();
        let gate_addr = gate.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        thread::spawn(move || serve(gate, port, None, &stopping));

        let resp = talk(
            gate_addr,
            "GET /public/store HTTP/1.1\r\nHost: vendor.onion\r\nOrigin: http://moderator.onion\r\nConnection: close\r\n\r\n",
        );
        assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
        let head = resp.split_once("\r\n\r\n").unwrap().0;
        let acao_count = head
            .lines()
            .filter(|line| line.to_ascii_lowercase().starts_with("access-control-allow-origin:"))
            .count();
        assert_eq!(acao_count, 1, "expected exactly one ACAO header, got {acao_count}: {head}");
        stop.store(true, Ordering::SeqCst);

        // Case-insensitive match on existing header.
        let lower = b"HTTP/1.1 200 OK\r\naccess-control-allow-origin: *\r\n\r\n".to_vec();
        assert_eq!(append_cors_header(lower.clone()), lower);

        // Body containing header text does not suppress injection.
        let in_body = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nAccess-Control-Allow-Origin: *\r\n".to_vec();
        let out = String::from_utf8(append_cors_header(in_body)).unwrap();
        let out_head = out.split_once("\r\n\r\n").unwrap().0;
        assert!(out_head.contains("Access-Control-Allow-Origin: *"), "{out_head}");

        // Empty header block does not panic and injects once.
        let empty = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        assert_eq!(
            append_cors_header(empty),
            b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\n\r\n"
        );
    }

    fn talk(addr: SocketAddr, request: &str) -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    }
}
