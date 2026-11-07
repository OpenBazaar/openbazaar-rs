use std::net::SocketAddr;
use std::sync::Arc;

use futures::{Future, Stream, StreamExt};
use tor_cell::relaycell::msg::{Connected, End, EndReason};
use tor_hsservice::{handle_rend_requests, HsNickname, OnionServiceConfig, RendRequest, RunningOnionService};
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
}

impl OnionForward {
    pub fn onion_address(&self) -> Option<OnionAddress> {
        self.service.onion_address().map(OnionAddress::from_hs_id)
    }

    /// Arti's view of the service, for logs and the status route.
    pub fn state(&self) -> String {
        format!("{:?}", self.service.status().state())
    }
}

/// Launch `nickname` on `client` and return the handle plus the future that
/// forwards its streams. Run the future on a Tokio runtime; it spawns one
/// task per stream.
pub fn start_onion_forward<R>(
    client: &arti_client::TorClient<R>,
    nickname: &str,
    port: u16,
    target: SocketAddr,
) -> Result<(OnionForward, impl Future<Output = ()> + Send + 'static), Error>
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
    Ok((OnionForward { service }, forward(requests, port, target)))
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
                let _ = request.reject(End::new_with_reason(EndReason::CONNECTREFUSED)).await;
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
    client.bootstrap().await.map_err(|err| Error::Arti(err.to_string()))?;
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
