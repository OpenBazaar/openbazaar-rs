//! The LAN fast path: an encrypted channel on port 8790, an mDNS
//! announcement, and the first-run setup page.
//!
//! `POST /lan` carries one request sealed to the node key and gets the answer
//! sealed back ([`super::protocol::lan_open_request`]); inside, the request is
//! signed exactly as over Tor, so the LAN adds speed and nothing else. The only
//! plaintext is the setup page (`GET /`), served while no device is paired or
//! for 10 minutes after `ob-node pair --lan-page`. It shows a pairing code
//! and, on first run only, lets whoever sees it approve a request. That is the
//! LAN setup risk `docs/home-node.md` describes.

use std::io::{ErrorKind, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::gateway::{self, Entry};
use super::pair_cli::qr_svg;
use super::pairing::{Code, State};
use super::protocol::{lan_open_request, lan_seal_response, words};
use super::{pair_http, LAN_PORT};
use crate::http::http_response_cors;
use crate::http_parse::{self, header};
use crate::Node;

const PAGE_WINDOW: Duration = Duration::from_secs(10 * 60);
const MDNS_SERVICE: &str = "_openbazaar._tcp.local.";
const MDNS_HOST: &str = "openbazaar.local.";
const MAX_LAN_BODY: usize = 80 * 1024;

#[derive(Default)]
pub(crate) struct LanState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    addresses: Vec<String>,
    page_until: Option<Instant>,
    page_code: Option<Code>,
}

impl LanState {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// `host:port` of each private LAN address the channel listens on.
    pub(crate) fn addresses(&self) -> Vec<String> {
        self.inner().addresses.clone()
    }

    pub(crate) fn show_page_for_a_while(&self) {
        self.inner().page_until = Some(Instant::now() + PAGE_WINDOW);
    }
}

fn first_run(node: &Node) -> bool {
    node.remote.devices().list().is_empty()
}

fn page_open(node: &Node) -> bool {
    first_run(node)
        || node
            .remote
            .lan
            .inner()
            .page_until
            .is_some_and(|until| until > Instant::now())
}

/// Private IPv4 and unique-local IPv6 addresses of this machine.
fn lan_ips() -> Vec<IpAddr> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|iface| !iface.is_loopback())
        .map(|iface| iface.ip())
        .filter(|ip| match ip {
            IpAddr::V4(v4) => v4.is_private(),
            IpAddr::V6(v6) => (v6.segments()[0] & 0xfe00) == 0xfc00,
        })
        .collect()
}

pub(crate) fn serve(node: &Node, addr: SocketAddr, main_port: u16, stop: &AtomicBool) {
    let listener = match TcpListener::bind(addr)
        .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
    {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("ob-node: LAN channel could not listen on {addr}: {err}");
            return;
        }
    };
    let port = listener.local_addr().map_or(LAN_PORT, |addr| addr.port());
    let ips: Vec<IpAddr> = if addr.ip().is_unspecified() {
        lan_ips()
    } else {
        vec![addr.ip()]
    };
    node.remote.lan.inner().addresses = ips
        .iter()
        .map(|ip| SocketAddr::new(*ip, port).to_string())
        .collect();
    let _mdns = if addr.ip().is_loopback() {
        None
    } else {
        announce(node, &ips, port)
    };
    eprintln!("ob-node: LAN channel on port {port} (encrypted to the node key); setup page at http://openbazaar.local:{port}/ while no device is paired");
    std::thread::scope(|scope| {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    scope.spawn(move || connection(node, stream, main_port));
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(err) => {
                    eprintln!("ob-node: LAN channel: {err}");
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    });
}

/// Announce `_openbazaar._tcp` with the node id, so apps on the LAN find it
/// without typing. The daemon stops when the returned value is dropped.
fn announce(node: &Node, ips: &[IpAddr], port: u16) -> Option<mdns_sd::ServiceDaemon> {
    let daemon = mdns_sd::ServiceDaemon::new()
        .map_err(|err| eprintln!("ob-node: mDNS is off: {err}"))
        .ok()?;
    let node_id = node.remote.node_id();
    let properties = [("v", "2"), ("id", node_id.as_str())];
    let info = mdns_sd::ServiceInfo::new(
        MDNS_SERVICE,
        &format!("openbazaar-{}", &node_id[..8]),
        MDNS_HOST,
        ips,
        port,
        &properties[..],
    )
    .map_err(|err| eprintln!("ob-node: mDNS is off: {err}"))
    .ok()?;
    daemon
        .register(info)
        .map_err(|err| eprintln!("ob-node: mDNS is off: {err}"))
        .ok()?;
    Some(daemon)
}

fn connection(node: &Node, mut stream: TcpStream, main_port: u16) {
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let Ok(raw) = http_parse::read_up_to(&mut stream, MAX_LAN_BODY) else {
        return;
    };
    let _ = stream.write_all(&respond(node, &raw, main_port));
}

