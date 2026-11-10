//! `--web-listen` and `--web-origin`: the admin gateway on the open web.
//!
//! A node on a host with no screen of its own (a moderator on a hosting
//! service) is managed from an ordinary browser at one `https://` address.
//! The node speaks plain HTTP here: the host's proxy ends TLS and says so
//! with `X-Forwarded-Proto`. Use it only behind such a proxy. The proxy and
//! the host see every request and answer, where the admin onion shows them
//! to nobody.
//!
//! The same signed requests as on the admin onion are accepted, from the
//! same device list, under the same [`super::policy`]. Pairing is not
//! offered here. A browser signs in with a passkey ([`super::passkeys`]), or
//! with an access key: one a paired device made, or the one the node was
//! started with ([`Node::allow_web_key`]). A new node started without one
//! belongs to the first browser that creates a passkey here.

use super::auth::Signer;
use super::devices::NewDevice;
use super::protocol::decode_base32;
use super::{now_secs, request_target};
use crate::http::{http_response_cors, json_error};
use crate::http_parse::header;
use crate::Node;

const ACCESS_KEY_PREFIX: &str = "obkey-";

/// The one address browsers use for the web gateway.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebOrigin {
    origin: String,
    host: String,
    https: bool,
}

impl WebOrigin {
    /// `https://HOST[:PORT]`, or `http://` for a loopback host (tests).
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let origin = text.trim_end_matches('/').to_ascii_lowercase();
        let (https, host) = match (origin.strip_prefix("https://"), origin.strip_prefix("http://")) {
            (Some(host), _) => (true, host),
            (_, Some(host)) => (false, host),
            _ => return Err(format!("--web-origin must look like https://node.example.com, not {text:?}")),
        };
        let named = !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b));
        if !named {
            return Err(format!("--web-origin must look like https://node.example.com, not {text:?}"));
        }
        let name = host.split(':').next().unwrap_or(host);
        if !https && name != "127.0.0.1" && name != "localhost" {
            return Err("--web-origin must be https. Plain http is accepted only for 127.0.0.1 and localhost.".into());
        }
        Ok(Self {
            host: host.to_string(),
            origin,
            https,
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.origin
    }

    /// The WebAuthn relying party id of this address: its host name.
    pub(crate) fn rp_id(&self) -> &str {
        self.host.split(':').next().unwrap_or(&self.host)
    }

    /// The answer to a request that did not come to this origin over its
    /// scheme, or from a page of another origin.
    pub(crate) fn refusal(&self, method: &str, head: &str) -> Option<Vec<u8>> {
        let host = header(head, "host").map(str::to_ascii_lowercase);
        if host.as_deref() != Some(&self.host) {
            return Some(http_response_cors(
                "421 Misdirected Request",
                "text/plain",
                b"This is not the address of this node.",
                None,
            ));
        }
        if self.https && header(head, "x-forwarded-proto") != Some("https") {
            if method != "GET" {
                return Some(json_error("400 Bad Request", "Reach this node over https.", None));
            }
            let target = request_target(head)
                .filter(|target| target.starts_with('/'))
                .unwrap_or("/");
            return Some(
                format!(
                    "HTTP/1.1 308 Permanent Redirect\r\nLocation: {}{target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    self.origin
                )
                .into_bytes(),
            );
        }
        if header(head, "origin").is_some_and(|origin| origin != self.origin) {
            return Some(json_error("403 Forbidden", "That page may not call this node.", None));
        }
        None
    }
}

fn parse_access_key(text: &str) -> Option<[u8; 32]> {
    let body = text.trim().strip_prefix(ACCESS_KEY_PREFIX)?;
    decode_base32(body)?.try_into().ok()
}

impl Node {
    /// Let the holder of `access_key` (`obkey-` and 52 letters) sign in on
    /// the web page. The key joins the device list unless it is there
    /// already, so removing it lasts only while the node is started without it.
    pub fn allow_web_key(&self, access_key: &str) -> Result<(), String> {
        let signer = parse_access_key(access_key)
            .and_then(|secret| Signer::from_secret(&secret))
            .ok_or("The web access key must be obkey- followed by 52 letters and digits.")?;
        let mut devices = self.remote.devices();
        if devices.signer(&signer.id()).is_some() {
            return Ok(());
        }
        let new = NewDevice {
            key: signer.key(),
            watch: None,
            discovery: None,
            name: "Web access key".into(),
            platform: "browser".into(),
            paired_by: None,
        };
        devices.add(new, now_secs()).map(|_| ())
    }
}
