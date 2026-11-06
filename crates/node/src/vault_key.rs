//! Where the vault key lives, and the permission check on secret files.
//!
//! The 256-bit vault key encrypts every `.obv` file. It is kept in one of:
//!
//! - **keystore** (default): the OS credential store (macOS Keychain, Windows
//!   Credential Manager, Linux Secret Service). The data dir holds only
//!   `vault.keystore`, which names the entry.
//! - **passphrase**: `vault.lock`, the key encrypted under an Argon2id hash
//!   of a passphrase the user types at start.
//! - **file**: `vault.key`, the raw key next to the files it encrypts. Only
//!   with `--vault file`, for headless machines and tests.
//!
//! A data dir from before this change has `vault.key`. Opening it in keystore
//! or passphrase mode moves the key there, checks it reads back, and then
//! overwrites and deletes `vault.key`. A dir already in one mode is never
//! silently opened in another, and a missing key is an error, not a new key:
//! a new key would leave every sealed file unreadable.

use std::fs;
use std::path::Path;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use crate::vault::write_secret;
use crate::Error;

const KEY_FILE: &str = "vault.key";
const KEYSTORE_FILE: &str = "vault.keystore";
const LOCK_FILE: &str = "vault.lock";
const KEYSTORE_MAGIC: &str = "OBK1";
const LOCK_MAGIC: &[u8; 4] = b"OBP1";
const LOCK_HEADER: usize = 4 + 12 + 16;
const LOCK_LEN: usize = LOCK_HEADER + 12 + 32 + 16;
const KEYRING_SERVICE: &str = "OpenBazaar node vault";

/// Files a node writes that must stay private to its user.
const SECRET_NAMES: [&str; 3] = [KEY_FILE, LOCK_FILE, "pairing.token"];

#[derive(Clone)]
pub enum VaultMode {
    Keystore,
    Passphrase(String),
    /// `vault.key` beside the data. Headless runs and tests only.
    File,
}

impl std::fmt::Debug for VaultMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            VaultMode::Keystore => "Keystore",
            VaultMode::Passphrase(_) => "Passphrase(..)",
            VaultMode::File => "File",
        })
    }
}

/// Argon2id cost: memory in KiB, passes, lanes.
#[derive(Clone, Copy)]
pub(crate) struct Cost {
    pub memory_kib: u32,
    pub passes: u32,
    pub lanes: u32,
}

pub(crate) const DEFAULT_COST: Cost = Cost {
    memory_kib: 64 * 1024,
    passes: 3,
    lanes: 1,
};

/// The OS credential store, or a stand-in in tests.
pub(crate) trait KeyStore {
    fn get(&self, account: &str) -> Result<Option<[u8; 32]>, String>;
    fn set(&self, account: &str, key: &[u8; 32]) -> Result<(), String>;
}

pub(crate) struct OsKeyStore;

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
impl KeyStore for OsKeyStore {
    fn get(&self, account: &str) -> Result<Option<[u8; 32]>, String> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, account).map_err(|err| err.to_string())?;
        match entry.get_password() {
            Ok(hex) => decode_hex_key(&hex).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(err.to_string()),
        }
    }

    fn set(&self, account: &str, key: &[u8; 32]) -> Result<(), String> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, account).map_err(|err| err.to_string())?;
        entry.set_password(&encode_hex(key)).map_err(|err| err.to_string())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
impl KeyStore for OsKeyStore {
    fn get(&self, _: &str) -> Result<Option<[u8; 32]>, String> {
        Err("this platform has no supported OS keystore".into())
    }
    fn set(&self, _: &str, _: &[u8; 32]) -> Result<(), String> {
        Err("this platform has no supported OS keystore".into())
    }
}

pub(crate) fn load_key(dir: &Path, mode: &VaultMode) -> Result<[u8; 32], Error> {
    load_key_with(dir, mode, &OsKeyStore, DEFAULT_COST)
}

