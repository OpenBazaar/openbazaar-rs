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
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::http_parse;

const ANSWER_MAX: usize = 4 * 1024 * 1024 + 8 * 1024;

pub(crate) fn bind() -> std::io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
}

pub(crate) fn serve(listener: TcpListener, main_port: u16, stop: &AtomicBool) {
    listener.set_nonblocking(true).ok();
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                handle(stream, main_port);
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(50)),
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

pub(crate) fn allowed(method: &str, path: &str) -> bool {
    match (method, path) {
        ("GET", "/public/store" | "/public/moderator" | "/public/moderators") => true,
        ("GET", path) if media(path) => true,
        ("GET" | "POST", "/public/reviews") => true,
        ("POST", "/mail" | "/mail/reply" | "/escrow/mail") => true,
        _ => false,
    }
}

fn media(path: &str) -> bool {
    let Some(hash) = path.strip_prefix("/public/media/") else {
        return false;
    };
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn handle(mut client: TcpStream, main_port: u16) {
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
        let _ = client.write_all(&plain("404 Not Found", "not found"));
        return;
    }
    let content_type = http_parse::header(request.head, "content-type").map(str::to_string);
    let outbound = rebuild(request.method, request.path, content_type.as_deref(), request.body, main_port);
    match forward(&outbound, main_port) {
        Ok(bytes) => {
            let _ = client.write_all(&bytes);
        }
        Err(_) => {
            let _ = client.write_all(&plain("502 Bad Gateway", "the shop did not answer"));
        }
    }
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
        assert!(allowed("GET", "/public/store"));
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
        assert!(allowed("POST", "/escrow/mail"));
        assert!(!allowed("GET", "/escrow/mail"));
        assert!(!allowed("GET", "/escrow/sales"));
        assert!(!allowed("POST", "/escrow/bump"));
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
        thread::spawn(move || serve(gate, port, &stopping));

        let wallet = talk(gate_addr, "GET /wallet HTTP/1.1\r\nHost: example.onion\r\nConnection: close\r\n\r\n");
        assert!(wallet.starts_with("HTTP/1.1 404"), "{wallet}");
        let catalog = talk(gate_addr, "GET /catalog HTTP/1.1\r\nHost: example.onion\r\nConnection: close\r\n\r\n");
        assert!(catalog.starts_with("HTTP/1.1 404"), "{catalog}");
        assert_eq!(opened.load(Ordering::SeqCst), 0, "a closed path was forwarded");

        let store = talk(
            gate_addr,
            "GET /public/store HTTP/1.1\r\nHost: example.onion\r\nOrigin: http://evil.example\r\nConnection: close\r\n\r\n",
        );
        thread::sleep(Duration::from_millis(150));
        assert!(store.starts_with("HTTP/1.1 302"), "{store}");
        assert_eq!(opened.load(Ordering::SeqCst), 1);
        assert_eq!(trap_hits.load(Ordering::SeqCst), 0, "the gate followed Location");
        let forwarded = seen.lock().unwrap()[0].clone();
        assert!(forwarded.contains(&format!("Host: 127.0.0.1:{port}")), "{forwarded}");
        assert!(!forwarded.to_ascii_lowercase().contains("origin"), "{forwarded}");
        assert!(forwarded.starts_with("GET /public/store "), "{forwarded}");
        stop.store(true, Ordering::SeqCst);
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
