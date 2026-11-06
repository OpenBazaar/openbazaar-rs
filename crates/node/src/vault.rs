//! Encryption for records that stay on this machine.
//!
//! The key file is created on first boot, mode 0600, and never leaves the
//! data directory. Addresses and the buyer's order list are sealed with
//! ChaCha20-Poly1305. The public catalog is a different file.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use crate::Error;

const MAGIC: &[u8] = b"OBV1";

#[derive(Clone)]
pub(crate) struct Vault {
    key: Key,
}

impl Vault {
    pub(crate) fn open(dir: &Path) -> Result<Self, Error> {
        let path = dir.join("vault.key");
        let bytes = if path.exists() {
            fs::read(&path).map_err(|err| Error::Detail(err.to_string()))?
        } else {
            let mut bytes = vec![0u8; 32];
            StdRng::from_os_rng().fill_bytes(&mut bytes);
            write_secret(&path, &bytes)?;
            bytes
        };
        if bytes.len() != 32 {
            return Err(Error::Detail("the vault key is the wrong length".to_string()));
        }
        let mut key = Key::default();
        key.copy_from_slice(&bytes);
        Ok(Self { key })
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

fn write_secret(path: &Path, bytes: &[u8]) -> Result<(), Error> {
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