pub(crate) fn load_key_with(dir: &Path, mode: &VaultMode, store: &dyn KeyStore, cost: Cost) -> Result<[u8; 32], Error> {
    let legacy = dir.join(KEY_FILE);
    let marker = dir.join(KEYSTORE_FILE);
    let lock = dir.join(LOCK_FILE);
    match mode {
        VaultMode::File => {
            refuse_other_mode(&marker, "keystore")?;
            refuse_other_mode(&lock, "passphrase")?;
            match read_key_file(&legacy)? {
                Some(key) => Ok(key),
                None => {
                    let key = random_key();
                    write_secret(&legacy, &key)?;
                    Ok(key)
                }
            }
        }
        VaultMode::Keystore => {
            refuse_other_mode(&lock, "passphrase")?;
            if let Some(account) = read_marker(&marker)? {
                let key = store
                    .get(&account)
                    .map_err(keystore_error)?
                    .ok_or_else(|| detail("The OS keystore has no entry for this data dir's vault key. Restore it, or the sealed files cannot be read."))?;
                if read_key_file(&legacy)? == Some(key) {
                    retire(&legacy, "the OS keystore")?;
                }
                return Ok(key);
            }
            let key = read_key_file(&legacy)?.unwrap_or_else(random_key);
            let account = encode_hex(&random_bytes::<16>());
            store.set(&account, &key).map_err(keystore_error)?;
            if store.get(&account).map_err(keystore_error)? != Some(key) {
                return Err(detail("The OS keystore did not return the vault key it was given."));
            }
            replace_file(&marker, format!("{KEYSTORE_MAGIC} {account}\n").as_bytes())?;
            retire(&legacy, "the OS keystore")
                .map(|()| key)
        }
        VaultMode::Passphrase(passphrase) => {
            refuse_other_mode(&marker, "keystore")?;
            if passphrase.is_empty() {
                return Err(detail("The vault passphrase is empty."));
            }
            if let Some(blob) = read_optional(&lock)? {
                return unlock(&blob, passphrase);
            }
            let key = read_key_file(&legacy)?.unwrap_or_else(random_key);
            let blob = lock_key(&key, passphrase, cost)?;
            if unlock(&blob, passphrase)? != key {
                return Err(detail("The vault lock did not open with its own passphrase."));
            }
            replace_file(&lock, &blob)?;
            retire(&legacy, "vault.lock").map(|()| key)
        }
    }
}

fn refuse_other_mode(path: &Path, mode: &str) -> Result<(), Error> {
    if path.exists() {
        return Err(detail(&format!("This data dir keeps its vault key in {mode} mode. Start the node with --vault {mode}.")));
    }
    Ok(())
}

fn keystore_error(err: String) -> Error {
    Error::Detail(format!(
        "The OS keystore is not available ({err}). Use --vault passphrase, or --vault file on a headless machine."
    ))
}

fn detail(message: &str) -> Error {
    Error::Detail(message.to_string())
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::Detail(err.to_string())),
    }
}

fn read_key_file(path: &Path) -> Result<Option<[u8; 32]>, Error> {
    match read_optional(path)? {
        None => Ok(None),
        Some(bytes) => bytes
            .try_into()
            .map(Some)
            .map_err(|_| detail("The vault key is the wrong length.")),
    }
}

fn read_marker(path: &Path) -> Result<Option<String>, Error> {
    let Some(bytes) = read_optional(path)? else {
        return Ok(None);
    };
    let text = String::from_utf8(bytes).map_err(|_| detail("vault.keystore could not be read."))?;
    match text.trim().split_once(' ') {
        Some((KEYSTORE_MAGIC, account)) if account.len() == 32 && account.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(Some(account.to_string()))
        }
        _ => Err(detail("vault.keystore could not be read.")),
    }
}

/// Write `bytes` to `path` (mode 0600) through a temporary file.
fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    write_secret(&tmp, bytes)?;
    fs::rename(&tmp, path).map_err(|err| Error::Detail(err.to_string()))
}

/// Overwrite and delete the old key file once the key is safe elsewhere.
fn retire(legacy: &Path, moved_to: &str) -> Result<(), Error> {
    if !legacy.exists() {
        return Ok(());
    }
    let zeros = [0u8; 32];
    fs::write(legacy, zeros)
        .and_then(|()| fs::remove_file(legacy))
        .map_err(|err| Error::Detail(format!("the old vault.key could not be removed: {err}")))?;
    eprintln!("ob-node: moved the vault key from vault.key to {moved_to}");
    Ok(())
}

fn argon2(passphrase: &str, salt: &[u8], cost: Cost) -> Result<Key, Error> {
    let params = Params::new(cost.memory_kib, cost.passes, cost.lanes, Some(32))
        .map_err(|_| detail("The vault lock has unusable Argon2 settings."))?;
    let mut out = Key::default();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|_| detail("The vault passphrase could not be hashed."))?;
    Ok(out)
}

