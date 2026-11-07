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
use tormail::{OnionAddress, OnionForward};

pub(crate) const ADMIN_PORT: u16 = 80;
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(90);

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
        let mut builder = TorClientConfigBuilder::from_directories(data_dir.join("tor/state"), data_dir.join("tor/cache"));
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

    /// Publish the admin onion service and pipe it to `target`.
    pub(crate) fn serve(&self, nickname: &str, target: SocketAddr) -> Result<OnionForward, String> {
        let _inside = self.runtime.enter();
        let (forward, run) =
            tormail::start_onion_forward(&self.client, nickname, ADMIN_PORT, target).map_err(|err| err.to_string())?;
        self.runtime.spawn(run);
        Ok(forward)
    }

    /// One request to a home node's onion; the whole answer.
    pub(crate) fn exchange(&self, onion: &OnionAddress, request: &[u8], max: usize) -> Result<Vec<u8>, String> {
        self.runtime.block_on(async {
            match tokio::time::timeout(EXCHANGE_TIMEOUT, tormail::exchange(&self.client, onion, ADMIN_PORT, request, max)).await {
                Ok(result) => result.map_err(|err| err.to_string()),
                Err(_) => Err("the home node did not answer over Tor in time".to_string()),
            }
        })
    }
}
