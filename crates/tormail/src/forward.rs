use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use futures::{Future, Stream, StreamExt};
use tor_cell::relaycell::msg::{Connected, End, EndReason};
use tor_hscrypto::pk::{HsClientDescEncKey, HsClientDescEncSecretKey, HsIdKeypair};
use tor_hsservice::config::restricted_discovery::HsClientNickname;
use tor_hsservice::{
    handle_rend_requests, HsNickname, OnionServiceConfig, RendRequest, RunningOnionService,
};
use tor_keymgr::KeystoreSelector;
use tor_llcrypto::pk::{curve25519, ed25519};
use tor_proto::stream::IncomingStreamRequest;
use tor_rtcompat::Runtime;

use crate::onion::OnionAddress;
use crate::Error;

/// An onion service whose streams to one virtual port are piped, byte for
/// byte, to a local TCP address. Streams to any other port are refused.
///
/// The local side does all authentication: this layer only proves to the
/// caller that it reached the holder of the onion key. Dropping the handle
/// stops the service once the forwarding future is dropped too.
pub struct OnionForward {
    service: Arc<RunningOnionService>,
    nickname: HsNickname,
}

/// Who may find a service. `Restricted` publishes the descriptor encrypted to
/// these clients only (Tor client authorization): `(nickname, "descriptor:x25519:…")`.
/// An empty list still restricts; a random key nobody holds keeps it closed.
pub enum Discovery<'a> {
    Open,
    Restricted(&'a [(String, String)]),
}

impl OnionForward {
    pub fn onion_address(&self) -> Option<OnionAddress> {
        self.service.onion_address().map(OnionAddress::from_hs_id)
    }

    /// Arti's view of the service, for logs and the status route.
    pub fn state(&self) -> String {
        format!("{:?}", self.service.status().state())
    }

    /// Replace the clients of a restricted service. Arti republishes the
    /// descriptor; clients removed here may still know the old introduction
    /// points for a while.
    pub fn set_clients(&self, clients: &[(String, String)]) -> Result<(), Error> {
        let config = service_config(&self.nickname, &Discovery::Restricted(clients))?;
        self.service
            .reconfigure(config, tor_config::Reconfigure::WarnOnFailures)
            .map_err(|err| Error::Arti(err.to_string()))
    }
}

/// The onion address of the service whose identity key is the ed25519 secret `seed`.
pub fn onion_for_seed(seed: &[u8; 32]) -> OnionAddress {
    let public = ed25519::Keypair::from_bytes(seed).verifying_key();
    OnionAddress::from_hs_id(tor_hscrypto::pk::HsIdKey::from(public).id())
}

/// Store the client half of a restricted service's discovery key in the
/// client's keystore, so it can find `onion`. `secret` is 32 bytes.
pub fn insert_discovery_key<R: Runtime>(
    client: &arti_client::TorClient<R>,
    onion: &OnionAddress,
    secret: [u8; 32],
) -> Result<(), Error> {
    let key = HsClientDescEncSecretKey::from(curve25519::StaticSecret::from(secret));
    match client.insert_service_discovery_key(KeystoreSelector::Primary, *onion.hs_id(), key) {
        Ok(_) => Ok(()),
        Err(_)
            if client
                .get_service_discovery_key(*onion.hs_id())
                .ok()
                .flatten()
                .is_some() =>
        {
            Ok(())
        }
        Err(err) => Err(Error::Arti(err.to_string())),
    }
}

/// `descriptor:x25519:…` for a discovery secret.
pub fn discovery_public(secret: [u8; 32]) -> String {
    let secret = HsClientDescEncSecretKey::from(curve25519::StaticSecret::from(secret));
    HsClientDescEncKey::from(&secret).to_string()
}

fn service_config(
    nickname: &HsNickname,
    discovery: &Discovery<'_>,
) -> Result<OnionServiceConfig, Error> {
    let mut builder = OnionServiceConfig::builder();
    builder.nickname(nickname.clone());
    if let Discovery::Restricted(clients) = discovery {
        let restricted = builder.restricted_discovery();
        restricted.enabled(true);
        let mut keys = Vec::with_capacity(clients.len().max(1));
        for (name, key) in clients.iter() {
            let name = HsClientNickname::from_str(name)
                .map_err(|_| Error::Listener("client nickname is not a valid slug"))?;
            let key = HsClientDescEncKey::from_str(key)
                .map_err(|_| Error::Listener("client key is not descriptor:x25519:…"))?;
            keys.push((name, key));
        }
        if keys.is_empty() {
            let nobody = curve25519::StaticSecret::from(rand::random::<[u8; 32]>());
            let key = HsClientDescEncKey::from(curve25519::PublicKey::from(&nobody));
            keys.push((
                HsClientNickname::from_str("nobody").expect("valid slug"),
                key,
            ));
        }
        for key in keys {
            restricted.static_keys().access().push(key);
        }
    }
    builder
        .build()
        .map_err(|_| Error::Listener("onion service config is incomplete"))
}