/// `OBP1 | memory | passes | lanes | salt[16] | nonce[12] | sealed key`.
/// The header is the AEAD associated data, so its costs cannot be lowered.
fn lock_key(key: &[u8; 32], passphrase: &str, cost: Cost) -> Result<Vec<u8>, Error> {
    let salt = random_bytes::<16>();
    let nonce = random_bytes::<12>();
    let mut blob = Vec::with_capacity(LOCK_LEN);
    blob.extend_from_slice(LOCK_MAGIC);
    for value in [cost.memory_kib, cost.passes, cost.lanes] {
        blob.extend_from_slice(&value.to_le_bytes());
    }
    blob.extend_from_slice(&salt);
    let wrap = argon2(passphrase, &salt, cost)?;
    let sealed = ChaCha20Poly1305::new(&wrap)
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: key, aad: &blob })
        .map_err(|_| detail("The vault key could not be locked."))?;
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&sealed);
    Ok(blob)
}

fn unlock(blob: &[u8], passphrase: &str) -> Result<[u8; 32], Error> {
    let unreadable = || detail("vault.lock could not be read.");
    if blob.len() != LOCK_LEN || &blob[..4] != LOCK_MAGIC {
        return Err(unreadable());
    }
    let word = |at: usize| u32::from_le_bytes([blob[at], blob[at + 1], blob[at + 2], blob[at + 3]]);
    let cost = Cost {
        memory_kib: word(4),
        passes: word(8),
        lanes: word(12),
    };
    if cost.memory_kib > 1024 * 1024 || cost.passes > 64 || cost.lanes > 16 {
        return Err(unreadable());
    }
    let (header, rest) = blob.split_at(LOCK_HEADER);
    let wrap = argon2(passphrase, &header[16..], cost)?;
    let key = ChaCha20Poly1305::new(&wrap)
        .decrypt(Nonce::from_slice(&rest[..12]), Payload { msg: &rest[12..], aad: header })
        .map_err(|_| detail("The vault passphrase is wrong."))?;
    key.try_into().map_err(|_| unreadable())
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    StdRng::from_os_rng().fill_bytes(&mut bytes);
    bytes
}

fn random_key() -> [u8; 32] {
    random_bytes()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows", target_os = "linux")), allow(dead_code))]
fn decode_hex_key(text: &str) -> Result<[u8; 32], String> {
    let text = text.trim();
    if text.len() != 64 || !text.is_ascii() {
        return Err("the keystore entry is not a vault key".into());
    }
    let mut key = [0u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| "the keystore entry is not a vault key".to_string())?;
    }
    Ok(key)
}

