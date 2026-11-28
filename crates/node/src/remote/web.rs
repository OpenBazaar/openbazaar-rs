//! The web shop export (`expo export --platform web`), served to remote
//! browsers from `--web-dir`.
//!
//! Files are public: they are the same app anyone can build. Only names made
//! of letters, digits, `-`, `_`, `.`, and `/` are served, never a `..`
//! segment, a dot file, or a symlink out of the directory. `index.html`
//! gets a `<meta name="ob-home-node">` tag so the page knows to sign its
//! requests, and a CSP that lets it load only from this origin.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use flate2::write::GzEncoder;
use flate2::Compression;

use crate::http_parse;

const MAX_FILE: u64 = 16 * 1024 * 1024;
const MAX_GZIP_CACHE_ENTRIES: usize = 128;
const MARKER: &str = r#"<meta name="ob-home-node" content="1">"#;
const LOCAL_MARKER: &str = r#"<meta name="ob-local-shop" content="1">"#;
const PAGE_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
/// The onion market fetches other stores' public catalogs in the browser.
const BARE_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self' http: https:; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

#[derive(Hash, PartialEq, Eq)]
struct GzipCacheKey {
    path: PathBuf,
    marker: Option<&'static str>,
    len: u64,
    modified: Option<SystemTime>,
}

fn gzip_cache() -> &'static Mutex<HashMap<GzipCacheKey, Vec<u8>>> {
    static CACHE: OnceLock<Mutex<HashMap<GzipCacheKey, Vec<u8>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn accepts_gzip(head: &str) -> bool {
    let Some(value) = http_parse::header(head, "accept-encoding") else {
        return false;
    };
    value.split(',').any(|item| {
        let mut parts = item.trim().split(';');
        let encoding = parts.next().unwrap_or("").trim();
        if !encoding.eq_ignore_ascii_case("gzip") {
            return false;
        }
        // Respect explicit q=0 refusal.
        !parts.any(|param| {
            let param = param.trim();
            param
                .strip_prefix("q=")
                .or_else(|| param.strip_prefix("Q="))
                .and_then(|q| q.trim().parse::<f32>().ok())
                .is_some_and(|q| q <= 0.0)
        })
    })
}

fn is_compressible(content_type: &str) -> bool {
    matches!(
        content_type,
        "text/html; charset=utf-8"
            | "text/javascript; charset=utf-8"
            | "text/css; charset=utf-8"
            | "application/json"
            | "image/svg+xml"
    )
}

fn gzip_bytes(body: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(body).ok()?;
    encoder.finish().ok()
}

pub(crate) fn response(dir: &Path, path: &str, accept_gzip: bool) -> Option<Vec<u8>> {
    page(dir, path, Some(MARKER), PAGE_CSP, accept_gzip)
}

/// The same export for a browser on this machine. No home-node marker, so
/// the page stays in the local shop and uses the localhost cookie.
pub(crate) fn local_page(dir: &Path, path: &str, accept_gzip: bool) -> Option<Vec<u8>> {
    page(dir, path, Some(LOCAL_MARKER), PAGE_CSP, accept_gzip)
}

/// The shop page on the public store onion. No home-node or local-shop
/// marker: the page is not signed in, and it reads other stores directly.
pub(crate) fn bare_page(dir: &Path, path: &str, accept_gzip: bool) -> Option<Vec<u8>> {
    page(dir, path, None, BARE_CSP, accept_gzip)
}

