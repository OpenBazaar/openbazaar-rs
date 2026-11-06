use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use futures::StreamExt;
use tor_cell::relaycell::msg::{Connected, End, EndReason};
use tor_hsservice::{HsNickname, OnionServiceConfig, RendRequest, RunningOnionService};
use tor_proto::stream::IncomingStreamRequest;
use tor_rtcompat::Runtime;

use crate::mailbox::Mailbox;
use crate::onion::{OnionAddress, ORDER_PORT};
use crate::transport::read_letter;
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
        mailbox.accept(&letter)
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
