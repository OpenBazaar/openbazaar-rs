//! Routes a paired device may not call, even with a valid signature.
//!
//! A device session manages the store, but it is not the node's machine:
//! it cannot read the pairing token, reveal the backup phrase, unlock with a
//! remembered key, turn that on, relay to another home node, open the LAN
//! setup page, or post peer mail. Every spend still needs the wallet
//! passphrase and a one-time spend ticket, exactly as on the node's own
//! machine.

use serde_json::Value;

pub(crate) fn refusal(method: &str, path: &str, body: &[u8]) -> Option<&'static str> {
    let path = path.split('?').next().unwrap_or(path);
    let flag = |name: &str| {
        serde_json::from_slice::<Value>(body)
            .ok()
            .is_some_and(|value| value.get(name) == Some(&Value::Bool(true)))
    };
    match (method, path) {
        (_, "/pair") => Some("The pairing token stays on the node's machine."),
        (_, path) if path == "/home" || path.starts_with("/home/") => {
            Some("A device cannot reach another home node through this one.")
        }
        ("POST", "/wallet/phrase") => {
            Some("The backup phrase is shown only on the node's own machine.")
        }
        ("POST", "/escrow/mail") => Some("Escrow mail comes from peer nodes, not from devices."),
        ("POST", "/remote/lan-page") => {
            Some("The LAN setup page is opened on the node's own machine.")
        }
        ("POST", "/unlock") if flag("remembered") => Some(
            "Type the wallet passphrase. A remembered unlock works only on the node's own machine.",
        ),
        ("PUT", "/lock/settings") if flag("rememberDevice") => {
            Some("Remembering the passphrase can be turned on only on the node's own machine.")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::refusal;

    #[test]
    fn device_sessions_stop_short_of_the_machine() {
        for (method, path, body) in [
            ("POST", "/pair", "{}"),
            ("GET", "/home", ""),
            ("POST", "/home/proxy/catalog", ""),
            ("POST", "/wallet/phrase", "{}"),
            ("POST", "/escrow/mail", ""),
            ("POST", "/remote/lan-page", "{}"),
            ("POST", "/unlock", r#"{"remembered":true}"#),
            (
                "PUT",
                "/lock/settings",
                r#"{"passphrase":"x","rememberDevice":true}"#,
            ),
        ] {
            assert!(
                refusal(method, path, body.as_bytes()).is_some(),
                "{method} {path}"
            );
        }
        for (method, path, body) in [
            ("GET", "/catalog", ""),
            ("POST", "/unlock", r#"{"passphrase":"x"}"#),
            (
                "PUT",
                "/lock/settings",
                r#"{"passphrase":"x","autoLockMinutes":5}"#,
            ),
            ("POST", "/spend-tickets", "{}"),
            ("GET", "/homepage", ""),
        ] {
            assert!(
                refusal(method, path, body.as_bytes()).is_none(),
                "{method} {path}"
            );
        }
    }
}
