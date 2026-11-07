//! Loopback HTTP for the shop page. One request per connection.
//!
//! Every request is parsed with hard limits ([`crate::http_parse`]), must
//! name this listener by a loopback Host, and, if a browser sent it, must
//! come from the desktop webview or the configured shop origin
//! ([`crate::http_guard`]). No route hands out the pairing token: the desktop
//! app reads it from the data dir, and a browser shop trades a one-time code
//! from the node's terminal for it at `POST /pair` ([`crate::pairing`]).
//! The listener is bound to 127.0.0.1; the peer check refuses a connection
//! that was forwarded from somewhere else. There is no other way to name a
//! peer from this server.

use std::io::Write;
use std::net::TcpListener;
use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::{json, Value};

use crate::escrow_http::{self, Action};
use crate::lock_http::{self, Route};
use crate::http_guard::{check_origin, host_allowed, secret_eq, OriginCheck, DEFAULT_SHOP_ORIGIN};
use crate::http_parse::{self, header, Request};
use crate::{find_listing, labelers, listing_value, mail_drop, media, pairing, stores, vendor_labels, wallet_http, Node, OrderError};

const PAIR_HEADER: &str = "X-Ob-Pair";
/// Every response: never cached, never sniffed or framed, no referrer, and
/// no content may load from it (the node serves no HTML).
const SECURITY_HEADERS: &str = "Cache-Control: no-store\r\nPragma: no-cache\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n";

/// Who is calling, from the node's point of view.
pub(crate) struct Client {
    pub loopback: bool,
    pub port: u16,
}

pub fn http_response(status: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
    http_response_cors(status, content_type, body, Some(DEFAULT_SHOP_ORIGIN))
}

pub(crate) fn http_response_cors(
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
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{SECURITY_HEADERS}{allow}Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, {PAIR_HEADER}\r\nAccess-Control-Max-Age: 600\r\nVary: Origin\r\n\r\n",
        body.len()
    );
    let mut response = header.into_bytes();
    response.extend_from_slice(body);
    response
}

pub fn serve(node: &Node, port: u16) -> std::io::Result<()> {
    serve_on(node, TcpListener::bind(("127.0.0.1", port))?)
}

/// Serve on a listener the caller bound (to 127.0.0.1).
pub fn serve_on(node: &Node, listener: TcpListener) -> std::io::Result<()> {
    let port = listener.local_addr()?.port();
    eprintln!("ob-node catalog at http://127.0.0.1:{port}/catalog");
    eprintln!("ob-node orders at http://127.0.0.1:{port}/orders");
    eprintln!("ob-node addresses at http://127.0.0.1:{port}/addresses");
    eprintln!("ob-node pair at http://127.0.0.1:{port}/pair");
    eprintln!("ob-node rates at http://127.0.0.1:{port}/rates");
    eprintln!("ob-node network at http://127.0.0.1:{port}/network");
    eprintln!("ob-node version at http://127.0.0.1:{port}/version");
    eprintln!("ob-node wallet at http://127.0.0.1:{port}/wallet");
    eprintln!("ob-node content labels at http://127.0.0.1:{port}/content-labels");
    eprintln!("ob-node stores at http://127.0.0.1:{port}/stores");
    eprintln!("ob-node labelers at http://127.0.0.1:{port}/labelers");
    eprintln!("ob-node mailbox {}", node.mailbox_path().display());
    std::thread::scope(|scope| {
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
            let raw = match http_parse::read(&mut stream) {
                Ok(raw) => raw,
                Err(err) => {
                    eprintln!("ob-node: {err}");
                    continue;
                }
            };
            let answer = move |raw: Vec<u8>, mut stream: std::net::TcpStream| {
                let response = catch_unwind(AssertUnwindSafe(|| respond(node, &raw, Client { loopback, port })))
                    .unwrap_or_else(|_| {
                        eprintln!("ob-node: a request handler panicked");
                        http_response_cors("500 Internal Server Error", "text/plain", b"internal error", None)
                    });
                if let Err(err) = stream.write_all(&response) {
                    eprintln!("ob-node: {err}");
                }
            };
            // A home node proxy request waits on Tor; it does not hold up the rest.
            if crate::remote::is_proxy_request(&raw) {
                scope.spawn(move || answer(raw, stream));
            } else {
                let _one = crate::remote::dispatch_lock();
                answer(raw, stream);
            }
        }
    });
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
    let request = match http_parse::parse(raw) {
        Ok(request) => request,
        Err(refusal) => return http_response_cors(refusal.status, "text/plain", refusal.message.as_bytes(), None),
    };
    if !host_allowed(request.head, client.port) {
        return http_response_cors(
            "421 Misdirected Request",
            "text/plain",
            b"This node answers only to 127.0.0.1, localhost, and [::1].",
            None,
        );
    }
    let origin = match check_origin(request.head, &node.local.shop_origin) {
        OriginCheck::Absent => Some(node.local.shop_origin.clone()),
        OriginCheck::Allowed(origin) => Some(origin),
        OriginCheck::Refused => {
            return http_response_cors("403 Forbidden", "text/plain", b"That page may not call this node.", None)
        }
    };
    if request.method == "OPTIONS" {
        return http_response_cors("204 No Content", "text/plain", b"", origin.as_deref());
    }
    route(node, &request, client, origin.as_deref())
}

