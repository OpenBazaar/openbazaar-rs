//! Checks every request passes before routing.
//!
//! **Host.** A DNS-rebinding page reaches this listener under its own name
//! (`Host: evil.example:PORT`), so only the loopback names are served:
//! `127.0.0.1`, `localhost`, and `[::1]`, with this node's port or no port.
//! A name an attacker controls never matches, whatever it resolves to.
//!
//! **Origin.** A browser names the page that sent a request. Only the desktop
//! webview origins, the configured shop origin, and this listener's own
//! loopback origin (the shop it serves to a browser on this machine) may
//! call. Any other `Origin`, including `null`, is refused before the route
//! runs, so a page cannot cause side effects even with a request whose
//! answer it cannot read.
//! A request without `Origin` is not from a page (peer nodes, the CLI) and
//! still needs the pairing token wherever the route asks for it.

use crate::http_parse::{has_header, header};

pub(crate) const DEFAULT_SHOP_ORIGIN: &str = "http://127.0.0.1:4173";
/// The Tauri desktop webview: macOS and Linux use the custom scheme, and
/// Windows WebView2 serves the same page from an http or https host.
pub(crate) const DESKTOP_ORIGINS: [&str; 3] = ["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost"];

/// True when the request names this listener by a loopback name.
pub(crate) fn host_allowed(head: &str, port: u16) -> bool {
    let Some(host) = header(head, "host") else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let (name, host_port) = match host.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((name, tail)) => (name, tail),
            None => return false,
        },
        None => match host.rfind(':') {
            Some(colon) => host.split_at(colon),
            None => (host.as_str(), ""),
        },
    };
    let port_ok = host_port.is_empty() || host_port == format!(":{port}");
    port_ok && matches!(name, "127.0.0.1" | "localhost" | "::1")
}

pub(crate) enum OriginCheck {
    /// No `Origin` header: not a browser page.
    Absent,
    /// An allowed page. Its origin goes back in `Access-Control-Allow-Origin`.
    Allowed(String),
    Refused,
}

pub(crate) fn check_origin(head: &str, shop_origin: &str) -> OriginCheck {
    if !has_header(head, "origin") {
        return OriginCheck::Absent;
    }
    match header(head, "origin") {
        Some(origin) if origin == shop_origin || DESKTOP_ORIGINS.contains(&origin) => {
            OriginCheck::Allowed(origin.to_string())
        }
        _ => OriginCheck::Refused,
    }
}

/// Accept only an `http://` origin on a loopback name with an explicit port.
pub(crate) fn parse_shop_origin(text: &str) -> Result<String, String> {
    let text = text.trim().trim_end_matches('/');
    let refused = || format!("--shop-origin must look like http://127.0.0.1:4173, not {text:?}");
    let authority = text.strip_prefix("http://").ok_or_else(refused)?;
    let (name, port) = match authority.strip_prefix('[') {
        Some(rest) => rest.split_once("]:").ok_or_else(refused)?,
        None => authority.split_once(':').ok_or_else(refused)?,
    };
    if !matches!(name, "127.0.0.1" | "localhost" | "::1") || port.parse::<u16>().map_or(true, |port| port == 0) {
        return Err(refused());
    }
    Ok(text.to_string())
}

/// Compare a presented secret with the real one in time that does not depend
/// on where they first differ.
pub(crate) fn secret_eq(presented: Option<&str>, secret: &str) -> bool {
    let Some(presented) = presented else {
        return false;
    };
    let (a, b) = (presented.as_bytes(), secret.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(host: &str) -> String {
        format!("GET / HTTP/1.1\r\nHost: {host}")
    }

    #[test]
    fn only_loopback_names_on_this_port_are_served() {
        for host in ["127.0.0.1:8787", "localhost:8787", "LOCALHOST:8787", "[::1]:8787", "127.0.0.1", "[::1]"] {
            assert!(host_allowed(&head(host), 8787), "{host}");
        }
        for host in [
            "evil.example:8787",
            "127.0.0.1.evil.example:8787",
            "localhost.evil.example",
            "127.0.0.1:8788",
            "127.0.0.1:",
            "[::1]8787",
            "[::1",
            "0.0.0.0:8787",
            "127.0.0.2:8787",
            "",
        ] {
            assert!(!host_allowed(&head(host), 8787), "{host}");
        }
        assert!(!host_allowed("GET / HTTP/1.1", 8787), "no Host");
        assert!(!host_allowed("GET / HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nHost: evil.example", 8787));
    }

    #[test]
    fn only_the_shop_and_desktop_origins_pass() {
        let shop = "http://127.0.0.1:4173";
        let with = |origin: &str| format!("GET / HTTP/1.1\r\nOrigin: {origin}");
        assert!(matches!(check_origin("GET / HTTP/1.1", shop), OriginCheck::Absent));
        for origin in [shop, "tauri://localhost", "http://tauri.localhost", "https://tauri.localhost"] {
            assert!(matches!(check_origin(&with(origin), shop), OriginCheck::Allowed(_)), "{origin}");
        }
        for origin in ["null", "https://evil.example", "http://127.0.0.1:8787", "tauri://localhost.evil.example", ""] {
            assert!(matches!(check_origin(&with(origin), shop), OriginCheck::Refused), "{origin}");
        }
    }

    #[test]
    fn shop_origins_are_loopback_http_with_a_port() {
        assert_eq!(parse_shop_origin("http://localhost:19006/").unwrap(), "http://localhost:19006");
        assert!(parse_shop_origin("http://[::1]:4173").is_ok());
        for bad in ["https://evil.example", "http://127.0.0.1", "http://127.0.0.1:0", "127.0.0.1:4173", "http://10.0.0.2:4173"] {
            assert!(parse_shop_origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn secrets_compare_whole() {
        assert!(secret_eq(Some("abcd"), "abcd"));
        assert!(!secret_eq(Some("abce"), "abcd"));
        assert!(!secret_eq(Some("abc"), "abcd"));
        assert!(!secret_eq(None, "abcd"));
    }
}
