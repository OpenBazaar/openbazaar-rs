//! The web shop export (`expo export --platform web`), served to remote
//! browsers from `--web-dir`.
//!
//! Files are public: they are the same app anyone can build. Only names made
//! of letters, digits, `-`, `_`, `.`, and `/` are served, never a `..`
//! segment, a dot file, or a symlink out of the directory. `index.html`
//! gets a `<meta name="ob-home-node">` tag so the page knows to sign its
//! requests, and a CSP that lets it load only from this origin.

use std::fs;
use std::path::Path;

const MAX_FILE: u64 = 16 * 1024 * 1024;
const MARKER: &str = r#"<meta name="ob-home-node" content="1">"#;
const PAGE_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

pub(crate) fn response(dir: &Path, path: &str) -> Option<Vec<u8>> {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let name = if path == "/" { "index.html" } else { path.strip_prefix('/')? };
    let safe = name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
        && name.split('/').all(|part| !part.is_empty() && !part.starts_with('.'));
    if !safe {
        return None;
    }
    let root = fs::canonicalize(dir).ok()?;
    let file = fs::canonicalize(root.join(name)).ok()?;
    if !file.starts_with(&root) || !file.is_file() || fs::metadata(&file).ok()?.len() > MAX_FILE {
        return None;
    }
    let mut body = fs::read(&file).ok()?;
    let content_type = content_type(name);
    if name == "index.html" {
        let page = String::from_utf8(body).ok()?;
        body = page.replacen("<head>", &format!("<head>{MARKER}"), 1).into_bytes();
    }
    let cache = if name.starts_with("_expo/static/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: {cache}\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: {PAGE_CSP}\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nCross-Origin-Opener-Policy: same-origin\r\n\r\n",
        body.len()
    );
    let mut response = head.into_bytes();
    response.extend_from_slice(&body);
    Some(response)
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::TempDir;

    #[test]
    fn serves_the_export_and_nothing_beside_it() {
        let dir = TempDir::new();
        let web = dir.0.join("web");
        fs::create_dir_all(web.join("_expo/static/js")).unwrap();
        fs::write(web.join("index.html"), "<html><head><title>x</title></head></html>").unwrap();
        fs::write(web.join("_expo/static/js/app.js"), "1").unwrap();
        fs::write(web.join(".env"), "secret").unwrap();
        fs::write(dir.0.join("vault.key"), "secret").unwrap();
        let page = String::from_utf8(response(&web, "/").unwrap()).unwrap();
        assert!(page.contains(MARKER) && page.contains("script-src 'self'"));
        let script = String::from_utf8(response(&web, "/_expo/static/js/app.js?v=1").unwrap()).unwrap();
        assert!(script.contains("immutable") && script.contains("text/javascript"));
        for bad in ["/../vault.key", "/.env", "/_expo/../../vault.key", "//etc/passwd", "/missing.js", "/_expo/static", "/a%2e%2e/b"] {
            assert!(response(&web, bad).is_none(), "{bad}");
        }
    }
}
