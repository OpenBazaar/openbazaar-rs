use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use futures::Stream;
use futures::StreamExt;
use tor_cell::relaycell::msg::{Connected, End, EndReason};
use tor_hsservice::{HsNickname, OnionServiceConfig, RendRequest, RunningOnionService};
use tor_proto::stream::IncomingStreamRequest;
use tor_rtcompat::Runtime;

use crate::mailbox::Mailbox;
use crate::onion::{OnionAddress, ORDER_PORT};
use crate::transport::{read_exact, read_letter, read_query_after_magic, write_query_reply, QUERY_MAGIC, STORED_ACK};
use crate::Error;
use crate::LetterId;

/// What arrived on the vendor onion. A shop request is answered by the sale
/// gate. It is not stored as a letter.
pub enum Arrival {
    Letter(LetterId),
    Shop,
    /// A directory query was answered on this stream.
    Query,
}

/// `GET` / `POST` and the other HTTP methods a buyer might send. A sealed
/// letter starts with `OBML` and is not a shop request.
pub fn is_shop_preface(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GET ")
        || bytes.starts_with(b"POST")
        || bytes.starts_with(b"HEAD")
        || bytes.starts_with(b"PUT ")
        || bytes.starts_with(b"DELE")
        || bytes.starts_with(b"OPTI")
        || bytes.starts_with(b"PATC")
}

fn is_letter_preface(bytes: &[u8]) -> bool {
    bytes.starts_with(b"OBML")
}

/// Handle returned by [`start_onion_listener`].
///
/// This is not a daemon. The node process keeps the handle and calls
/// [`OnionListener::accept_letter`] in a loop. Dropping the handle stops
/// the service from the caller's point of view; Arti's runtime tasks wind
/// down with the `RunningOnionService`.
pub struct OnionListener {
    service: Arc<RunningOnionService>,
    requests: Pin<Box<dyn Stream<Item = RendRequest> + Send>>,
}

impl OnionListener {
    /// Onion address Arti published for this service, once the identity key
    /// is in the client keystore.
    pub fn onion_address(&self) -> Option<OnionAddress> {
        self.service.onion_address().map(OnionAddress::from_hs_id)
    }

    /// Accept one introduced stream.
    ///
    /// Streams aimed at any port other than [`ORDER_PORT`] are rejected.
    /// A public shop request on that port is not a letter. The store secret
    /// is not used here.
    pub async fn accept_letter(&mut self, mailbox: &Mailbox) -> Result<LetterId, Error> {
        match self.accept_next(mailbox, None, |_, _| None).await? {
            Arrival::Letter(id) => Ok(id),
            Arrival::Shop => Err(Error::Listener("shop request with no sale gate")),
            Arrival::Query => Err(Error::Listener("directory query with no answer")),
        }
    }