/// `path` without `prefix` and `suffix`, or `None` when it lacks either.
fn between<'a>(path: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    path.strip_prefix(prefix)?.strip_suffix(suffix)
}

fn route(node: &Node, request: &Request<'_>, client: Client, origin: Option<&str>) -> Vec<u8> {
    let (method, path, head, body) = (request.method, request.path, request.head, request.body);
    let client = &client;
    let modules = [crate::store_http::response, crate::sales_http::response, crate::public_http::response];
    for module in modules {
        if let Some(response) = module(node, client, head, method, path, body, origin) {
            return response;
        }
    }
    match (method, path) {
        ("GET", "/catalog") | ("GET", "/") => crate::xmr::catalog_response(node, origin),
        (_, path) if path.starts_with("/xmr/") => crate::xmr::response(node, client, head, method, path, body, origin),
        (_, path) if path.starts_with("/ln/") => crate::lightning::response(node, client, head, method, path, body, origin),
        ("POST", "/pair") => pairing::response(node, body, origin),
        ("GET", "/rates") => json_body("200 OK", &node.rates_value(), origin),
        ("GET", "/addresses") => addresses_list(node, client, head, origin),
        ("POST", "/addresses") => addresses_create(node, client, head, body, origin),
        ("PUT", path) if path.starts_with("/addresses/") => {
            addresses_update(node, client, head, &path["/addresses/".len()..], body, origin)
        }
        ("DELETE", path) if path.starts_with("/addresses/") => {
            addresses_delete(node, client, head, &path["/addresses/".len()..], origin)
        }
        ("GET", "/network") => wallet_http::network_response(node, origin),
        ("GET", "/version") => crate::build_info::version_response(node, client, head, origin),
        ("GET", "/wallet") => wallet_http::wallet_response(node, client, head, origin),
        ("POST", "/wallet/address") => wallet_http::wallet_address_response(node, client, head, origin),
        ("GET", "/lock") => lock_http::response(node, client, head, Route::Status, body, origin),
        ("POST", "/lock") => lock_http::response(node, client, head, Route::Lock, body, origin),
        ("POST", "/lock/activity") => lock_http::response(node, client, head, Route::Activity, body, origin),
        ("PUT", "/lock/settings") => lock_http::response(node, client, head, Route::Settings, body, origin),
        ("POST", "/lock/passphrase") => lock_http::response(node, client, head, Route::Passphrase, body, origin),
        ("POST", "/unlock") => lock_http::response(node, client, head, Route::Unlock, body, origin),
        ("POST", "/setup") => lock_http::response(node, client, head, Route::Setup, body, origin),
        ("POST", "/setup/confirm") => lock_http::response(node, client, head, Route::ConfirmBackup, body, origin),
        ("POST", "/wallet/phrase") => lock_http::response(node, client, head, Route::Phrase, body, origin),
        ("POST", "/spend-tickets") => lock_http::response(node, client, head, Route::Ticket, body, origin),
        ("POST", path) if between(path, "/orders/", "/pay/quote").is_some() => {
            let id = between(path, "/orders/", "/pay/quote").unwrap_or_default();
            lock_http::response(node, client, head, Route::Quote(id), body, origin)
        }
        ("POST", path) if between(path, "/orders/", "/pay/bump").is_some() => {
            let id = between(path, "/orders/", "/pay/bump").unwrap_or_default();
            wallet_http::bump_response(node, client, head, id, body, origin)
        }
        ("POST", path) if between(path, "/orders/", "/pay").is_some() => {
            let id = between(path, "/orders/", "/pay").unwrap_or_default();
            wallet_http::pay_response(node, client, head, id, body, origin)
        }
        ("POST", path) if between(path, "/orders/", "/release").is_some() => {
            let id = between(path, "/orders/", "/release").unwrap_or_default();
            escrow_http::action_response(node, client, head, Action::Release(id), body, origin)
        }
        ("POST", path) if between(path, "/orders/", "/dispute").is_some() => {
            let id = between(path, "/orders/", "/dispute").unwrap_or_default();
            escrow_http::action_response(node, client, head, Action::Dispute(id), body, origin)
        }
        ("POST", "/escrow/mail") => match mail_drop::admit(node, head, body) {
            Ok(()) => escrow_http::mail_response(node, client, body, origin),
            Err(response) => response,
        },
        ("GET", "/escrow/sales") => escrow_http::sales_response(node, client, head, origin),
        ("POST", path) if between(path, "/escrow/sales/", "/dispute").is_some() => {
            let id = between(path, "/escrow/sales/", "/dispute").unwrap_or_default();
            escrow_http::action_response(node, client, head, Action::SaleDispute(id), body, origin)
        }
        ("POST", "/escrow/bump") => escrow_http::action_response(node, client, head, Action::Bump, body, origin),
        ("GET", "/moderator/disputes") => escrow_http::disputes_response(node, client, head, origin),
        ("POST", path) if between(path, "/moderator/disputes/", "/rule").is_some() => {
            let id = between(path, "/moderator/disputes/", "/rule").unwrap_or_default();
            escrow_http::action_response(node, client, head, Action::Rule(id), body, origin)
        }
        ("GET", "/orders") => orders_list(node, client, head, origin),
        ("GET", path) if path.starts_with("/orders/") => {
            orders_get(node, client, head, &path["/orders/".len()..], origin)
        }
        ("GET", path) if path.starts_with("/listings/") => {
            listing_response(node, &path["/listings/".len()..], origin)
        }
        ("POST", "/orders") => order_response(node, head, body, origin),
        ("POST", "/checkout") => checkout_response(node, client, head, body, origin),
        ("GET", "/labelers") | ("PUT", "/labelers") => labelers::response(node, client, head, method, body, origin),
        ("GET", "/stores") => stores::list_response(node, client, head, origin),
        ("GET", path) if path.starts_with("/stores/") => {
            stores::detail_response(node, client, head, &path["/stores/".len()..], origin)
        }
        ("GET", path) if path.starts_with("/media/") => {
            media::response(node, client, head, &path["/media/".len()..], origin)
        }
        ("POST", "/content-labels") => vendor_labels::response(node, client, head, body, origin),
        (_, path) if path == "/remote" || path.starts_with("/remote/") => {
            crate::remote::local_response(node, client, head, method, path, body, origin)
        }
        (_, path) if path == "/home" || path.starts_with("/home/") => {
            crate::remote::home_response(node, client, head, method, path, body, origin)
        }
        _ => http_response_cors("404 Not Found", "text/plain", b"not found", origin),
    }
}

fn listing_response(node: &Node, hash: &str, origin: Option<&str>) -> Vec<u8> {
    let catalog = node.catalog();
    let Some(listing) = find_listing(&catalog, hash) else {
        return json_error("404 Not Found", "That listing is not in this shop.", origin);
    };
    match listing_value(&catalog, listing) {
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
    if !secret_eq(header(head, PAIR_HEADER), node.pairing_token()) {
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
        Err(OrderError::Locked(state)) => lock_http::locked_response(state, origin),
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
        Err(OrderError::Locked(state)) => lock_http::locked_response(state, origin),
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

pub(crate) fn local_guard(node: &Node, client: &Client, head: &str, origin: Option<&str>) -> Result<(), Vec<u8>> {
    if !client.loopback {
        return Err(json_error(
            "403 Forbidden",
            "This stays on this machine.",
            origin,
        ));
    }
    if !secret_eq(header(head, PAIR_HEADER), node.pairing_token()) {
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
