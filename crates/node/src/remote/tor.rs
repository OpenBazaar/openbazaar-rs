//! One Arti client per process, on its own Tokio runtime.
//!
//! State and cache live in `tor/` in the data dir; the onion service key is
//! in Arti's keystore there, so the address stays the same across restarts.
//! Arti refuses a state dir other users can write to.
//! `OB_TOR_TRUST_DATA_DIR=1` skips that check (CI runners and some Docker
//! volumes have loose parent directories).

use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arti_client::config::TorClientConfigBuilder;
use arti_client::{BootstrapBehavior, TorClient};
use tor_rtcompat::PreferredRuntime;
use tormail::{Discovery, OnionAddress, OnionForward};

use super::onion_check::Answered;
use super::tor_health::{Directory, Reading};

pub(crate) const ADMIN_PORT: u16 = 80;
/// Virtual port a Lightning peer dials on the `ob-ln` onion. LDK binds a
/// loopback socket; this is the port the node announces.
pub(crate) const LIGHTNING_PORT: u16 = 9735;
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(90);
/// Pause before refetching a descriptor that refused our discovery key.
const DESCRIPTOR_WAIT: Duration = Duration::from_secs(8);
const START_ATTEMPTS: u32 = 4;
const START_RETRY_WAIT: Duration = Duration::from_secs(2);

pub(crate) struct Tor {
    runtime: tokio::runtime::Runtime,
    client: Arc<TorClient<PreferredRuntime>>,
}

impl Tor {
    /// [`Tor::start`], tried again a few times. The `--onion` listener opens
    /// its own client on `{data}/tor` at the same moment, and on a fresh data
    /// dir one of the two can find the directory cache locked until the other
    /// has created it (`tormail::open_client` retries the same way).
    pub(crate) fn start_with_retry(data_dir: &Path) -> Result<Self, String> {
        let mut result = Self::start(data_dir);
        for _ in 0..START_ATTEMPTS - 1 {
            let Err(err) = &result else { break };
            eprintln!("ob-node: tor: starting again after: {err}");
            std::thread::sleep(START_RETRY_WAIT);
            result = Self::start(data_dir);
        }
        result
    }

