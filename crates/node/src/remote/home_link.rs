//! How this node reaches its home node: the encrypted LAN channel when the
//! home node is on the same network, Tor otherwise.
//!
//! A LAN address is dialed only if it is private (or loopback, with
//! `--allow-loopback-home` for tests), so a crafted pairing link cannot make
//! this machine connect to the internet outside Tor.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

use tormail::OnionAddress;
use x25519_dalek::StaticSecret;

use super::protocol::{lan_open_response, lan_seal_request};
use crate::http_parse::header;
use crate::vault_key::random_bytes;
use crate::Node;

pub(crate) const MAX_ANSWER: usize = 8 * 1024 * 1024;
const LAN_CONNECT: Duration = Duration::from_millis(1500);
const LAN_TIMEOUT: Duration = Duration::from_secs(30);

/// Where a request goes.
pub(crate) struct Route<'a> {
    pub onion: Option<&'a str>,
    pub lan: &'a [String],
    pub node_key: Option<&'a [u8; 32]>,
}

/// One HTTP/1.1 answer.
pub(crate) struct Answer {
    pub status: String,
    pub content_type: String,
    pub body: Vec<u8>,
    pub next_onion: Option<String>,
    pub via: &'static str,
}

fn lan_allowed(node: &Node, addr: &SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_link_local()
                || (v4.is_loopback() && node.remote.config().loopback_homes)
        }
        IpAddr::V6(v6) => {
            (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.is_loopback() && node.remote.config().loopback_homes)
        }
    }
}

/// Send `request` (one complete HTTP/1.1 request) and read the answer.
pub(crate) fn exchange(node: &Node, route: &Route<'_>, request: &[u8]) -> Result<Answer, String> {
    let mut last = None;
    if let Some(node_key) = route.node_key {
        for addr in route
            .lan
            .iter()
            .filter_map(|text| text.parse::<SocketAddr>().ok())
            .filter(|addr| lan_allowed(node, addr))
        {
            match lan_exchange(&addr, node_key, request) {
                Ok(bytes) => return parse_answer(&bytes, "lan"),
                Err(err) => last = Some(err),
            }
        }
    }
    let Some(onion) = route.onion else {
        return Err(
            last.unwrap_or_else(|| "The home node has no address this node can reach.".into())
        );
    };
    let onion =
        OnionAddress::parse(onion).map_err(|_| "That onion address is not valid.".to_string())?;
    let bytes = node
        .remote
        .tor(&node.data_dir)?
        .exchange(&onion, request, MAX_ANSWER)?;
    parse_answer(&bytes, "tor")
}

fn lan_exchange(addr: &SocketAddr, node_key: &[u8; 32], request: &[u8]) -> Result<Vec<u8>, String> {
    let io = |err: std::io::Error| format!("the home node on the LAN could not be reached: {err}");
    let ephemeral = StaticSecret::from(random_bytes::<32>());
    let (body, response_key) = lan_seal_request(node_key, &ephemeral, request);
    let mut stream = TcpStream::connect_timeout(addr, LAN_CONNECT).map_err(io)?;
    stream.set_read_timeout(Some(LAN_TIMEOUT)).map_err(io)?;
    let head = format!(
        "POST /lan HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&body))
        .map_err(io)?;
    let mut answer = Vec::new();
    stream
        .take(MAX_ANSWER as u64 + 1024)
        .read_to_end(&mut answer)
        .map_err(io)?;
    let outer = parse_answer(&answer, "lan")?;
    if !outer.status.starts_with("200") {
        return Err(format!(
            "the home node on the LAN refused the channel: {}",
            outer.status
        ));
    }
    lan_open_response(&response_key, &outer.body)
        .ok_or_else(|| "The LAN answer was not sealed by the home node's key.".into())
}

fn parse_answer(bytes: &[u8], via: &'static str) -> Result<Answer, String> {
    let bad = || "The home node sent an answer this node could not read.".to_string();
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(bad)?;
    let head = std::str::from_utf8(&bytes[..split]).map_err(|_| bad())?;
    let body = &bytes[split + 4..];
    let status = head
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("HTTP/1.1 "))
        .ok_or_else(bad)?
        .to_string();
    if let Some(length) = header(head, "content-length") {
        if length.parse::<usize>().ok() != Some(body.len()) {
            return Err(bad());
        }
    }
    Ok(Answer {
        status,
        content_type: header(head, "content-type")
            .unwrap_or("application/octet-stream")
            .to_string(),
        body: body.to_vec(),
        next_onion: header(head, super::NEXT_ONION_HEADER)
            .map(str::to_ascii_lowercase)
            .filter(|onion| OnionAddress::parse(onion).is_ok()),
        via,
    })
}
