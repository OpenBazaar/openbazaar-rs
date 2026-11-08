use std::pin::Pin;
use std::sync::Arc;

use futures::io::{AsyncReadExt, AsyncWriteExt};
use futures::Stream;
use futures::StreamExt;
use tor_cell::relaycell::msg::{Connected, End, EndReason};
use tor_hsservice::{HsNickname, OnionServiceConfig, RendRequest, RunningOnionService};
use tor_proto::stream::IncomingStreamRequest;
use tor_rtcompat::Runtime;

use crate::mailbox::Mailbox;
use crate::onion::{OnionAddress, ORDER_PORT};
use crate::transport::{read_letter, STORED_ACK};
use crate::Error;
use crate::LetterId;

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

    /// Accept one introduced stream, read one sealed letter, and store it.
    ///
    /// Streams aimed at any port other than [`ORDER_PORT`] are rejected.
    /// The store secret is not used here.
    pub async fn accept_letter(&mut self, mailbox: &Mailbox) -> Result<LetterId, Error> {
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
        let letter = read_letter(&mut stream).await?;
        let id = mailbox.accept(&letter)?;
        // The sender blocks until it reads this byte. Close without it and
        // the sender drops the circuit thinking the letter was lost.
        stream.write_all(&[STORED_ACK]).await?;
        stream.flush().await?;
        let mut done = [0u8; 1];
        let _ = stream.read(&mut done).await;
        Ok(id)
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
        Ok(Self { listener, client })
    }

    /// The v3 address Arti stored for this service.
    pub fn onion_address(&self) -> Result<OnionAddress, Error> {
        self.listener
            .onion_address()
            .ok_or(Error::Listener("onion identity is not in the keystore"))
    }

    /// Read one sealed letter and store it in `mailbox`.
    pub async fn accept_letter(&mut self, mailbox: &Mailbox) -> Result<LetterId, Error> {
        self.listener.accept_letter(mailbox).await
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
