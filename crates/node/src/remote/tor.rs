//! One Arti client per process, on its own Tokio runtime.
//!
//! State and cache live in `tor/` in the data dir; the onion service key is
//! in Arti's keystore there, so the address stays the same across restarts.
//! Arti refuses a state dir other users can write to.
//! `OB_TOR_TRUST_DATA_DIR=1` skips that check (CI runners and some Docker
//! volumes have loose parent directories).

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use arti_client::config::TorClientConfigBuilder;
use arti_client::{BootstrapBehavior, TorClient};
use tor_rtcompat::PreferredRuntime;
use tormail::{Discovery, OnionAddress, OnionForward};

pub(crate) const ADMIN_PORT: u16 = 80;
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(90);
/// Pause before refetching a descriptor that refused our discovery key.
const DESCRIPTOR_WAIT: Duration = Duration::from_secs(8);

pub(crate) struct Tor {
    runtime: tokio::runtime::Runtime,
    client: Arc<TorClient<PreferredRuntime>>,
}

impl Tor {
    pub(crate) fn start(data_dir: &Path) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ob-tor")
            .enable_all()
            .build()
            .map_err(|err| err.to_string())?;
        let mut builder = TorClientConfigBuilder::from_directories(
            data_dir.join("tor/state"),
            data_dir.join("tor/cache"),
        );
        if std::env::var_os("OB_TOR_TRUST_DATA_DIR").is_some() {
            builder.storage().permissions().dangerously_trust_everyone();
        }
        let config = builder.build().map_err(|err| err.to_string())?;
        let client = {
            let _inside = runtime.enter();
            TorClient::builder()
                .config(config)
                .bootstrap_behavior(BootstrapBehavior::OnDemand)
                .create_unbootstrapped()
                .map_err(|err| err.to_string())?
        };
        let background = client.clone();
        runtime.spawn(async move {
            if let Err(err) = background.bootstrap().await {
                eprintln!("ob-node: Tor bootstrap: {err}");
            }
        });
        Ok(Self { runtime, client })
    }

    /// Publish an onion service and pipe port 80 to `target`. It runs until
    /// the returned [`Service`] is dropped.
    pub(crate) fn serve(
        &self,
        nickname: &str,
        identity: Option<[u8; 32]>,
        discovery: Discovery<'_>,
        target: SocketAddr,
    ) -> Result<Service, String> {
        let _inside = self.runtime.enter();
        let (forward, run) = tormail::start_onion_forward(
            &self.client,
            nickname,
            identity,
            discovery,
            ADMIN_PORT,
            target,
        )
        .map_err(|err| err.to_string())?;
        let task = self.runtime.spawn(run);
        Ok(Service { forward, task })
    }

    /// Let this client find the restricted service `onion`.
    pub(crate) fn add_discovery_key(
        &self,
        onion: &OnionAddress,
        secret: [u8; 32],
    ) -> Result<(), String> {
        tormail::insert_discovery_key(&self.client, onion, secret).map_err(|err| err.to_string())
    }

    /// One request to an onion service; the whole answer.
    pub(crate) fn exchange(
        &self,
        onion: &OnionAddress,
        request: &[u8],
        max: usize,
    ) -> Result<Vec<u8>, String> {
        self.exchange_within(onion, request, max, EXCHANGE_TIMEOUT)
    }

    /// One request within `limit`. A refused discovery key is retried with a
    /// fresh descriptor: the cached one may predate this device, and right
    /// after approval the node may not have uploaded the new one yet.
    pub(crate) fn exchange_within(
        &self,
        onion: &OnionAddress,
        request: &[u8],
        max: usize,
        limit: Duration,
    ) -> Result<Vec<u8>, String> {
        let deadline = std::time::Instant::now() + limit;
        let mut result = self.exchange_once(onion, request, max, limit, false);
        for _ in 0..3 {
            match &result {
                Err(err) if err.contains("authentication") => {}
                _ => break,
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left <= DESCRIPTOR_WAIT {
                break;
            }
            std::thread::sleep(DESCRIPTOR_WAIT);
            result = self.exchange_once(onion, request, max, left - DESCRIPTOR_WAIT, true);
        }
        result
    }

    fn exchange_once(
        &self,
        onion: &OnionAddress,
        request: &[u8],
        max: usize,
        limit: Duration,
        fresh: bool,
    ) -> Result<Vec<u8>, String> {
        self.runtime.block_on(async {
            match tokio::time::timeout(
                limit,
                tormail::exchange(&self.client, onion, ADMIN_PORT, request, max, fresh),
            )
            .await
            {
                Ok(result) => result.map_err(|err| err.to_string()),
                Err(_) => Err("the home node did not answer over Tor in time".to_string()),
            }
        })
    }
}

pub(crate) struct Service {
    pub forward: OnionForward,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Service {
    fn drop(&mut self) {
        self.task.abort();
    }
}