    /// Accept one stream. `shop` is the sale gate. An HTTP preface is relayed
    /// there and is not stored. `OBML` is a sealed letter. `OBQR` is a
    /// directory query answered with `answer`.
    pub async fn accept_next<F>(&mut self, mailbox: &Mailbox, shop: Option<SocketAddr>, answer: F) -> Result<Arrival, Error>
    where
        F: FnOnce(u8, [u8; 32]) -> Option<Vec<u8>>,
    {
        let rend = self.requests.next().await.ok_or(Error::ListenerClosed)?;
        let mut streams = rend
            .accept()
            .await
            .map_err(|err| Error::Arti(err.to_string()))?;
        let request = streams
            .next()
            .await
            .ok_or(Error::Listener("rendezvous closed before a stream"))?;
        let port = match request.request() {
            IncomingStreamRequest::Begin(begin) => Some(begin.port()),
            _ => None,
        };
        if port != Some(ORDER_PORT) {
            request
                .reject(End::new_with_reason(EndReason::DONE))
                .await
                .map_err(|err| Error::Arti(err.to_string()))?;
            return Err(Error::Listener("rejected a stream for another port"));
        }
        let mut stream = request
            .accept(Connected::new_empty())
            .await
            .map_err(|err| Error::Arti(err.to_string()))?;
        let mut preface = [0u8; 4];
        read_exact(&mut stream, &mut preface).await?;
        if is_letter_preface(&preface) {
            let mut replay = futures::io::Cursor::new(preface).chain(&mut stream);
            let letter = read_letter(&mut replay).await?;
            let id = mailbox.accept(&letter)?;
            // The sender blocks until it reads this byte. Close without it and
            // the sender drops the circuit thinking the letter was lost.
            stream.write_all(&[STORED_ACK]).await?;
            stream.flush().await?;
            let mut done = [0u8; 1];
            let _ = stream.read(&mut done).await;
            return Ok(Arrival::Letter(id));
        }
        if preface == *QUERY_MAGIC {
            let (kind, id) = read_query_after_magic(&mut stream).await?;
            write_query_reply(&mut stream, answer(kind, id).as_deref()).await?;
            stream.flush().await?;
            return Ok(Arrival::Query);
        }
        if is_shop_preface(&preface) {
            let Some(shop) = shop else {
                return Err(Error::Listener("shop request with no sale gate"));
            };
            relay_shop(&mut stream, preface, shop).await?;
            return Ok(Arrival::Shop);
        }
        Err(Error::Malformed)
    }

    /// Short status for the operator log: high-level state, and the current
    /// problem when there is one.
    pub fn status_summary(&self) -> String {
        let status = self.service.status();
        match status.current_problem() {
            Some(problem) => format!("{:?}: {problem:?}", status.state()),
            None => format!("{:?}", status.state()),
        }
    }
}

/// Copy one HTTP request to the sale gate and copy its answer back.
///
/// The gate is on loopback. This does not dial the Host header.
async fn relay_shop<S>(onion: &mut S, preface: [u8; 4], shop: SocketAddr) -> Result<(), Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut local = tokio::net::TcpStream::connect(shop).await?;
    let mut request = preface.to_vec();
    tokio::io::AsyncWriteExt::write_all(&mut local, &preface).await?;
    let mut buf = [0u8; 8192];
    while !http_request_complete(&request) {
        if request.len() > 4 * 1024 * 1024 + 8192 {
            return Err(Error::Listener("shop request is too large"));
        }
        let n = onion.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        request.extend_from_slice(&buf[..n]);
        tokio::io::AsyncWriteExt::write_all(&mut local, &buf[..n]).await?;
    }
    tokio::io::AsyncWriteExt::shutdown(&mut local).await?;
    let mut response = Vec::new();
    loop {
        let n = tokio::io::AsyncReadExt::read(&mut local, &mut buf).await?;
        if n == 0 {
            break;
        }
        if response.len() + n > 4 * 1024 * 1024 + 8192 {
            return Err(Error::Listener("shop answer is too large"));
        }
        response.extend_from_slice(&buf[..n]);
    }
    onion.write_all(&response).await?;
    onion.flush().await?;
    Ok(())
}

fn http_request_complete(bytes: &[u8]) -> bool {
    let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let head = &bytes[..end];
    let mut length = 0usize;
    for line in head.split(|&byte| byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some((name, value)) = split_once_byte(line, b':') else {
            continue;
        };
        if name.eq_ignore_ascii_case(b"content-length") {
            length = std::str::from_utf8(trim_ascii(value)).ok().and_then(|text| text.parse().ok()).unwrap_or(0);
        }
    }
    bytes.len() >= end + 4 + length
}

fn split_once_byte(bytes: &[u8], needle: u8) -> Option<(&[u8], &[u8])> {
    let index = bytes.iter().position(|byte| *byte == needle)?;
    Some((&bytes[..index], &bytes[index + 1..]))
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|byte| !byte.is_ascii_whitespace()).unwrap_or(bytes.len());
    let end = bytes.iter().rposition(|byte| !byte.is_ascii_whitespace()).map(|index| index + 1).unwrap_or(start);
    &bytes[start..end]
}

