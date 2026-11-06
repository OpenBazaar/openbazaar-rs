//! Wallet lock routes. Each one needs a loopback caller with the pairing
//! token; the token alone opens nothing in the spend tier.
//!
//! | route | state | body | answer |
//! |---|---|---|---|
//! | `GET /lock` | any | | lock status |
//! | `POST /setup` | setup | `{passphrase}` | `{phrase, confirmWords, lock}`, the phrase this one time |
//! | `POST /setup/confirm` | unlocked | `{words: [..]}` | lock status |
//! | `POST /unlock` | locked | `{passphrase}` or `{remembered: true}` | lock status |
//! | `POST /lock` | any | | lock status |
//! | `POST /lock/activity` | any | | lock status; restarts the auto-lock timer |
//! | `PUT /lock/settings` | unlocked | `{passphrase, autoLockMinutes?, rememberDevice?}` | lock status |
//! | `POST /lock/passphrase` | unlocked | `{current, new}` | lock status |
//! | `POST /wallet/phrase` | unlocked | `{passphrase}` | `{phrase, confirmWords}`; positions until the backup is confirmed |
//! | `POST /orders/{id}/pay/quote` | unlocked | | `{quote, ...terms}` |
//! | `POST /spend-tickets` | unlocked | `{quote, passphrase}` | `{ticket, ...terms}` |
//!
//! While locked, spend routes answer `423 Locked` with `{"code": "locked"}`,
//! or `"setup_required"` before a passphrase exists. A wrong passphrase is
//! `400` with `"code": "wrong_passphrase"`, and a try during the backoff is
//! `429` with `"code": "backoff"`; both carry `retryAfterSecs`.

use serde_json::{json, Map, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::pay_order::PayError;
use crate::spend_tickets::TicketError;
use crate::wallet_lock::{LockError, State};
use crate::Node;

pub(crate) const LOCKED_MESSAGE: &str = "The wallet is locked. Unlock it to spend or sign.";
const SETUP_MESSAGE: &str = "Set a wallet passphrase first. Until then this node does not spend or sign.";
const MAX_BODY: usize = 4 * 1024;

pub(crate) enum Route<'a> {
    Status,
    Setup,
    ConfirmBackup,
    Unlock,
    Lock,
    Activity,
    Settings,
    Passphrase,
    Phrase,
    Quote(&'a str),
    Ticket,
}

/// The 423 answer for a spend while the wallet is `state`.
pub(crate) fn locked_response(state: &str, origin: Option<&str>) -> Vec<u8> {
    let (code, message) = if state == State::Setup.as_str() {
        ("setup_required", SETUP_MESSAGE)
    } else {
        ("locked", LOCKED_MESSAGE)
    };
    json_body(
        "423 Locked",
        &json!({ "error": message, "code": code, "state": state }),
        origin,
    )
}

pub(crate) fn lock_error(err: LockError, origin: Option<&str>) -> Vec<u8> {
    match err {
        LockError::Locked(state) => locked_response(state.as_str(), origin),
        LockError::Wrong { retry_after } => json_body(
            "400 Bad Request",
            &json!({ "error": "That passphrase is wrong.", "code": "wrong_passphrase", "retryAfterSecs": retry_after }),
            origin,
        ),
        LockError::Backoff { retry_after } => json_body(
            "429 Too Many Requests",
            &json!({
                "error": format!("Too many wrong passphrases. Try again in {retry_after} seconds."),
                "code": "backoff",
                "retryAfterSecs": retry_after,
            }),
            origin,
        ),
        LockError::Invalid(message) => json_error("400 Bad Request", &message, origin),
        LockError::Failed(detail) => {
            eprintln!("ob-node: wallet lock: {detail}");
            json_error("500 Internal Server Error", "The wallet lock failed. Details are in the node log.", origin)
        }
    }
}

pub(crate) fn pay_error(err: PayError, origin: Option<&str>) -> Vec<u8> {
    match err {
        PayError::NotFound => json_error("404 Not Found", "That order is not on this node.", origin),
        PayError::Invalid(message) => json_error("400 Bad Request", &message, origin),
        PayError::Conflict(message) => json_error("409 Conflict", &message, origin),
        PayError::Chain(message) => json_error("502 Bad Gateway", &message, origin),
        PayError::Locked(state) => locked_response(state, origin),
        PayError::Failed(detail) => {
            eprintln!("ob-node: quote: {detail}");
            json_error("500 Internal Server Error", "The payment could not be quoted. Details are in the node log.", origin)
        }
    }
}

/// A JSON object with only `allowed` keys.
fn read_body(body: &[u8], allowed: &[&str], origin: Option<&str>) -> Result<Map<String, Value>, Vec<u8>> {
    if body.len() > MAX_BODY {
        return Err(json_error("400 Bad Request", "That request is too large.", origin));
    }
    let value: Value = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        serde_json::from_slice(body).map_err(|_| json_error("400 Bad Request", "The request could not be read.", origin))?
    };
    match value {
        Value::Object(map) if map.keys().all(|key| allowed.contains(&key.as_str())) => Ok(map),
        _ => Err(json_error("400 Bad Request", "That field is not part of this request.", origin)),
    }
}

