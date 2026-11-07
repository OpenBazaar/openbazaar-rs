//! Embedded Arti for the OpenBazaar phone app.
//!
//! One Tor client per process, on its own Tokio runtime, with its state,
//! cache, and keystore in a directory the app owns. The app reaches its home
//! node with [`exchange`]: one HTTP/1.1 request per stream, the whole answer
//! back. Swift calls the C ABI in [`ffi`]; Kotlin calls the JNI functions in
//! `jni.rs`.

mod base32;
pub mod ffi;
#[cfg(target_os = "android")]
mod jni;

use std::path::Path;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use arti_client::config::TorClientConfigBuilder;
use arti_client::{BootstrapBehavior, KeystoreSelector, TorClient};
use tor_hscrypto::pk::{HsClientDescEncSecretKey, HsId};
use tor_llcrypto::pk::curve25519::StaticSecret;
use tor_rtcompat::tokio::TokioRustlsRuntime;

type Client = std::sync::Arc<TorClient<TokioRustlsRuntime>>;

struct Tor {
    runtime: tokio::runtime::Runtime,
    client: Client,
    failure: Mutex<Option<String>>,
}

static TOR: OnceLock<Tor> = OnceLock::new();
static STARTING: Mutex<()> = Mutex::new(());

/// Where bootstrapping is, for the app's "Connecting privately…" badge.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub state: &'static str,
    pub progress: f32,
    pub summary: String,
}

impl Status {
    pub fn to_json(&self) -> String {
        format!(
            "{{\"state\":\"{}\",\"progress\":{:.3},\"summary\":\"{}\"}}",
            self.state,
            self.progress,
            json_escape(&self.summary)
        )
    }
}

impl Tor {
    fn create(data_dir: &Path) -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ob-tor")
            .enable_all()
            .build()
            .map_err(|err| err.to_string())?;
        let mut builder = TorClientConfigBuilder::from_directories(
            data_dir.join("state"),
            data_dir.join("cache"),
        );
        // The app sandbox is the boundary; parents such as /data/user/0 are
        // group-writable and would fail Arti's permission check.
        builder.storage().permissions().dangerously_trust_everyone();
        let config = builder.build().map_err(|err| err.to_string())?;
        let client = {
            let _inside = runtime.enter();
            let rt = TokioRustlsRuntime::current().map_err(|err| err.to_string())?;
            TorClient::with_runtime(rt)
                .config(config)
                .bootstrap_behavior(BootstrapBehavior::OnDemand)
                .create_unbootstrapped()
                .map_err(|err| err.to_string())?
        };
        Ok(Self {
            runtime,
            client,
            failure: Mutex::new(None),
        })
    }

    fn bootstrap_in_background(&'static self) {
        self.runtime.spawn(async move {
            if let Err(err) = self.client.bootstrap().await {
                *self.failure.lock().unwrap() = Some(err.to_string());
            }
        });
    }

    fn status(&self) -> Status {
        let status = self.client.bootstrap_status();
        if status.ready_for_traffic() {
            return Status {
                state: "ready",
                progress: 1.0,
                summary: status.to_string(),
            };
        }
        if let Some(err) = self.failure.lock().unwrap().clone() {
            return Status {
                state: "failed",
                progress: status.as_frac(),
                summary: err,
            };
        }
        Status {
            state: "bootstrapping",
            progress: status.as_frac(),
            summary: status.to_string(),
        }
    }

    fn insert_discovery_key(&self, onion: &str, secret_base32: &str) -> Result<(), String> {
        let hsid = parse_onion(onion)?;
        let bytes: [u8; 32] = base32::decode(secret_base32)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or("the discovery secret is not 32 bytes of base32")?;
        let secret = HsClientDescEncSecretKey::from(StaticSecret::from(bytes));
        let _ = self
            .client
            .remove_service_discovery_key(KeystoreSelector::Primary, hsid);
        self.client
            .insert_service_discovery_key(KeystoreSelector::Primary, hsid, secret)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    fn exchange(
        &self,
        onion: &str,
        port: u16,
        request: &[u8],
        max: usize,
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        let host = format!("{}.onion", onion.trim_end_matches(".onion"));
        parse_onion(&host)?;
        self.runtime.block_on(async {
            match tokio::time::timeout(
                timeout,
                exchange_inner(&self.client, &host, port, request, max),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err("the home node did not answer over Tor in time".to_string()),
            }
        })
    }
}

const AUTH_RETRIES: u32 = 4;
const AUTH_RETRY_DELAY: Duration = Duration::from_secs(8);

/// Connect to `host:port`. Right after pairing, the service may still publish a
/// descriptor without this device's discovery key; Arti then reports the key
/// as incorrect, so wait and fetch a fresh descriptor on a new isolation group.
async fn connect(
    client: &TorClient<TokioRustlsRuntime>,
    host: &str,
    port: u16,
) -> Result<arti_client::DataStream, String> {
    let mut attempt = 0;
    loop {
        let fresh;
        let dialer = if attempt == 0 {
            client
        } else {
            fresh = client.isolated_client();
            &fresh
        };
        match dialer.connect((host, port)).await {
            Ok(stream) => return Ok(stream),
            Err(err) => {
                let text = err.to_string();
                if attempt >= AUTH_RETRIES || !text.contains("authentication was incorrect") {
                    return Err(text);
                }
            }
        }
        attempt += 1;
        tokio::time::sleep(AUTH_RETRY_DELAY).await;
    }
}