    pub(crate) fn start(data_dir: &Path) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("ob-tor")
            .enable_all()
            .build()
            .map_err(|err| err.to_string())?;
        let (state, cache) =
            tormail::private_dirs(&data_dir.join("tor")).map_err(|err| err.to_string())?;
        let mut builder = TorClientConfigBuilder::from_directories(state, cache);
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
        super::tor_log::install();
        let background = client.clone();
        let started = std::time::Instant::now();
        runtime.spawn(async move {
            match background.bootstrap().await {
                Ok(()) => eprintln!(
                    "ob-node: tor: connected to the Tor network after {}s",
                    started.elapsed().as_secs()
                ),
                Err(err) => eprintln!("ob-node: Tor bootstrap: {err}"),
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
        self.serve_on(nickname, identity, discovery, ADMIN_PORT, target)
    }

    /// Publish an onion service and pipe `port` to `target`.
    pub(crate) fn serve_on(
        &self,
        nickname: &str,
        identity: Option<[u8; 32]>,
        discovery: Discovery<'_>,
        port: u16,
        target: SocketAddr,
    ) -> Result<Service, String> {
        let _inside = self.runtime.enter();
        let (forward, run) =
            tormail::start_onion_forward(&self.client, nickname, identity, discovery, port, target)
                .map_err(|err| err.to_string())?;
        let task = self.runtime.spawn(run);
        Ok(Service::Arti(ArtiService {
            forward,
            task,
            watch: None,
            answered: Answered::default(),
        }))
    }

    /// Follow a new pairing onion and set `reached` once this client has
    /// dialed it and got an answer: see [`super::pair_ready`].
    pub(crate) fn watch_pairing(
        &self,
        service: &mut Service,
        label: String,
        reached: Arc<AtomicBool>,
    ) {
        if let Some(forward) = service.arti_forward() {
            let watch = super::pair_ready::watch(
                Arc::clone(&self.client),
                forward.clone(),
                label,
                reached,
            );
            service.set_watch(self.runtime.spawn(watch));
        }
    }

    /// Dial the store onion from this client while Arti does not say it is
    /// published: see [`super::onion_check`].
    pub(crate) fn watch_store(&self, service: &mut Service) {
        if let Some(forward) = service.arti_forward() {
            let watch = super::onion_check::watch(
                Arc::clone(&self.client),
                forward.clone(),
                service.answered().clone(),
            );
            service.set_watch(self.runtime.spawn(watch));
        }
    }

    /// What Arti says of its connection and its directory right now, with
    /// `onions` as the services report themselves and `store_answered` as
    /// the store's [`Service`] has it: see [`super::tor_health`].
    pub(crate) fn reading(
        &self,
        onions: Vec<(&'static str, String, bool)>,
        store_answered: Option<u64>,
    ) -> Reading {
        let status = self.client.bootstrap_status();
        let secs = |time: SystemTime| {
            time.duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_secs())
        };
        // Arti hands out its directory until three days after it expired.
        let netdir = self
            .client
            .dirmgr()
            .ok()
            .and_then(|dirmgr| dirmgr.timely_netdir().ok());
        let directory = netdir.map(|netdir| {
            let lifetime = netdir.lifetime();
            Directory {
                valid_after: secs(lifetime.valid_after()),
                fresh_until: secs(lifetime.fresh_until()),
                valid_until: secs(lifetime.valid_until()),
            }
        });
        Reading {
            ready: status.ready_for_traffic(),
            blocked: status.blocked().map(|blockage| blockage.to_string()),
            directory,
            store_answered,
            onions,
        }
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

/// One running onion service, backed by either Arti or C `tor`.
pub(crate) enum Service {
    Arti(ArtiService),
    External(super::external_tor::ExternalService),
}

pub(crate) struct ArtiService {
    pub forward: OnionForward,
    task: tokio::task::JoinHandle<()>,
    watch: Option<tokio::task::JoinHandle<()>>,
    pub answered: Answered,
}

impl Service {
    pub fn onion_address(&self) -> Option<OnionAddress> {
        match self {
            Self::Arti(s) => s.forward.onion_address(),
            Self::External(s) => s.onion_address(),
        }
    }

    pub fn state(&self) -> String {
        match self {
            Self::Arti(s) => s.forward.state(),
            Self::External(s) => s.state(),
        }
    }

    pub fn is_published(&self) -> bool {
        match self {
            Self::Arti(s) => s.forward.is_published(),
            Self::External(_) => true,
        }
    }

    /// The Arti `OnionForward`, when this is an Arti service.
    pub fn arti_forward(&self) -> Option<&OnionForward> {
        match self {
            Self::Arti(s) => Some(&s.forward),
            Self::External(_) => None,
        }
    }

    pub fn set_clients(&self, clients: &[(String, String)]) -> Result<(), String> {
        match self {
            Self::Arti(s) => s
                .forward
                .set_clients(clients)
                .map_err(|e| e.to_string()),
            Self::External(_) => {
                // Client updates for external services go through
                // ExternalTor::update_clients, called from Services.
                Ok(())
            }
        }
    }

    pub fn answered(&self) -> &Answered {
        match self {
            Self::Arti(s) => &s.answered,
            Self::External(s) => &s.answered,
        }
    }

    pub fn set_watch(&mut self, handle: tokio::task::JoinHandle<()>) {
        if let Self::Arti(s) = self {
            s.watch = Some(handle);
        }
    }
}

impl Drop for ArtiService {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(watch) = &self.watch {
            watch.abort();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;

    /// Under `target/`, not the system temp dir, so Arti's permission check
    /// sees the directories this code made rather than `/tmp`.
    fn fresh_dir(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tor-dir-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        dir
    }

    #[test]
    fn remote_and_onion_clients_open_one_fresh_tor_dir_together() {
        for round in 0..4 {
            let dir = fresh_dir(&format!("race-{round}"));
            let onion_dir = dir.join("tor");
            let onion = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _inside = runtime.enter();
                tormail::open_client(&onion_dir)
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            });
            let remote = Tor::start_with_retry(&dir).map(|_| ());
            assert_eq!(remote, Ok(()), "round {round}");
            assert_eq!(onion.join().unwrap(), Ok(()), "round {round}");
            for sub in ["tor/state", "tor/cache"] {
                let mode = std::fs::metadata(dir.join(sub))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o700, "{sub}");
            }
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
