//! `tor` in `GET /network`: one answer to "is Tor working for this node?".
//!
//! * mode: `builtin` (`--tor`), `system` (`--proxy` to a Tor already running),
//!   or `none`.
//! * state: `connected` only once Tor can build circuits: built-in Tor has
//!   bootstrapped, or the proxy answers. A listening SOCKS port is not
//!   enough, since built-in Tor listens before it has bootstrapped. Tor that
//!   the health check calls cut off ([`crate::remote::tor_health`]) is
//!   `failed` too. Otherwise `connecting`, or `off` with no Tor at all.
//! * onionServices: the node's own onion services by role, and whether each
//!   is published, from the same health check.

use serde_json::{json, Value};

use crate::tor_socks::Bootstrap;

/// `built_in` is set with `--tor`; `proxy` is the `--proxy` URL and whether it
/// answers; `health` is the health summary of the onion-service client.
pub(crate) fn value(
    built_in: Option<&Bootstrap>,
    proxy: Option<(&str, bool)>,
    health: Option<&Value>,
) -> Value {
    let cut_off = health
        .filter(|health| health["state"] == "cut off")
        .map(|health| text(&health["reason"]));
    let (mode, state, progress, error) = match (built_in, proxy) {
        (Some(tor), _) if tor.ready => ("builtin", "connected", Some(100), None),
        (Some(tor), _) => (
            "builtin",
            if tor.failed { "failed" } else { "connecting" },
            Some(tor.progress),
            tor.problem.clone(),
        ),
        (None, Some((_, true))) => ("system", "connected", None, None),
        (None, Some((url, false))) => (
            "system",
            "failed",
            None,
            Some(format!(
                "the Tor proxy at {} does not answer",
                url.trim_start_matches("socks5h://")
            )),
        ),
        (None, None) => ("none", "off", None, None),
    };
    let (state, error) = match cut_off {
        Some(reason) if state == "connected" => ("failed", Some(reason)),
        _ => (state, error),
    };
    let onions: Vec<Value> = health
        .and_then(|health| health["onions"].as_array())
        .map(|onions| onions.iter().map(|onion| json!({ "name": onion["role"], "published": onion["published"] == true })).collect())
        .unwrap_or_default();
    json!({
        "mode": mode,
        "state": state,
        "progress": progress,
        "error": error,
        "onionServices": onions,
        "health": health.map(|health| json!({ "state": health["state"], "reason": health["reason"] })),
    })
}

fn text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or("Tor has lost contact with the network")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn booting(progress: u8, problem: Option<&str>, failed: bool) -> Bootstrap {
        Bootstrap {
            ready: false,
            progress,
            problem: problem.map(str::to_string),
            failed,
        }
    }

    #[test]
    fn built_in_tor_is_connected_only_once_bootstrapped() {
        let tor = value(
            Some(&booting(40, None, false)),
            Some(("socks5h://127.0.0.1:5555", true)),
            None,
        );
        assert_eq!(
            (
                tor["mode"].as_str(),
                tor["state"].as_str(),
                tor["progress"].as_u64()
            ),
            (Some("builtin"), Some("connecting"), Some(40))
        );
        let stuck = value(Some(&booting(8, Some("no internet"), true)), None, None);
        assert_eq!(
            (stuck["state"].as_str(), stuck["error"].as_str()),
            (Some("failed"), Some("no internet"))
        );
        let ready = Bootstrap {
            ready: true,
            progress: 100,
            problem: None,
            failed: false,
        };
        assert_eq!(value(Some(&ready), None, None)["state"], "connected");
    }

    #[test]
    fn a_system_proxy_is_judged_by_whether_it_answers() {
        let up = value(None, Some(("socks5h://127.0.0.1:9050", true)), None);
        assert_eq!(
            (up["mode"].as_str(), up["state"].as_str()),
            (Some("system"), Some("connected"))
        );
        let down = value(None, Some(("socks5h://127.0.0.1:9050", false)), None);
        assert_eq!(down["state"], "failed");
        assert!(down["error"].as_str().unwrap().contains("127.0.0.1:9050"));
        assert_eq!(value(None, None, None)["state"], "off");
    }

    #[test]
    fn onion_services_and_a_cut_off_come_from_the_health_check() {
        let health = json!({ "state": "cut off", "reason": "the directory ran out 4 hours ago", "onions": [
            { "role": "store", "published": true }, { "role": "lightning", "published": false } ] });
        let tor = value(
            None,
            Some(("socks5h://127.0.0.1:9050", true)),
            Some(&health),
        );
        assert_eq!(
            (tor["state"].as_str(), tor["error"].as_str()),
            (Some("failed"), Some("the directory ran out 4 hours ago"))
        );
        assert_eq!(
            tor["onionServices"],
            json!([{ "name": "store", "published": true }, { "name": "lightning", "published": false }])
        );
    }
}