/// Launch `nickname` on `client` and return the handle plus the future that
/// forwards its streams. Run the future on a Tokio runtime; it spawns one
/// task per stream.
///
/// `identity` is an ed25519 secret for the service key; without it Arti
/// makes one and keeps it in its keystore under `nickname`.
pub fn start_onion_forward<R>(
    client: &arti_client::TorClient<R>,
    nickname: &str,
    identity: Option<[u8; 32]>,
    discovery: Discovery<'_>,
    port: u16,
    target: SocketAddr,
) -> Result<(OnionForward, impl Future<Output = ()> + Send + 'static), Error>
where
    R: Runtime + 'static,
{
    let nickname = HsNickname::new(nickname.to_owned())
        .map_err(|_| Error::Listener("nickname is not a valid onion-service slug"))?;
    let config = service_config(&nickname, &discovery)?;
    let launched = match identity {
        Some(seed) => {
            let keypair = HsIdKeypair::from(ed25519::ExpandedKeypair::from(
                &ed25519::Keypair::from_bytes(&seed),
            ));
            // A key left in the keystore by an earlier run is the same key.
            match client.launch_onion_service_with_hsid(config.clone(), keypair) {
                Ok(launched) => Ok(launched.map(|(service, requests)| (service, requests.boxed()))),
                Err(_) => client.launch_onion_service(config).map(|launched| {
                    launched.map(|(service, requests)| (service, requests.boxed()))
                }),
            }
        }
        None => client
            .launch_onion_service(config)
            .map(|launched| launched.map(|(service, requests)| (service, requests.boxed()))),
    };
    let (service, requests) = launched
        .map_err(|err: arti_client::Error| Error::Arti(err.to_string()))?
        .ok_or(Error::Listener("onion service is disabled in the config"))?;
    Ok((
        OnionForward { service, nickname },
        forward(requests, port, target),
    ))
}

async fn forward<S>(requests: S, port: u16, target: SocketAddr)
where
    S: Stream<Item = RendRequest> + Send + 'static,
{
    let mut streams = Box::pin(handle_rend_requests(requests));
    while let Some(request) = streams.next().await {
        let wanted = matches!(request.request(), IncomingStreamRequest::Begin(begin) if begin.port() == port);
        tokio::spawn(async move {
            if !wanted {
                let _ = request.reject(End::new_with_reason(EndReason::DONE)).await;
                return;
            }
            let Ok(mut local) = tokio::net::TcpStream::connect(target).await else {
                let _ = request
                    .reject(End::new_with_reason(EndReason::CONNECTREFUSED))
                    .await;
                return;
            };
            let Ok(mut onion) = request.accept(Connected::new_empty()).await else {
                return;
            };
            let _ = tokio::io::copy_bidirectional(&mut onion, &mut local).await;
        });
    }
}

/// Send `request` to `onion:port` over a fresh stream on `client` and read
/// the answer until the far side closes, up to `max` bytes.
pub async fn exchange<R>(
    client: &arti_client::TorClient<R>,
    onion: &OnionAddress,
    port: u16,
    request: &[u8],
    max: usize,
) -> Result<Vec<u8>, Error>
where
    R: Runtime,
{
    use futures::{AsyncReadExt, AsyncWriteExt};
    client
        .bootstrap()
        .await
        .map_err(|err| Error::Arti(err.to_string()))?;
    let mut stream = client
        .connect((onion.as_str(), port))
        .await
        .map_err(|err| Error::Arti(err.to_string()))?;
    stream.write_all(request).await?;
    stream.flush().await?;
    let mut answer = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        // A service that closes its side ends the stream with an END cell
        // Arti reads as an error, after every data cell. What arrived is the
        // answer; the caller checks it is whole (Content-Length).
        let read = match stream.read(&mut chunk).await {
            Ok(read) => read,
            Err(_) if !answer.is_empty() => 0,
            Err(err) => return Err(err.into()),
        };
        if read == 0 {
            break;
        }
        if answer.len() + read > max {
            return Err(Error::Listener("the answer is larger than allowed"));
        }
        answer.extend_from_slice(&chunk[..read]);
    }
    Ok(answer)
}
