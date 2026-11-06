//! Loopback HTTP for the shop page. One request per connection.
//!
//! The shop origin (`http://127.0.0.1:4173`) and this node's own origin are
//! the only CORS callers. `GET /pair` returns the pairing token to a loopback
//! peer so the local shop can send it on `POST /orders`. A non-loopback
//! caller does not get the token. The listener is bound to 127.0.0.1; the
//! peer check refuses a connection that was forwarded from somewhere else.
//! The token itself is created in the node data dir on first boot and printed
//! once. There is no other way to name a peer from this server.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use serde_json::{json, Value};

use crate::{catalog_json, find_listing, listing_value, wallet_http, Node, OrderError};

const SHOP_ORIGIN: &str = "http://127.0.0.1:4173";
const PAIR_HEADER: &str = "X-Ob-Pair";
const MAX_REQUEST: usize = 64 * 1024;

/// Who is calling, from the node's point of view.
pub(crate) struct Client {
    pub loopback: bool,
    pub port: u16,
}

pub fn http_response(status: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
    http_response_cors(status, content_type, body, Some(SHOP_ORIGIN))
}

fn http_response_cors(
    status: &str,
    content_type: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    let allow = match origin {
        Some(origin) => format!("Access-Control-Allow-Origin: {origin}\r\n"),
        None => String::new(),
    };
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{allow}Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, {PAIR_HEADER}\r\nAccess-Control-Max-Age: 600\r\nVary: Origin\r\n\r\n",
        body.len()
    );
    let mut response = header.into_bytes();
    response.extend_from_slice(body);
    response
}

