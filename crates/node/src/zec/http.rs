//! Zcash routes. All need a loopback caller with the pairing token.
//!
//! | route | lock | body | answer |
//! |---|---|---|---|
//! | `GET /zec/stores` | any | | which stores accept Zcash |
//! | `PUT /zec/stores` | any | `{seller, accept}` | the same, after the demo store's record is signed again |
//!
//! The own store answers 409 for now (see the module docs). Order amounts
//! are zatoshi as decimal strings (`"amountZatoshi"`), with the same amount
//! in ZEC alongside (`"amountZec"`). Paying from the node comes in phase 3.

use serde_json::{json, Map, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::zec::{AcceptError, OWN_STORE_LATER};
use crate::Node;

const MAX_BODY: usize = 4 * 1024;

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

pub(crate) fn response(node: &Node, client: &Client, head: &str, method: &str, path: &str, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    match (method, &path["/zec".len()..]) {
        ("GET", "/stores") => json_body("200 OK", &node.zec_stores_value(), origin),
        ("PUT", "/stores") => match read_body(body, &["seller", "accept"], origin) {
            Ok(map) => {
                let (Some(seller), Some(accept)) = (map.get("seller").and_then(Value::as_str), map.get("accept").and_then(Value::as_bool)) else {
                    return json_error("400 Bad Request", "Send {\"seller\": NAME, \"accept\": true or false}.", origin);
                };
                match node.set_accepts_zec(seller, accept) {
                    Ok(()) => json_body("200 OK", &node.zec_stores_value(), origin),
                    Err(AcceptError::Unknown(message)) => json_error("404 Not Found", &message, origin),
                    Err(AcceptError::OwnStore) => json_error("409 Conflict", OWN_STORE_LATER, origin),
                    Err(AcceptError::Failed(detail)) => {
                        eprintln!("ob-node: {detail}");
                        json_error("500 Internal Server Error", "The store could not be signed again.", origin)
                    }
                }
            }
            Err(response) => response,
        },
        _ => json_error("404 Not Found", "There is no such Zcash route.", origin),
    }
}
