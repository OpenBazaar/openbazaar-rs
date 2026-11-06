//! Encryption for records that stay on this machine.
//!
//! The key comes from [`crate::vault_key`]: the OS keystore, a passphrase, or
//! (headless and tests only) `vault.key` in the data directory. Addresses,
//! orders, seeds, and wallet state are sealed with ChaCha20-Poly1305. The
//! public catalog is a different file.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use crate::vault_key::{self, VaultMode};
use crate::Error;

const MAGIC: &[u8] = b"OBV1";

#[derive(Clone)]
pub(crate) struct Vault {
    key: Key,
}

impl Vault {
    pub(crate) fn open(dir: &Path, mode: &VaultMode) -> Result<Self, Error> {
        let bytes = vault_key::load_key(dir, mode)?;
        Ok(Self {
            key: Key::clone_from_slice(&bytes),
        })
    }

    pub(crate) fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
        let cipher = ChaCha20Poly1305::new(&self.key);
        let mut nonce_bytes = [0u8; 12];
        StdRng::from_os_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|_| Error::Detail("the record could not be sealed".to_string()))?;
        let mut out = Vec::with_capacity(MAGIC.len() + nonce_bytes.len() + ciphertext.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    pub(crate) fn open_bytes(&self, blob: &[u8]) -> Result<Vec<u8>, Error> {
        if blob.len() < MAGIC.len() + 12 || !blob.starts_with(MAGIC) {
            return Err(Error::Detail("the sealed record could not be read".to_string()));
        }
        let nonce = Nonce::from_slice(&blob[MAGIC.len()..MAGIC.len() + 12]);
        let cipher = ChaCha20Poly1305::new(&self.key);
        cipher
            .decrypt(nonce, &blob[MAGIC.len() + 12..])
            .map_err(|_| Error::Detail("the sealed record could not be opened".to_string()))
    }
}

impl Vault {
    /// Seal `plaintext` into `path` (mode 0600), replacing it atomically.
    pub(crate) fn write_sealed(&self, path: &Path, plaintext: &[u8]) -> Result<(), Error> {
        let sealed = self.seal(plaintext)?;
        let tmp = path.with_extension("tmp");
        let _ = fs::remove_file(&tmp);
        write_secret(&tmp, &sealed)?;
        fs::rename(&tmp, path).map_err(|err| Error::Detail(err.to_string()))
    }

    /// Open a file written by [`Vault::write_sealed`]. `None` when it does not exist.
    pub(crate) fn read_sealed(&self, path: &Path) -> Result<Option<Vec<u8>>, Error> {
        match fs::read(path) {
            Ok(blob) => self.open_bytes(&blob).map(Some),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(Error::Detail(err.to_string())),
        }
    }
}

pub(crate) fn write_secret(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|err| Error::Detail(err.to_string()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|err| Error::Detail(err.to_string()))
}
