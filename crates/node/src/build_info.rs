//! Which build of ob-node this is, from `scripts/build_info.rs` at compile
//! time. `ob-node --version`, the startup log, and `GET /version` all read it.

use serde_json::{json, Value};

use crate::http::{json_body, local_guard, Client};
use crate::{NetworkMode, Node};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: &'static str,
    /// First 7 hex digits of the commit, or "unknown".
    pub commit: &'static str,
    /// Tracked files differed from the commit.
    pub dirty: bool,
    /// UTC, e.g. 2026-01-31T12:00:00Z.
    pub build_date: &'static str,
    /// "release", "ci", or "dev".
    pub build_kind: &'static str,
}

pub const BUILD: BuildInfo = BuildInfo {
    version: env!("CARGO_PKG_VERSION"),
    commit: env!("OB_BUILD_COMMIT"),
    dirty: matches!(env!("OB_BUILD_DIRTY").as_bytes(), b"true"),
    build_date: env!("OB_BUILD_DATE"),
    build_kind: env!("OB_BUILD_KIND"),
};

impl BuildInfo {
    /// `0.1.0 (a1b2c3d-dirty, dev build, 2026-01-31T12:00:00Z)`
    pub fn summary(&self) -> String {
        let dirty = if self.dirty { "-dirty" } else { "" };
        format!("{} ({}{dirty}, {} build, {})", self.version, self.commit, self.build_kind, self.build_date)
    }

    pub fn value(&self, network: NetworkMode) -> Value {
        json!({
            "version": self.version,
            "commit": self.commit,
            "dirty": self.dirty,
            "buildDate": self.build_date,
            "buildKind": self.build_kind,
            "network": network.as_str(),
        })
    }
}

/// `GET /version`, for the paired shop only.
pub(crate) fn version_response(node: &Node, client: &Client, head: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    json_body("200 OK", &BUILD.value(node.network()), origin)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::BUILD;
    use crate::harness::TempDir;
    use crate::http::{respond, Client};
    use crate::{load_catalog, NetworkMode, Node};

    fn get(node: &Node, headers: &str) -> (String, Value) {
        let raw = format!("GET /version HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n{headers}\r\n");
        let response = String::from_utf8(respond(node, raw.as_bytes(), Client { loopback: true, port: 8787 })).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        (head.lines().next().unwrap().to_string(), serde_json::from_str(body).unwrap_or(Value::Null))
    }

    #[test]
    fn version_reports_this_build_to_the_paired_shop() {
        let dir = TempDir::new();
        let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
        let (status, body) = get(&node, &format!("X-Ob-Pair: {}\r\n", node.pairing_token()));
        assert!(status.contains("200"), "{status}");
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(body["commit"], BUILD.commit);
        assert_eq!(body["dirty"], BUILD.dirty);
        assert_eq!(body["buildDate"], BUILD.build_date);
        assert_eq!(body["buildKind"], BUILD.build_kind);
        assert_eq!(body["network"], "testnet4");
        let keys: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys.len(), 6, "{keys:?}");
    }

    #[test]
    fn version_needs_the_pairing_token() {
        let dir = TempDir::new();
        let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
        for headers in ["", "X-Ob-Pair: 00\r\n"] {
            let (status, body) = get(&node, headers);
            assert!(status.contains("403"), "{status}");
            assert!(body.get("version").is_none());
        }
    }

    #[test]
    fn build_metadata_is_well_formed() {
        assert!(BUILD.commit == "unknown" || (BUILD.commit.len() == 7 && BUILD.commit.bytes().all(|b| b.is_ascii_hexdigit())));
        assert!(["release", "ci", "dev"].contains(&BUILD.build_kind));
        assert_eq!(BUILD.build_date.len(), "2026-01-31T12:00:00Z".len());
        assert!(BUILD.build_date.ends_with('Z'));
        assert!(BUILD.summary().starts_with(&format!("{} ({}", BUILD.version, BUILD.commit)));
    }
}