pub fn serve(node: &Node, port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    eprintln!("ob-node catalog at http://127.0.0.1:{port}/catalog");
    eprintln!("ob-node orders at http://127.0.0.1:{port}/orders");
    eprintln!("ob-node addresses at http://127.0.0.1:{port}/addresses");
    eprintln!("ob-node pair at http://127.0.0.1:{port}/pair");
    eprintln!("ob-node rates at http://127.0.0.1:{port}/rates");
    eprintln!("ob-node network at http://127.0.0.1:{port}/network");
    eprintln!("ob-node wallet at http://127.0.0.1:{port}/wallet");
    eprintln!("ob-node mailbox {}", node.mailbox_path().display());
    for connection in listener.incoming() {
        let mut stream = match connection {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("ob-node: {err}");
                continue;
            }
        };
        let loopback = stream
            .peer_addr()
            .map(|addr| addr.ip().is_loopback())
            .unwrap_or(false);
        let raw = match read_http(&mut stream) {
            Ok(raw) => raw,
            Err(err) => {
                eprintln!("ob-node: {err}");
                continue;
            }
        };
        let response = respond(node, &raw, Client { loopback, port });
        if let Err(err) = stream.write_all(&response) {
            eprintln!("ob-node: {err}");
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn dispatch(node: &Node, raw: &[u8]) -> Vec<u8> {
    respond(
        node,
        raw,
        Client {
            loopback: true,
            port: 8787,
        },
    )
}

pub(crate) fn respond(node: &Node, raw: &[u8], client: Client) -> Vec<u8> {
    let Some((head, rest)) = split_head(raw) else {
        return http_response("400 Bad Request", "text/plain", b"bad request");
    };
    let Some(line) = head.lines().next() else {
        return http_response("400 Bad Request", "text/plain", b"bad request");
    };
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or(target);
    let origin = cors_origin(header(head, "origin"), client.port);
    if method == "OPTIONS" {
        return http_response_cors("204 No Content", "text/plain", b"", origin.as_deref());
    }
    match (method, path) {
        ("GET", "/catalog") | ("GET", "/") => match catalog_json(&node.catalog) {
            Ok(json) => http_response_cors(
                "200 OK",
                "application/json",
                json.as_bytes(),
                origin.as_deref(),
            ),
            Err(err) => {
                eprintln!("ob-node: {err}");
                json_error(
                    "500 Internal Server Error",
                    "The catalog could not be read.",
                    origin.as_deref(),
                )
            }
        },
        ("GET", "/pair") => pair_response(node, &client, origin.as_deref()),
        ("GET", "/rates") => json_body("200 OK", &node.rates_value(), origin.as_deref()),
        ("GET", "/addresses") => addresses_list(node, &client, head, origin.as_deref()),
        ("POST", "/addresses") => addresses_create(node, &client, head, body_of(head, rest), origin.as_deref()),
        ("PUT", path) if path.starts_with("/addresses/") => addresses_update(
            node,
            &client,
            head,
            &path["/addresses/".len()..],
            body_of(head, rest),
            origin.as_deref(),
        ),
        ("DELETE", path) if path.starts_with("/addresses/") => {
            addresses_delete(node, &client, head, &path["/addresses/".len()..], origin.as_deref())
        }
        ("GET", "/network") => wallet_http::network_response(node, origin.as_deref()),
        ("GET", "/wallet") => wallet_http::wallet_response(node, &client, head, origin.as_deref()),
        ("POST", "/wallet/address") => {
            wallet_http::wallet_address_response(node, &client, head, origin.as_deref())
        }
        ("POST", path) if path.starts_with("/orders/") && path.ends_with("/pay") => wallet_http::pay_response(
            node,
            &client,
            head,
            &path["/orders/".len()..path.len() - "/pay".len()],
            body_of(head, rest),
            origin.as_deref(),
        ),
        ("GET", "/orders") => orders_list(node, &client, head, origin.as_deref()),
        ("GET", path) if path.starts_with("/orders/") => {
            orders_get(node, &client, head, &path["/orders/".len()..], origin.as_deref())
        }
        ("GET", path) if path.starts_with("/listings/") => {
            listing_response(node, &path["/listings/".len()..], origin.as_deref())
        }
        ("POST", "/orders") => order_response(node, head, body_of(head, rest), origin.as_deref()),
        ("POST", "/checkout") => checkout_response(node, &client, head, body_of(head, rest), origin.as_deref()),
        _ => http_response_cors(
            "404 Not Found",
            "text/plain",
            b"not found",
            origin.as_deref(),
        ),
    }
}

fn pair_response(node: &Node, client: &Client, origin: Option<&str>) -> Vec<u8> {
    // Loopback only. The shop on this machine fetches the token here and
    // sends it back as X-Ob-Pair. Anyone who is not on the loopback interface
    // gets a refusal that does not contain the token.
    if !client.loopback {
        return json_error(
            "403 Forbidden",
            "The pairing token stays on this machine.",
            origin,
        );
    }
    json_body("200 OK", &json!({ "token": node.pairing_token() }), origin)
}

fn listing_response(node: &Node, hash: &str, origin: Option<&str>) -> Vec<u8> {
    let Some(listing) = find_listing(&node.catalog, hash) else {
        return json_error("404 Not Found", "That listing is not in this shop.", origin);
    };
    match listing_value(&node.catalog, listing) {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(err) => {
            eprintln!("ob-node: {err}");
            json_error(
                "500 Internal Server Error",
                "The listing could not be read.",
                origin,
            )
        }
    }
}

fn order_response(node: &Node, head: &str, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if header(head, PAIR_HEADER) != Some(node.pairing_token()) {
        return json_error(
            "401 Unauthorized",
            "The shop is not paired with this node.",
            origin,
        );
    }
    if body.len() > 16 * 1024 {
        return json_error("400 Bad Request", "The order is too large.", origin);
    }
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return json_error("400 Bad Request", "The order could not be read.", origin),
    };
    match node.place_order(&parsed) {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(OrderError::UnknownListing) => {
            json_error("404 Not Found", "That listing is not in this shop.", origin)
        }
        Err(OrderError::Invalid(message)) => json_error("400 Bad Request", message, origin),
        Err(OrderError::Failed(detail)) => {
            eprintln!("ob-node: {detail}");
            json_error(
                "500 Internal Server Error",
                "The order could not be sealed.",
                origin,
            )
        }
    }
}

fn checkout_response(node: &Node, client: &Client, head: &str, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    if body.len() > 64 * 1024 {
        return json_error("400 Bad Request", "The order is too large.", origin);
    }
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return json_error("400 Bad Request", "The order could not be read.", origin),
    };
    match node.checkout(&parsed) {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(OrderError::UnknownListing) => {
            json_error("404 Not Found", "That listing is not in this shop.", origin)
        }
        Err(OrderError::Invalid(message)) => json_error("400 Bad Request", message, origin),
        Err(OrderError::Failed(detail)) => {
            eprintln!("ob-node: {detail}");
            json_error(
                "500 Internal Server Error",
                "The order could not be sealed.",
                origin,
            )
        }
    }
}