async fn exchange_inner(
    client: &TorClient<TokioRustlsRuntime>,
    host: &str,
    port: u16,
    request: &[u8],
    max: usize,
) -> Result<Vec<u8>, String> {
    use futures::{AsyncReadExt, AsyncWriteExt};
    client.bootstrap().await.map_err(|err| err.to_string())?;
    let mut stream = connect(client, host, port).await?;
    stream
        .write_all(request)
        .await
        .map_err(|err| err.to_string())?;
    stream.flush().await.map_err(|err| err.to_string())?;
    let mut answer = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        // A service that closes its side ends the stream with an END cell
        // Arti reads as an error, after every data cell. What arrived is the
        // answer; the caller checks it is whole (Content-Length).
        let read = match stream.read(&mut chunk).await {
            Ok(read) => read,
            Err(_) if !answer.is_empty() => 0,
            Err(err) => return Err(err.to_string()),
        };
        if read == 0 {
            break;
        }
        if answer.len() + read > max {
            return Err("the answer is larger than allowed".to_string());
        }
        answer.extend_from_slice(&chunk[..read]);
    }
    Ok(answer)
}

fn parse_onion(onion: &str) -> Result<HsId, String> {
    let host = format!(
        "{}.onion",
        onion.trim().trim_end_matches(".onion").to_ascii_lowercase()
    );
    HsId::from_str(&host).map_err(|_| "not a v3 onion address".to_string())
}

fn json_escape(text: &str) -> String {
    text.chars()
        .flat_map(|ch| match ch {
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect(),
            ch if (ch as u32) < 0x20 => format!("\\u{:04x}", ch as u32).chars().collect(),
            ch => vec![ch],
        })
        .collect()
}

/// Create the client (once) and bootstrap it in the background.
pub fn start(data_dir: &Path) -> Result<(), String> {
    let _guard = STARTING.lock().unwrap();
    if let Some(tor) = TOR.get() {
        if tor.failure.lock().unwrap().take().is_some() {
            tor.bootstrap_in_background();
        }
        return Ok(());
    }
    let _ = TOR.set(Tor::create(data_dir)?);
    TOR.get().expect("set above").bootstrap_in_background();
    Ok(())
}

pub fn status() -> Status {
    match TOR.get() {
        Some(tor) => tor.status(),
        None => Status {
            state: "stopped",
            progress: 0.0,
            summary: String::new(),
        },
    }
}

/// Keep the client-authorization key for `onion` in Arti's keystore, replacing any older one.
pub fn insert_discovery_key(onion: &str, secret_base32: &str) -> Result<(), String> {
    TOR.get()
        .ok_or("Tor is not started")?
        .insert_discovery_key(onion, secret_base32)
}

/// One request to `onion:port`; the bytes the service sent before closing.
pub fn exchange(
    onion: &str,
    port: u16,
    request: &[u8],
    max: usize,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    TOR.get()
        .ok_or("Tor is not started")?
        .exchange(onion, port, request, max, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONION: &str = "2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid";

    #[test]
    fn keeps_a_discovery_key_offline() {
        let dir = tempfile::tempdir().unwrap();
        let tor = Tor::create(dir.path()).unwrap();
        let secret = base32::encode(&[7u8; 32]);
        tor.insert_discovery_key(ONION, &secret).unwrap();
        tor.insert_discovery_key(&format!("{ONION}.onion"), &secret)
            .unwrap();
        let hsid = parse_onion(ONION).unwrap();
        let public = tor
            .client
            .get_service_discovery_key(hsid)
            .unwrap()
            .expect("stored");
        let expected =
            tor_llcrypto::pk::curve25519::PublicKey::from(&StaticSecret::from([7u8; 32]));
        assert_eq!(public.as_bytes(), expected.as_bytes());
        assert!(tor.insert_discovery_key("not-an-onion", &secret).is_err());
        assert!(tor.insert_discovery_key(ONION, "abc").is_err());
    }

    #[test]
    fn refuses_bad_addresses_before_dialing() {
        let dir = tempfile::tempdir().unwrap();
        let tor = Tor::create(dir.path()).unwrap();
        let err = tor
            .exchange(
                "example.com",
                80,
                b"GET / HTTP/1.1\r\n\r\n",
                1024,
                Duration::from_secs(1),
            )
            .unwrap_err();
        assert_eq!(err, "not a v3 onion address");
        assert_eq!(tor.status().state, "bootstrapping");
    }

    #[test]
    fn status_is_json() {
        let status = Status {
            state: "failed",
            progress: 0.5,
            summary: "a \"b\"\n".into(),
        };
        assert_eq!(
            status.to_json(),
            r#"{"state":"failed","progress":0.500,"summary":"a \"b\"\u000a"}"#
        );
    }
}
