//! A browser on this machine, signed in without the pairing token.
//!
//! The desktop app already holds the pairing token. It asks for a one-time
//! path, the system browser opens that path, and the node sets a short-lived
//! cookie then redirects to the shop. The cookie is not the pairing token
//! and not a wallet key. It lives only in this process, and only a loopback
//! request whose page is this listener may use it. A paired device on Tor
//! or the LAN never sees the path or the cookie.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use crate::http::{self, json_error, Client};
use crate::http_guard::secret_eq;
use crate::http_parse::header;
use crate::remote::REMOTE_VIA_HEADER;
use crate::Node;

const PAIR_HEADER: &str = "X-Ob-Pair";
const COOKIE: &str = "ob_local";
const TICKET_TTL: Duration = Duration::from_secs(60);
const SESSION_TTL: Duration = Duration::from_secs(8 * 60 * 60);

pub(crate) struct Browser {
    tickets: Mutex<HashMap<String, Instant>>,
    sessions: Mutex<HashMap<String, Instant>>,
}

impl Browser {
    pub(crate) fn new() -> Self {
        Self {
            tickets: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn mint_ticket(&self) -> String {
        let ticket = random_id();
        let mut tickets = self.tickets.lock().unwrap_or_else(|err| err.into_inner());
        let now = Instant::now();
        tickets.retain(|_, expiry| *expiry > now);
        tickets.insert(ticket.clone(), now + TICKET_TTL);
        ticket
    }

    fn redeem(&self, ticket: &str) -> Option<String> {
        let mut tickets = self.tickets.lock().unwrap_or_else(|err| err.into_inner());
        let expiry = tickets.remove(ticket)?;
        if expiry <= Instant::now() {
            return None;
        }
        let id = random_id();
        let mut sessions = self.sessions.lock().unwrap_or_else(|err| err.into_inner());
        sessions.retain(|_, expiry| *expiry > Instant::now());
        sessions.insert(id.clone(), Instant::now() + SESSION_TTL);
        Some(id)
    }

    fn session_ok(&self, id: &str) -> bool {
        let sessions = self.sessions.lock().unwrap_or_else(|err| err.into_inner());
        sessions
            .get(id)
            .is_some_and(|expiry| *expiry > Instant::now())
    }
}

/// The pairing token, or a localhost cookie for this listener. A request the
/// onion gateway rebuilt still needs the token: the cookie is not a device.
pub(crate) fn allows(node: &Node, client: &Client, head: &str) -> bool {
    if secret_eq(header(head, PAIR_HEADER), node.pairing_token()) {
        return true;
    }
    client.loopback && cookie_ok(node, head, client.port)
}

fn cookie_ok(node: &Node, head: &str, port: u16) -> bool {
    if header(head, REMOTE_VIA_HEADER).is_some() {
        return false;
    }
    let Some(value) = cookie_value(head) else {
        return false;
    };
    if !origin_ok(head, port) {
        return false;
    }
    node.browser.session_ok(value)
}

fn origin_ok(head: &str, port: u16) -> bool {
    match header(head, "Origin") {
        None => true,
        Some(origin) => is_own_origin(origin, port),
    }
}

/// The page served by this listener: `http://127.0.0.1:PORT` and the other
/// loopback names for the same port. No other origin.
pub(crate) fn is_own_origin(origin: &str, port: u16) -> bool {
    let own = [
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
        format!("http://[::1]:{port}"),
    ];
    own.iter().any(|item| item.eq_ignore_ascii_case(origin))
}

/// Browser-session routes, or `None` so the shop router keeps the request.
pub(crate) fn route(
    node: &Node,
    client: &Client,
    method: &str,
    path: &str,
    head: &str,
    origin: Option<&str>,
) -> Option<Vec<u8>> {
    if path == "/browser-session" {
        return Some(match method {
            "POST" => mint(node, client, head, origin),
            "GET" => session_status(node, client, head, origin),
            _ => json_error("405 Method Not Allowed", "Use GET or POST.", origin),
        });
    }
    let ticket = path.strip_prefix("/browser/")?;
    if method != "GET" {
        return Some(json_error(
            "405 Method Not Allowed",
            "Open that link in the browser.",
            origin,
        ));
    }
    Some(redeem(node, client, head, ticket, origin))
}

fn mint(node: &Node, client: &Client, head: &str, origin: Option<&str>) -> Vec<u8> {
    if !client.loopback || header(head, REMOTE_VIA_HEADER).is_some() {
        return json_error("403 Forbidden", "This stays on this machine.", origin);
    }
    if !secret_eq(header(head, PAIR_HEADER), node.pairing_token()) {
        return json_error(
            "403 Forbidden",
            "The shop is not paired with this node.",
            origin,
        );
    }
    let ticket = node.browser.mint_ticket();
    let url = format!("http://127.0.0.1:{}/browser/{ticket}", client.port);
    debug_assert!(!url.contains(node.pairing_token()));
    let body = format!("{url}\n");
    http::http_response_cors(
        "200 OK",
        "text/plain; charset=utf-8",
        body.as_bytes(),
        origin,
    )
}

fn session_status(node: &Node, client: &Client, head: &str, origin: Option<&str>) -> Vec<u8> {
    if !client.loopback || header(head, REMOTE_VIA_HEADER).is_some() {
        return json_error("403 Forbidden", "This stays on this machine.", origin);
    }
    if cookie_ok(node, head, client.port) {
        http::http_response_cors("204 No Content", "text/plain", b"", origin)
    } else {
        json_error(
            "401 Unauthorized",
            "This browser is not signed in on this machine.",
            origin,
        )
    }
}

fn redeem(node: &Node, client: &Client, head: &str, ticket: &str, origin: Option<&str>) -> Vec<u8> {
    if !client.loopback || header(head, REMOTE_VIA_HEADER).is_some() {
        return json_error("403 Forbidden", "This stays on this machine.", origin);
    }
    if !is_id(ticket) {
        return json_error("404 Not Found", "That link has expired.", origin);
    }
    let Some(id) = node.browser.redeem(ticket) else {
        return json_error("404 Not Found", "That link has expired.", origin);
    };
    let extra = format!(
        "Set-Cookie: {COOKIE}={id}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}\r\nLocation: /\r\n",
        SESSION_TTL.as_secs()
    );
    http::http_response_headers(
        "303 See Other",
        "text/plain; charset=utf-8",
        b"",
        origin,
        &extra,
    )
}

fn cookie_value(head: &str) -> Option<&str> {
    let header = header(head, "Cookie")?;
    for part in header.split(';') {
        let (name, value) = part.trim().split_once('=')?;
        if name.trim() == COOKIE && is_id(value.trim()) {
            return Some(value.trim());
        }
    }
    None
}

fn is_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn random_id() -> String {
    let mut bytes = [0u8; 32];
    StdRng::from_os_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Shop HTML when the browser asked for a page and `--local-shop` is set.
pub(crate) fn shop_page(node: &Node, head: &str) -> Option<Vec<u8>> {
    if !wants_html(head) {
        return None;
    }
    let dir = node.local.local_shop.as_deref()?;
    crate::remote::local_shop_page(dir, "/")
}

pub(crate) fn shop_file(node: &Node, method: &str, path: &str, head: &str) -> Option<Vec<u8>> {
    if method != "GET" {
        return None;
    }
    let dir = node.local.local_shop.as_deref()?;
    if let Some(file) = crate::remote::local_shop_page(dir, path) {
        return Some(file);
    }
    if wants_html(head) && !path.contains('.') {
        return crate::remote::local_shop_page(dir, "/");
    }
    None
}

fn wants_html(head: &str) -> bool {
    header(head, "Accept").is_some_and(|value| value.to_ascii_lowercase().contains("text/html"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::TempDir;
    use crate::http::respond;
    use crate::{load_catalog, NetworkMode, Node};

    fn node() -> (TempDir, Node) {
        let dir = TempDir::new();
        let catalog = load_catalog(NetworkMode::Testnet4).unwrap();
        let node = Node::open(&dir.0, catalog).unwrap();
        (dir, node)
    }

    fn call(node: &Node, raw: &str, loopback: bool) -> String {
        let response = respond(
            node,
            raw.as_bytes(),
            Client {
                loopback,
                port: 8787,
            },
        );
        String::from_utf8(response).unwrap()
    }

    fn ticket_from(response: &str, token: &str) -> String {
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(
            !response.contains(token),
            "the one-time path carried the pairing token"
        );
        let body = response.split("\r\n\r\n").nth(1).unwrap().trim();
        let ticket = body
            .strip_prefix("http://127.0.0.1:8787/browser/")
            .expect("one-time path");
        assert_eq!(ticket.len(), 64);
        assert_ne!(ticket, token);
        ticket.to_string()
    }

    #[test]
    fn a_browser_on_this_machine_gets_a_cookie_instead_of_the_pairing_token() {
        let (dir, mut node) = node();
        let shop = dir.0.join("shop");
        std::fs::create_dir(&shop).unwrap();
        node.local.local_shop = Some(shop);
        let token = node.pairing_token().to_string();
        let refused = call(
            &node,
            "POST /browser-session HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nContent-Length: 0\r\n\r\n",
            true,
        );
        assert!(refused.contains("403"), "{refused}");
        let remote = format!(
            "POST /browser-session HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nX-Ob-Pair: {token}\r\n{REMOTE_VIA_HEADER}: tor:1\r\nContent-Length: 0\r\n\r\n"
        );
        assert!(
            call(&node, &remote, true).contains("403"),
            "a device session cannot mint a local browser"
        );

        let minted = call(
            &node,
            &format!("POST /browser-session HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nX-Ob-Pair: {token}\r\nContent-Length: 0\r\n\r\n"),
            true,
        );
        let ticket = ticket_from(&minted, &token);
        let opened = call(
            &node,
            &format!("GET /browser/{ticket} HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n\r\n"),
            true,
        );
        assert!(opened.starts_with("HTTP/1.1 303"), "{opened}");
        assert!(opened.contains("Location: /\r\n"), "{opened}");
        assert!(!opened.contains(&token), "{opened}");
        let cookie = opened
            .split("\r\n")
            .find_map(|line| line.strip_prefix("Set-Cookie: ob_local="))
            .unwrap();
        let id = cookie.split(';').next().unwrap();
        assert_eq!(id.len(), 64);
        assert_ne!(id, token);
        assert_ne!(id, ticket);
        assert!(
            cookie.contains("HttpOnly")
                && cookie.contains("SameSite=Strict")
                && !cookie.contains("Domain=")
        );
        let again = call(
            &node,
            &format!("GET /browser/{ticket} HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n\r\n"),
            true,
        );
        assert!(again.contains("404"), "the path works once");

        let orders = format!(
            "GET /orders HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: http://127.0.0.1:8787\r\nCookie: ob_local={id}\r\n\r\n"
        );
        let ok = call(&node, &orders, true);
        assert!(ok.contains("200") && ok.contains("orders"), "{ok}");
        assert!(!ok.contains(&token));

        let foreign = format!(
            "GET /orders HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: http://evil.example\r\nCookie: ob_local={id}\r\n\r\n"
        );
        assert!(
            call(&node, &foreign, true).contains("403"),
            "another page cannot use the cookie"
        );
        let other_port = format!(
            "GET /orders HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: http://127.0.0.1:4173\r\nCookie: ob_local={id}\r\n\r\n"
        );
        assert!(
            call(&node, &other_port, true).contains("403"),
            "the dev shop origin is not this listener"
        );
        assert!(
            call(&node, &orders, false).contains("403"),
            "a forwarded connection is not this machine"
        );
        let via = format!(
            "GET /orders HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: http://127.0.0.1:8787\r\nCookie: ob_local={id}\r\n{REMOTE_VIA_HEADER}: tor:1\r\n\r\n"
        );
        assert!(
            call(&node, &via, true).contains("403"),
            "the onion gateway does not get the cookie"
        );

        let probe = format!(
            "GET /browser-session HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: http://127.0.0.1:8787\r\nCookie: ob_local={id}\r\n\r\n"
        );
        assert!(
            call(&node, &probe, true).starts_with("HTTP/1.1 204"),
            "the page can see that it is signed in"
        );
        assert!(call(
            &node,
            "GET /browser-session HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n\r\n",
            true
        )
        .contains("401"));
    }

    #[test]
    fn the_local_shop_page_is_this_machine_and_the_catalog_stays_json() {
        let (dir, mut node) = node();
        let shop = dir.0.join("shop");
        std::fs::create_dir_all(shop.join("_expo/static/js")).unwrap();
        std::fs::write(
            shop.join("index.html"),
            "<html><head><title>shop</title></head></html>",
        )
        .unwrap();
        std::fs::write(shop.join("_expo/static/js/app.js"), "shop").unwrap();
        node.local.local_shop = Some(shop);
        let token = node.pairing_token().to_string();

        let page = call(
            &node,
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nAccept: text/html\r\n\r\n",
            true,
        );
        assert!(
            page.contains("200") && page.contains("ob-local-shop"),
            "{page}"
        );
        assert!(
            !page.contains("ob-home-node"),
            "the local shop is not a remote device"
        );
        assert!(!page.contains(&token));
        let catalog = call(
            &node,
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n\r\n",
            true,
        );
        assert!(
            catalog.contains("200") && catalog.contains("listings"),
            "{catalog}"
        );
        let script = call(
            &node,
            "GET /_expo/static/js/app.js HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n\r\n",
            true,
        );
        assert!(
            script.contains("text/javascript") && script.contains("shop"),
            "{script}"
        );
        let seller = call(
            &node,
            "GET /store/some-seller HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nAccept: text/html\r\n\r\n",
            true,
        );
        assert!(
            seller.contains("ob-local-shop"),
            "a shop path still loads the page"
        );
        assert!(call(
            &node,
            "GET /../pairing.token HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n\r\n",
            true
        )
        .contains("404"));
    }

    #[test]
    fn local_shop_flag_needs_the_export() {
        let err =
            crate::ServeOptions::parse(&["--local-shop".to_string(), "/no/such/shop".to_string()])
                .unwrap_err();
        assert!(err.contains("index.html"), "{err}");
    }
}