fn cors_origin(request_origin: Option<&str>, port: u16) -> Option<String> {
    let own = format!("http://127.0.0.1:{port}");
    match request_origin {
        Some(origin) if origin == SHOP_ORIGIN || origin == own => Some(origin.to_string()),
        Some(_) => None,
        None => Some(SHOP_ORIGIN.to_string()),
    }
}

pub(crate) fn local_guard(node: &Node, client: &Client, head: &str, origin: Option<&str>) -> Result<(), Vec<u8>> {
    if !client.loopback {
        return Err(json_error(
            "403 Forbidden",
            "This stays on this machine.",
            origin,
        ));
    }
    if header(head, PAIR_HEADER) != Some(node.pairing_token()) {
        return Err(json_error(
            "403 Forbidden",
            "The shop is not paired with this node.",
            origin,
        ));
    }
    Ok(())
}

fn book_result(result: Result<Value, String>, origin: Option<&str>) -> Vec<u8> {
    match result {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(message) if message.contains("not on this node") => {
            json_error("404 Not Found", &message, origin)
        }
        Err(message) => json_error("400 Bad Request", &message, origin),
    }
}

fn read_object(body: &[u8], origin: Option<&str>) -> Result<Value, Vec<u8>> {
    if body.len() > 8 * 1024 {
        return Err(json_error("400 Bad Request", "That request is too large.", origin));
    }
    match serde_json::from_slice(body) {
        Ok(value) => Ok(value),
        Err(_) => Err(json_error("400 Bad Request", "The address could not be read.", origin)),
    }
}

fn addresses_list(node: &Node, client: &Client, head: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let book = node.addresses.lock().unwrap_or_else(|err| err.into_inner());
    json_body("200 OK", &book.list_value(), origin)
}

fn addresses_create(node: &Node, client: &Client, head: &str, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let value = match read_object(body, origin) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut book = node.addresses.lock().unwrap_or_else(|err| err.into_inner());
    book_result(book.create(&value), origin)
}

fn addresses_update(
    node: &Node,
    client: &Client,
    head: &str,
    id: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let value = match read_object(body, origin) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut book = node.addresses.lock().unwrap_or_else(|err| err.into_inner());
    book_result(book.update(id, &value), origin)
}

fn addresses_delete(node: &Node, client: &Client, head: &str, id: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let mut book = node.addresses.lock().unwrap_or_else(|err| err.into_inner());
    book_result(book.delete(id), origin)
}

fn orders_list(node: &Node, client: &Client, head: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let book = node.buyer_orders.lock().unwrap_or_else(|err| err.into_inner());
    json_body("200 OK", &book.list_value(), origin)
}

fn orders_get(node: &Node, client: &Client, head: &str, id: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let book = node.buyer_orders.lock().unwrap_or_else(|err| err.into_inner());
    match book.get_value(id) {
        Some(value) => json_body("200 OK", &value, origin),
        None => json_error("404 Not Found", "That order is not on this node.", origin),
    }
}

pub(crate) fn json_body(status: &str, value: &Value, origin: Option<&str>) -> Vec<u8> {
    let body = serde_json::to_vec_pretty(value).unwrap_or_else(|_| b"{}".to_vec());
    http_response_cors(status, "application/json", &body, origin)
}

pub(crate) fn json_error(status: &str, message: &str, origin: Option<&str>) -> Vec<u8> {
    json_body(status, &json!({ "error": message }), origin)
}

fn split_head(raw: &[u8]) -> Option<(&str, &[u8])> {
    let split = raw.windows(4).position(|window| window == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    Some((head, &raw[split + 4..]))
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    for line in head.lines().skip(1) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(value.trim());
        }
    }
    None
}

fn body_of<'a>(head: &str, rest: &'a [u8]) -> &'a [u8] {
    match content_length(head) {
        Some(len) if len < rest.len() => &rest[..len],
        _ => rest,
    }
}

fn content_length(head: &str) -> Option<usize> {
    header(head, "content-length").and_then(|value| value.parse().ok())
}

fn read_http(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        if buf.len() > MAX_REQUEST {
            break;
        }
        if request_complete(&buf) {
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => buf.extend_from_slice(&chunk[..read]),
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                if buf.is_empty() {
                    return Err(err);
                }
                break;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(buf)
}

fn request_complete(buf: &[u8]) -> bool {
    let Some((head, rest)) = split_head(buf) else {
        return false;
    };
    match content_length(head) {
        Some(len) => rest.len() >= len,
        None => true,
    }
}