fn text<'a>(map: &'a Map<String, Value>, key: &str) -> &'a str {
    map.get(key).and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn response(node: &Node, client: &Client, head: &str, route: Route<'_>, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let status = |node: &Node| json_body("200 OK", &node.lock.status_value(), origin);
    let done = |result: Result<(), LockError>| match result {
        Ok(()) => status(node),
        Err(err) => lock_error(err, origin),
    };
    match route {
        Route::Status => status(node),
        Route::Lock => {
            node.lock.lock();
            status(node)
        }
        Route::Activity => {
            node.lock.touch();
            status(node)
        }
        Route::Setup => match read_body(body, &["passphrase"], origin) {
            Ok(map) => match node.setup_wallet(text(&map, "passphrase")) {
                Ok(value) => json_body("200 OK", &value, origin),
                Err(err) => lock_error(err, origin),
            },
            Err(response) => response,
        },
        Route::ConfirmBackup => match read_body(body, &["words"], origin) {
            Ok(map) => {
                let words: Vec<String> = map
                    .get("words")
                    .and_then(Value::as_array)
                    .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                done(node.confirm_backup(&words))
            }
            Err(response) => response,
        },
        Route::Unlock => match read_body(body, &["passphrase", "remembered"], origin) {
            Ok(map) if map.get("remembered") == Some(&Value::Bool(true)) => done(node.unlock_wallet(None)),
            Ok(map) => done(node.unlock_wallet(Some(text(&map, "passphrase")))),
            Err(response) => response,
        },
        Route::Settings => match read_body(body, &["passphrase", "autoLockMinutes", "rememberDevice"], origin) {
            Ok(map) => done(change_settings(node, &map)),
            Err(response) => response,
        },
        Route::Passphrase => match read_body(body, &["current", "new"], origin) {
            Ok(map) => done(node.lock.change_passphrase(text(&map, "current"), text(&map, "new"))),
            Err(response) => response,
        },
        Route::Phrase => match read_body(body, &["passphrase"], origin) {
            Ok(map) => match node.reveal_phrase(text(&map, "passphrase")) {
                Ok(phrase) => {
                    let confirm = if node.lock.status_value()["backupConfirmed"] == true {
                        Vec::new()
                    } else {
                        node.lock.confirm_positions()
                    };
                    json_body("200 OK", &json!({ "phrase": phrase.as_str(), "confirmWords": confirm }), origin)
                }
                Err(err) => lock_error(err, origin),
            },
            Err(response) => response,
        },
        Route::Quote(id) => match node.quote_payment(id) {
            Ok((quote, terms)) => {
                let mut value = terms.value(node.network.as_str());
                value["quote"] = json!(quote);
                json_body("200 OK", &value, origin)
            }
            Err(err) => pay_error(err, origin),
        },
        Route::Ticket => match read_body(body, &["quote", "passphrase"], origin) {
            Ok(map) => issue_ticket(node, &map, origin),
            Err(response) => response,
        },
    }
}

/// Every setting change needs the passphrase again, and an unlocked wallet.
fn change_settings(node: &Node, map: &Map<String, Value>) -> Result<(), LockError> {
    let minutes = match map.get("autoLockMinutes") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|minutes| u32::try_from(minutes).ok())
                .ok_or_else(|| LockError::Invalid("autoLockMinutes must be a whole number.".into()))?,
        ),
    };
    let remember = match map.get("rememberDevice") {
        None => None,
        Some(Value::Bool(on)) => Some(*on),
        Some(_) => return Err(LockError::Invalid("rememberDevice must be true or false.".into())),
    };
    node.lock.verify(text(map, "passphrase"))?;
    if let Some(minutes) = minutes {
        node.lock.set_auto_lock(minutes)?;
    }
    if let Some(on) = remember {
        node.lock.set_remember(on)?;
    }
    Ok(())
}

fn issue_ticket(node: &Node, map: &Map<String, Value>, origin: Option<&str>) -> Vec<u8> {
    if let Err(err) = node.lock.verify(text(map, "passphrase")) {
        return lock_error(err, origin);
    }
    match node.lock.tickets.issue(text(map, "quote"), node.lock.now()) {
        Ok((ticket, terms)) => {
            let mut value = terms.value(node.network.as_str());
            value["ticket"] = json!(ticket);
            json_body("200 OK", &value, origin)
        }
        Err(TicketError::Unknown | TicketError::OtherOrder) => json_error(
            "409 Conflict",
            "That quote is used or expired. Ask for a new one.",
            origin,
        ),
    }
}