/// Make the data dir owner-only and refuse to start when the vault key, the
/// vault lock, or the pairing token can be read by other users. Sealed `.obv`
/// files are tightened to 0600 instead: they are useless without the key, and
/// earlier nodes wrote some of them with the umask.
pub(crate) fn check_private(dir: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir).map_err(|err| Error::Detail(err.to_string()))?.permissions().mode();
        if mode & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|err| Error::Detail(err.to_string()))?;
        }
        let mut files = Vec::new();
        collect_secrets(dir, &mut files);
        if let Ok(wallets) = fs::read_dir(dir.join("wallets")) {
            for wallet in wallets.flatten() {
                collect_secrets(&wallet.path(), &mut files);
            }
        }
        for path in files {
            let mode = fs::symlink_metadata(&path).map_err(|err| Error::Detail(err.to_string()))?.permissions().mode();
            if mode & 0o077 != 0 && path.extension().is_some_and(|ext| ext == "obv") {
                // Earlier nodes wrote addresses.obv and orders.obv with the umask.
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|err| Error::Detail(err.to_string()))?;
                eprintln!("ob-node: {} was readable by other users; it is now mode 600", path.display());
                continue;
            }
            if mode & 0o077 != 0 {
                return Err(Error::Detail(format!(
                    "{} can be read or changed by other users (mode {:o}). Run chmod 600 on it, then start the node again.",
                    path.display(),
                    mode & 0o777
                )));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

#[cfg(unix)]
fn collect_secrets(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if SECRET_NAMES.contains(&name.as_ref()) || name.ends_with(".obv") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::TempDir;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    const FAST: Cost = Cost {
        memory_kib: 64,
        passes: 1,
        lanes: 1,
    };

    #[derive(Default)]
    struct MemoryStore(RefCell<BTreeMap<String, [u8; 32]>>);

    impl KeyStore for MemoryStore {
        fn get(&self, account: &str) -> Result<Option<[u8; 32]>, String> {
            Ok(self.0.borrow().get(account).copied())
        }
        fn set(&self, account: &str, key: &[u8; 32]) -> Result<(), String> {
            self.0.borrow_mut().insert(account.to_string(), *key);
            Ok(())
        }
    }

    struct NoStore;

    impl KeyStore for NoStore {
        fn get(&self, _: &str) -> Result<Option<[u8; 32]>, String> {
            Err("no secret service".into())
        }
        fn set(&self, _: &str, _: &[u8; 32]) -> Result<(), String> {
            Err("no secret service".into())
        }
    }

    fn dir() -> TempDir {
        let dir = TempDir::new();
        fs::create_dir_all(&dir.0).unwrap();
        dir
    }

    #[test]
    fn keystore_mode_keeps_no_key_in_the_data_dir() {
        let dir = dir();
        let store = MemoryStore::default();
        let key = load_key_with(&dir.0, &VaultMode::Keystore, &store, FAST).unwrap();
        assert!(!dir.0.join(KEY_FILE).exists());
        assert!(dir.0.join(KEYSTORE_FILE).exists());
        assert_eq!(load_key_with(&dir.0, &VaultMode::Keystore, &store, FAST).unwrap(), key);
        let other = MemoryStore::default();
        assert!(load_key_with(&dir.0, &VaultMode::Keystore, &other, FAST).is_err(), "a missing entry is not a new key");
        assert!(load_key_with(&dir.0, &VaultMode::File, &store, FAST).is_err());
        assert!(load_key_with(&dir.0, &VaultMode::Passphrase("x".into()), &store, FAST).is_err());
    }

    #[test]
    fn an_old_vault_key_moves_into_the_keystore() {
        let dir = dir();
        let old = load_key_with(&dir.0, &VaultMode::File, &NoStore, FAST).unwrap();
        assert!(dir.0.join(KEY_FILE).exists());
        assert!(load_key_with(&dir.0, &VaultMode::Keystore, &NoStore, FAST).is_err());
        assert!(dir.0.join(KEY_FILE).exists(), "a failed move keeps the old file");
        let store = MemoryStore::default();
        assert_eq!(load_key_with(&dir.0, &VaultMode::Keystore, &store, FAST).unwrap(), old);
        assert!(!dir.0.join(KEY_FILE).exists());
    }

    #[test]
    fn passphrase_mode_locks_the_key_with_argon2id() {
        let dir = dir();
        let old = load_key_with(&dir.0, &VaultMode::File, &NoStore, FAST).unwrap();
        let mode = VaultMode::Passphrase("correct horse".into());
        assert_eq!(load_key_with(&dir.0, &mode, &NoStore, FAST).unwrap(), old);
        assert!(!dir.0.join(KEY_FILE).exists());
        let blob = fs::read(dir.0.join(LOCK_FILE)).unwrap();
        assert!(!blob.windows(32).any(|window| window == old), "the key is not in the lock in the clear");
        assert_eq!(load_key_with(&dir.0, &mode, &NoStore, FAST).unwrap(), old);
        let wrong = load_key_with(&dir.0, &VaultMode::Passphrase("wrong".into()), &NoStore, FAST);
        assert!(wrong.unwrap_err().to_string().contains("wrong"));

        let mut cheaper = blob.clone();
        cheaper[8] = 0;
        cheaper[4] = 8;
        fs::write(dir.0.join(LOCK_FILE), &cheaper).unwrap();
        assert!(load_key_with(&dir.0, &mode, &NoStore, FAST).is_err(), "the cost is authenticated");
        assert!(load_key_with(&dir.0, &VaultMode::Keystore, &MemoryStore::default(), FAST).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn world_readable_secrets_stop_the_node() {
        use std::os::unix::fs::PermissionsExt;
        let dir = dir();
        load_key_with(&dir.0, &VaultMode::File, &NoStore, FAST).unwrap();
        fs::create_dir_all(dir.0.join("wallets/node")).unwrap();
        fs::write(dir.0.join("wallets/node/seed.obv"), b"sealed").unwrap();
        fs::set_permissions(dir.0.join("wallets/node/seed.obv"), fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
        check_private(&dir.0).unwrap();
        assert_eq!(fs::metadata(&dir.0).unwrap().permissions().mode() & 0o777, 0o700);

        fs::set_permissions(dir.0.join(KEY_FILE), fs::Permissions::from_mode(0o644)).unwrap();
        let err = check_private(&dir.0).unwrap_err().to_string();
        assert!(err.contains("vault.key") && err.contains("644"), "{err}");
        fs::set_permissions(dir.0.join(KEY_FILE), fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(dir.0.join("wallets/node/seed.obv"), fs::Permissions::from_mode(0o644)).unwrap();
        check_private(&dir.0).unwrap();
        let mode = fs::metadata(dir.0.join("wallets/node/seed.obv")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        fs::write(dir.0.join("pairing.token"), b"x").unwrap();
        fs::set_permissions(dir.0.join("pairing.token"), fs::Permissions::from_mode(0o640)).unwrap();
        assert!(check_private(&dir.0).is_err());
    }
}