fn page(
    dir: &Path,
    path: &str,
    marker: Option<&'static str>,
    csp: &str,
    accept_gzip: bool,
) -> Option<Vec<u8>> {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let name = if path == "/" {
        "index.html"
    } else {
        path.strip_prefix('/')?
    };
    let safe = name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
        && name
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.'));
    if !safe {
        return None;
    }
    let root = fs::canonicalize(dir).ok()?;
    let file = fs::canonicalize(root.join(name)).ok()?;
    let meta = fs::metadata(&file).ok()?;
    if !file.starts_with(&root) || !file.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    let content_type = content_type(name);
    let compressible = is_compressible(content_type);
    let use_gzip = accept_gzip && compressible;

    let body = if use_gzip {
        let key = GzipCacheKey {
            path: file.clone(),
            marker: (name == "index.html").then_some(marker).flatten(),
            len: meta.len(),
            modified: meta.modified().ok(),
        };
        let cached = gzip_cache()
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .get(&key)
            .cloned();
        if let Some(compressed) = cached {
            compressed
        } else {
            let mut raw = fs::read(&file).ok()?;
            if name == "index.html" {
                if let Some(marker) = marker {
                    let page = String::from_utf8(raw).ok()?;
                    raw = page
                        .replacen("<head>", &format!("<head>{marker}"), 1)
                        .into_bytes();
                }
            }
            let compressed = gzip_bytes(&raw)?;
            let mut cache = gzip_cache().lock().unwrap_or_else(|err| err.into_inner());
            if cache.len() >= MAX_GZIP_CACHE_ENTRIES {
                cache.clear();
            }
            cache.insert(key, compressed.clone());
            compressed
        }
    } else {
        let mut raw = fs::read(&file).ok()?;
        if name == "index.html" {
            if let Some(marker) = marker {
                let page = String::from_utf8(raw).ok()?;
                raw = page
                    .replacen("<head>", &format!("<head>{marker}"), 1)
                    .into_bytes();
            }
        }
        raw
    };

    let cache = if name.starts_with("_expo/static/") {
        "public, max-age=31536000, immutable"
    } else if name.starts_with("goods/") || name.starts_with("brand/") {
        "public, max-age=86400"
    } else {
        "no-cache"
    };
    let encoding_headers = if use_gzip {
        "Content-Encoding: gzip\r\nVary: Accept-Encoding\r\n"
    } else if compressible {
        "Vary: Accept-Encoding\r\n"
    } else {
        ""
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{encoding_headers}Connection: close\r\nCache-Control: {cache}\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: {csp}\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nCross-Origin-Opener-Policy: same-origin\r\n\r\n",
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
    use std::io::Read;

    use flate2::read::GzDecoder;

    use super::*;
    use crate::harness::TempDir;

    #[test]
    fn serves_the_export_and_nothing_beside_it() {
        let dir = TempDir::new();
        let web = dir.0.join("web");
        fs::create_dir_all(web.join("_expo/static/js")).unwrap();
        fs::write(
            web.join("index.html"),
            "<html><head><title>x</title></head></html>",
        )
        .unwrap();
        fs::write(web.join("_expo/static/js/app.js"), "1").unwrap();
        fs::write(web.join(".env"), "secret").unwrap();
        fs::write(dir.0.join("vault.key"), "secret").unwrap();
        let page = String::from_utf8(response(&web, "/", false).unwrap()).unwrap();
        assert!(page.contains(MARKER) && page.contains("script-src 'self'"));
        let local = String::from_utf8(local_page(&web, "/", false).unwrap()).unwrap();
        assert!(local.contains(LOCAL_MARKER) && !local.contains("ob-home-node"));
        let bare = String::from_utf8(bare_page(&web, "/", false).unwrap()).unwrap();
        assert!(!bare.contains("ob-home-node") && !bare.contains("ob-local-shop"));
        assert!(bare.contains("connect-src 'self' http: https:"));
        assert!(!page.contains("connect-src 'self' http:"));
        let script =
            String::from_utf8(response(&web, "/_expo/static/js/app.js?v=1", false).unwrap())
                .unwrap();
        assert!(script.contains("immutable") && script.contains("text/javascript"));
        for bad in [
            "/../vault.key",
            "/.env",
            "/_expo/../../vault.key",
            "//etc/passwd",
            "/missing.js",
            "/_expo/static",
            "/a%2e%2e/b",
        ] {
            assert!(response(&web, bad, false).is_none(), "{bad}");
        }
    }

    #[test]
    fn compresses_text_assets_when_accepted_and_caches_goods_and_brand() {
        let dir = TempDir::new();
        let web = dir.0.join("web");
        fs::create_dir_all(web.join("_expo/static/js")).unwrap();
        fs::create_dir_all(web.join("goods")).unwrap();
        fs::create_dir_all(web.join("brand")).unwrap();
        let big_js = "console.log('openbazaar');\n".repeat(400);
        fs::write(web.join("_expo/static/js/bundle.js"), &big_js).unwrap();
        let jpg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46];
        fs::write(web.join("goods/mug.jpg"), &jpg).unwrap();
        fs::write(web.join("brand/logo.svg"), "<svg><rect width='10' height='10'/></svg>").unwrap();

        assert!(accepts_gzip("GET / HTTP/1.1\r\nAccept-Encoding: gzip, deflate, br\r\n"));
        assert!(!accepts_gzip("GET / HTTP/1.1\r\nAccept-Encoding: br, deflate\r\n"));
        assert!(!accepts_gzip("GET / HTTP/1.1\r\nAccept-Encoding: gzip;q=0\r\n"));
        assert!(!accepts_gzip("GET / HTTP/1.1\r\nHost: example.onion\r\n"));

        // JS bundle with Accept-Encoding: gzip is compressed and round-trips cleanly.
        let raw_resp = response(&web, "/_expo/static/js/bundle.js", true).unwrap();
        let sep = raw_resp.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = std::str::from_utf8(&raw_resp[..sep]).unwrap();
        let body = &raw_resp[sep + 4..];
        assert!(head.contains("Content-Encoding: gzip"), "{head}");
        assert!(head.contains("Vary: Accept-Encoding"), "{head}");
        assert!(head.contains("Cache-Control: public, max-age=31536000, immutable"), "{head}");
        assert!(body.len() < big_js.len() / 5, "expected strong gzip compression: {} vs {}", body.len(), big_js.len());
        let mut decoded = String::new();
        GzDecoder::new(body).read_to_string(&mut decoded).unwrap();
        assert_eq!(decoded, big_js);

        // Serving the same file again hits the cache and returns identical bytes.
        let cached_resp = response(&web, "/_expo/static/js/bundle.js", true).unwrap();
        assert_eq!(cached_resp, raw_resp);

        // goods/*.jpg gets 24h cache and is NOT gzipped (already compressed JPEG).
        let img_resp = response(&web, "/goods/mug.jpg", true).unwrap();
        let sep = img_resp.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let img_head = std::str::from_utf8(&img_resp[..sep]).unwrap();
        assert!(img_head.contains("Cache-Control: public, max-age=86400"), "{img_head}");
        assert!(!img_head.contains("Content-Encoding: gzip"), "{img_head}");
        assert_eq!(&img_resp[sep + 4..], jpg.as_slice());

        // brand/*.svg gets 24h cache AND is gzipped when accepted.
        let svg_resp = response(&web, "/brand/logo.svg", true).unwrap();
        let sep = svg_resp.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let svg_head = std::str::from_utf8(&svg_resp[..sep]).unwrap();
        assert!(svg_head.contains("Cache-Control: public, max-age=86400"), "{svg_head}");
        assert!(svg_head.contains("Content-Encoding: gzip"), "{svg_head}");
        assert!(svg_head.contains("Vary: Accept-Encoding"), "{svg_head}");
    }
}