pub(crate) fn respond(node: &Node, raw: &[u8], main_port: u16) -> Vec<u8> {
    let Ok(request) = http_parse::parse_up_to(raw, MAX_LAN_BODY) else {
        return bare("400 Bad Request");
    };
    match (request.method, request.path) {
        ("POST", "/lan") => {
            let secret = node.remote.devices().node_secret();
            let Some((inner, response_key)) = lan_open_request(&secret, request.body) else {
                return bare("400 Bad Request");
            };
            let answer = gateway::answer(node, &inner, Entry::Lan, main_port);
            http_response_cors(
                "200 OK",
                "application/octet-stream",
                &lan_seal_response(&response_key, &answer),
                None,
            )
        }
        ("GET", "/") if page_open(node) => page(node),
        ("POST", "/setup/decide") if first_run(node) => {
            let host = header(request.head, "host").unwrap_or_default();
            if header(request.head, "origin")
                .is_some_and(|origin| origin != format!("http://{host}"))
            {
                return bare("403 Forbidden");
            }
            let form = String::from_utf8_lossy(request.body);
            let field = |name: &str| {
                form.split('&')
                    .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
                    .unwrap_or_default()
                    .to_string()
            };
            if let Err(message) = pair_http::decide(node, &field("id"), field("approve") == "1") {
                eprintln!("ob-node: setup page: {message}");
            }
            redirect_home()
        }
        _ => bare("404 Not Found"),
    }
}

fn page_code(node: &Node) -> Option<Code> {
    let live = node.remote.pairing().live(Instant::now());
    let mut inner = node.remote.lan.inner();
    if let Some(code) = inner
        .page_code
        .as_ref()
        .and_then(|code| live.iter().find(|live| live.id == code.id && !live.taken))
    {
        return Some(code.clone());
    }
    drop(inner);
    let code = node.remote.issue_code(None)?;
    inner = node.remote.lan.inner();
    inner.page_code = Some(code.clone());
    Some(code)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn page(node: &Node) -> Vec<u8> {
    let Some(code) = page_code(node) else {
        return bare("409 Conflict");
    };
    let link = node.remote.link_for(&code);
    let code_words = words(&code.secret).join(" ");
    let pending: String = if first_run(node) {
        node.remote
            .pairing()
            .pending(Instant::now())
            .into_iter()
            .filter(|(request, _)| request.state == State::Pending)
            .map(|(request, _)| {
                let (sas, names) = pair_http::sas(node, &request);
                format!(
                    "<form method=post action=/setup/decide class=req><p><b>{}</b> ({}) wants to pair.</p>\
                     <p class=sas title=\"{}\">{}</p><p>Approve only if your device shows the same four pictures.</p>\
                     <input type=hidden name=id value={}><button name=approve value=1>Approve</button> \
                     <button name=approve value=0 class=quiet>Reject</button></form>",
                    escape(&request.fields.name),
                    escape(&request.fields.platform),
                    names.join(", "),
                    sas.join(" "),
                    escape(&request.id),
                )
            })
            .collect()
    } else {
        "<p class=hint>Approve new devices in the app on a device you already paired.</p>".into()
    };
    let html = format!(
        "<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content=\"width=device-width\">\
         <meta http-equiv=refresh content=5><title>Set up your OpenBazaar home node</title><style>{STYLE}</style></head><body><main>\
         <h1>Pair a phone or computer</h1><p>In the OpenBazaar app choose <b>Connect to my home node</b> and scan this code, or type the eight words.</p>\
         <div class=qr>{qr}</div><p class=words>{words}</p><p class=hint>One use. It expires in {mins} minutes. This page is only on your home network.</p>\
         {pending}</main></body></html>",
        qr = qr_svg(&link),
        words = escape(&code_words),
        mins = code.expires_in(Instant::now()).as_secs().div_ceil(60),
    );
    let mut response =
        http_response_cors("200 OK", "text/html; charset=utf-8", html.as_bytes(), None);
    insert_headers(
        &mut response,
        "Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; img-src data:; form-action 'self'; frame-ancestors 'none'\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n",
    );
    response
}

fn insert_headers(response: &mut Vec<u8>, headers: &str) {
    if let Some(end) = response.windows(4).position(|window| window == b"\r\n\r\n") {
        response.splice(end + 2..end + 2, headers.bytes());
    }
}

fn redirect_home() -> Vec<u8> {
    b"HTTP/1.1 303 See Other\r\nLocation: /\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        .to_vec()
}

fn bare(status: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
}

const STYLE: &str = "body{margin:0;background:#0b1f3a;color:#e8eefc;font:16px/1.5 -apple-system,system-ui,sans-serif}\
main{max-width:30rem;margin:0 auto;padding:2rem 1.25rem}h1{font-size:1.5rem;margin:0 0 .5rem}\
.qr{background:#fff;border-radius:12px;padding:12px;width:240px;margin:1.25rem 0}.qr svg{width:100%;height:auto;display:block}\
.words{font:600 1.2rem/1.6 ui-monospace,monospace;letter-spacing:.02em;color:#fff}.hint{color:#9fb3d9;font-size:.9rem}\
.req{background:#13305a;border:1px solid #2f6fed;border-radius:12px;padding:1rem;margin-top:1.25rem}.sas{font-size:2.5rem;margin:.25rem 0}\
button{background:#2f6fed;color:#fff;border:0;border-radius:8px;padding:.6rem 1.1rem;font-size:1rem}button.quiet{background:transparent;color:#9fb3d9}";