/// Local keystore name for this node's order mailbox. Not the onion address.
const ORDER_NICKNAME: &str = "ob-orders";

/// A vendor mailbox listening on its onion.
///
/// Holds the Arti client so the service keeps running. Dropping this stops
/// the service. [`RunningMailbox::start`] does not dial; publishing the
/// descriptor happens in the background once the client bootstraps.
pub struct RunningMailbox {
    listener: OnionListener,
    /// Arti tasks for this service borrow the client. It has to outlive them.
    client: Arc<arti_client::TorClient<tor_rtcompat::PreferredRuntime>>,
    /// Sale gate for public shop requests on this same onion. Absent until
    /// the node binds one. Letters do not need it.
    shop: Option<SocketAddr>,
}

impl RunningMailbox {
    /// Start the order listener with Arti state under `state_dir`.
    ///
    /// Call this inside a running Tokio runtime. The identity key is created
    /// in `state_dir` if it is not already there, and
    /// [`RunningMailbox::onion_address`] can be read before any circuit opens.
    pub fn start(state_dir: &std::path::Path) -> Result<Self, Error> {
        let client = crate::client::open_client(state_dir)?;
        let listener = start_onion_listener(&client, ORDER_NICKNAME)?;
        Ok(Self { listener, client, shop: None })
    }

    /// Public shop requests on this onion are relayed to `gate`. The gate
    /// decides which routes are served. This does not open a clearnet socket.
    pub fn set_shop(&mut self, gate: SocketAddr) {
        self.shop = Some(gate);
    }

    /// The v3 address Arti stored for this service.
    pub fn onion_address(&self) -> Result<OnionAddress, Error> {
        self.listener
            .onion_address()
            .ok_or(Error::Listener("onion identity is not in the keystore"))
    }

    /// Read one sealed letter, relay one public shop request, or answer one directory query.
    pub async fn accept_letter<F>(&mut self, mailbox: &Mailbox, answer: F) -> Result<Arrival, Error>
    where
        F: FnOnce(u8, [u8; 32]) -> Option<Vec<u8>>,
    {
        self.listener.accept_next(mailbox, self.shop, answer).await
    }

    /// Download a Tor directory. Launching the service does not do this.
    /// Without it the descriptor is never published and clients hear that
    /// the onion service was not found.
    pub async fn bootstrap(&self) -> Result<(), Error> {
        self.client.bootstrap().await.map_err(|err| Error::Arti(err.to_string()))
    }

    pub fn status_summary(&self) -> String {
        self.listener.status_summary()
    }
}

/// Start the vendor's onion mailbox on an existing Arti client.
///
/// The node binary calls this. It does not fork, detach, or install a
/// supervisor. The client must be configured with a keystore: Arti stores
/// the onion identity there and `launch_onion_service` refuses to run
/// without one.
///
/// `nickname` is the local keystore name, for example `ob-orders`. It is
/// not the onion address. Read the address from
/// [`OnionListener::onion_address`] after this returns.
pub fn start_onion_listener<R>(
    client: &arti_client::TorClient<R>,
    nickname: &str,
) -> Result<OnionListener, Error>
where
    R: Runtime + 'static,
{
    let nickname = HsNickname::new(nickname.to_owned())
        .map_err(|_| Error::Listener("nickname is not a valid onion-service slug"))?;
    let config = OnionServiceConfig::builder()
        .nickname(nickname)
        .build()
        .map_err(|_| Error::Listener("onion service config is incomplete"))?;
    let (service, requests) = client
        .launch_onion_service(config)
        .map_err(|err| Error::Arti(err.to_string()))?
        .ok_or(Error::Listener("onion service is disabled in the config"))?;
    Ok(OnionListener {
        service,
        requests: Box::pin(requests),
    })
}
