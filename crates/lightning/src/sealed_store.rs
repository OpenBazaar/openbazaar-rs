//! LDK's key-value store, sealed value by value.
//!
//! Wraps `lightning_persister`'s filesystem store. Each value is written as
//! `0x01 || nonce (12) || ChaCha20-Poly1305(value)`, with the namespace and
//! key as associated data, so a sealed file moved to another key does not
//! open. Writes keep LDK's ordering guarantees because the inner store's
//! write is called with the sealed bytes before the future is returned.
//!
//! Namespaces and keys stay in the clear (they name channel monitors and
//! payment ids, which are hashes); values (channel state, the channel
//! manager, payment records, the wallet) do not.

use std::path::PathBuf;
use std::sync::Arc;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use ldk_node::lightning::io;
use ldk_node::lightning::util::persist::{KVStore, KVStoreSync};
use lightning_persister::fs_store::FilesystemStore;
use rand::RngCore;

type AsyncResult<T> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, io::Error>> + 'static + Send>>;

const VERSION: u8 = 1;
const NONCE_LEN: usize = 12;
const AAD_TAG: &[u8] = b"ob-ln-store/v1";

pub struct SealedStore {
    inner: Arc<FilesystemStore>,
    cipher: ChaCha20Poly1305,
}

impl SealedStore {
    pub fn new(dir: PathBuf, key: &[u8; 32]) -> Self {
        Self { inner: Arc::new(FilesystemStore::new(dir)), cipher: ChaCha20Poly1305::new(Key::from_slice(key)) }
    }
}

fn aad(primary: &str, secondary: &str, key: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(AAD_TAG.len() + primary.len() + secondary.len() + key.len() + 3);
    for part in [AAD_TAG, primary.as_bytes(), secondary.as_bytes(), key.as_bytes()] {
        out.extend_from_slice(part);
        out.push(0);
    }
    out
}

fn seal(cipher: &ChaCha20Poly1305, aad: &[u8], plain: &[u8]) -> Result<Vec<u8>, io::Error> {
    let mut nonce = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    let sealed = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "lightning state could not be sealed"))?;
    let mut out = Vec::with_capacity(1 + NONCE_LEN + sealed.len());
    out.push(VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

fn open(cipher: &ChaCha20Poly1305, aad: &[u8], bytes: &[u8]) -> Result<Vec<u8>, io::Error> {
    if bytes.len() < 1 + NONCE_LEN || bytes[0] != VERSION {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "lightning state is not sealed with this format"));
    }
    cipher
        .decrypt(Nonce::from_slice(&bytes[1..1 + NONCE_LEN]), Payload { msg: &bytes[1 + NONCE_LEN..], aad })
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "lightning state did not open with this key"))
}

impl KVStoreSync for SealedStore {
    fn read(&self, primary: &str, secondary: &str, key: &str) -> Result<Vec<u8>, io::Error> {
        let bytes = KVStoreSync::read(&*self.inner, primary, secondary, key)?;
        open(&self.cipher, &aad(primary, secondary, key), &bytes)
    }

    fn write(&self, primary: &str, secondary: &str, key: &str, buf: Vec<u8>) -> Result<(), io::Error> {
        let sealed = seal(&self.cipher, &aad(primary, secondary, key), &buf)?;
        KVStoreSync::write(&*self.inner, primary, secondary, key, sealed)
    }

    fn remove(&self, primary: &str, secondary: &str, key: &str, lazy: bool) -> Result<(), io::Error> {
        KVStoreSync::remove(&*self.inner, primary, secondary, key, lazy)
    }

    fn list(&self, primary: &str, secondary: &str) -> Result<Vec<String>, io::Error> {
        KVStoreSync::list(&*self.inner, primary, secondary)
    }
}

impl KVStore for SealedStore {
    fn read(&self, primary: &str, secondary: &str, key: &str) -> AsyncResult<Vec<u8>> {
        let read = KVStore::read(&*self.inner, primary, secondary, key);
        let cipher = self.cipher.clone();
        let aad = aad(primary, secondary, key);
        Box::pin(async move { open(&cipher, &aad, &read.await?) })
    }

    fn write(&self, primary: &str, secondary: &str, key: &str, buf: Vec<u8>) -> AsyncResult<()> {
        match seal(&self.cipher, &aad(primary, secondary, key), &buf) {
            Ok(sealed) => KVStore::write(&*self.inner, primary, secondary, key, sealed),
            Err(err) => Box::pin(async move { Err(err) }),
        }
    }

    fn remove(&self, primary: &str, secondary: &str, key: &str, lazy: bool) -> AsyncResult<()> {
        KVStore::remove(&*self.inner, primary, secondary, key, lazy)
    }

    fn list(&self, primary: &str, secondary: &str) -> AsyncResult<Vec<String>> {
        KVStore::list(&*self.inner, primary, secondary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_sealed_and_bound_to_their_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = SealedStore::new(dir.path().to_path_buf(), &[3u8; 32]);
        KVStoreSync::write(&store, "monitors", "", "abc_0", b"channel state".to_vec()).unwrap();
        assert_eq!(KVStoreSync::read(&store, "monitors", "", "abc_0").unwrap(), b"channel state");
        assert_eq!(KVStoreSync::list(&store, "monitors", "").unwrap(), vec!["abc_0".to_string()]);

        let raw = std::fs::read(dir.path().join("monitors").join("abc_0")).unwrap();
        assert!(!raw.windows(13).any(|window| window == b"channel state"), "the value is on disk in the clear");

        std::fs::write(dir.path().join("monitors").join("abc_1"), &raw).unwrap();
        assert!(KVStoreSync::read(&store, "monitors", "", "abc_1").is_err(), "a moved file opened under another key");

        let other = SealedStore::new(dir.path().to_path_buf(), &[4u8; 32]);
        assert!(KVStoreSync::read(&other, "monitors", "", "abc_0").is_err(), "another key opened the value");

        KVStoreSync::remove(&store, "monitors", "", "abc_0", false).unwrap();
        assert_eq!(KVStoreSync::read(&store, "monitors", "", "abc_0").unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
