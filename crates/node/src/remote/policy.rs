//! Routes a paired device may not call, even with a valid signature.
//!
//! A device session manages the store, but it is not the node's machine:
//! it cannot read the pairing token, reveal a backed-up phrase, unlock with a
//! remembered key, turn that on, relay to another home node, open the LAN
//! setup page, or post peer mail. Every spend still needs the wallet
//! passphrase and a one-time spend ticket, exactly as on the node's own
//! machine.
//!
//! The backup phrase is the one exception with a condition. A device may set
//! the wallet up, and setup shows it the phrase. Until the owner confirms the
//! backup, that phrase must stay readable (with the passphrase), or a node
//! with no screen of its own could never be backed up. Once confirmed, only
//! the node's machine shows it.

use serde_json::Value;

/// Why a device may not make this request, if it may not. `backup_confirmed`
/// is the wallet lock's record of the owner having written the phrase down.
pub(crate) fn refusal(method: &str, path: &str, body: &[u8], backup_confirmed: bool) -> Option<&'static str> {
    let path = path.split('?').next().unwrap_or(path);
    let flag = |name: &str| {
        serde_json::from_slice::<Value>(body)
            .ok()
            .is_some_and(|value| value.get(name) == Some(&Value::Bool(true)))
    };
    match (method, path) {
        (_, "/pair") => Some("The pairing token stays on the node's machine."),
        (_, path) if path == "/browser-session" || path.starts_with("/browser/") => {
            Some("A browser on this machine opens the shop. A paired device uses its own session.")
        }
        (_, path) if path == "/home" || path.starts_with("/home/") => {
            Some("A device cannot reach another home node through this one.")
        }
        ("POST", "/wallet/phrase") if backup_confirmed => {
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
            ("POST", "/browser-session", ""),
            (
                "GET",
                "/browser/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "",
            ),
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
                refusal(method, path, body.as_bytes(), true).is_some(),
                "{method} {path}"
            );
        }
        // Before the backup is confirmed, the device that set the wallet up can see the phrase again.
        assert!(refusal("POST", "/wallet/phrase", b"{}", false).is_none());
        assert!(refusal("POST", "/pair", b"{}", false).is_some());
        for (method, path, body) in [
            ("GET", "/catalog", ""),
            ("POST", "/unlock", r#"{"passphrase":"x"}"#),
            (
                "PUT",
                "/lock/settings",
                r#"{"passphrase":"x","autoLockMinutes":5}"#,
            ),
            ("POST", "/spend-tickets", "{}"),
            // A hosted node has no screen: its phone sets the wallet lock and rules on disputes.
            ("POST", "/setup", r#"{"passphrase":"x"}"#),
            ("POST", "/setup/confirm", r#"{"words":[]}"#),
            ("GET", "/moderator/disputes", ""),
            ("POST", "/moderator/disputes/00/rule", r#"{"favor":"buyer"}"#),
            // The same phone names the moderator and chooses the moderators its store accepts.
            ("GET", "/moderator/profile", ""),
            ("PUT", "/moderator/profile", r#"{"name":"x"}"#),
            ("GET", "/moderators", ""),
            ("POST", "/moderators", r#"{"address":"http://127.0.0.1:9"}"#),
            ("DELETE", "/moderators/00", ""),
            ("GET", "/stores/00/moderators", ""),
            ("GET", "/homepage", ""),
        ] {
            assert!(
                refusal(method, path, body.as_bytes(), true).is_none(),
                "{method} {path}"
            );
        }
    }
}
