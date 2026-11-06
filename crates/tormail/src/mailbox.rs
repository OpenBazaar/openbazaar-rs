use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use crate::seal::SealedLetter;
use crate::Error;

/// Identity of one sealed letter on disk.
///
/// The id is 16 random bytes, hex-encoded. It is not derived from the
/// sender and it is not a Tor identity.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LetterId([u8; 16]);

impl LetterId {
    fn generate() -> Self {
        let mut bytes = [0u8; 16];
        StdRng::from_os_rng().fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub fn parse(text: &str) -> Result<Self, Error> {
        if text.len() != 32 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::Malformed);
        }
        let mut bytes = [0u8; 16];
        for (index, chunk) in text.as_bytes().chunks(2).enumerate() {
            let hi = hex_val(chunk[0])?;
            let lo = hex_val(chunk[1])?;
            bytes[index] = (hi << 4) | lo;
        }
        Ok(Self(bytes))
    }

    pub fn as_str(&self) -> String {
        hex(&self.0)
    }
}

impl std::fmt::Display for LetterId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_str())
    }
}

impl std::fmt::Debug for LetterId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("LetterId")
            .field(&self.as_str())
            .finish()
    }
}

/// Directory of sealed letters waiting for the vendor's desktop app.
///
/// `accept` does not decrypt. The home node can hold mail while the
/// vendor is away without the store secret being loaded.
pub struct Mailbox {
    dir: PathBuf,
    lock: Mutex<()>,
}

impl Mailbox {
    /// Open `dir`, creating it if needed.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, Error> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            lock: Mutex::new(()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Persist a sealed letter and return its id.
    pub fn accept(&self, letter: &SealedLetter) -> Result<LetterId, Error> {
        let _guard = self.lock()?;
        let bytes = letter.to_bytes();
        for _ in 0..8 {
            let id = LetterId::generate();
            let path = self.letter_path(&id);
            let tmp = self.dir.join(format!("{}.letter.tmp", id.as_str()));
            let file = OpenOptions::new().write(true).create_new(true).open(&tmp);
            let mut file = match file {
                Ok(file) => file,
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err.into()),
            };
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp, &path)?;
            return Ok(id);
        }
        Err(Error::Mailbox("could not allocate a letter id".into()))
    }

    /// Ids currently stored, in hex order.
    pub fn list(&self) -> Result<Vec<LetterId>, Error> {
        let _guard = self.lock()?;
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(stem) = name.strip_suffix(".letter") else {
                continue;
            };
            if let Ok(id) = LetterId::parse(stem) {
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// Read one letter and leave it stored.
    pub fn read(&self, id: &LetterId) -> Result<SealedLetter, Error> {
        let _guard = self.lock()?;
        self.read_unlocked(id)
    }

    /// Read and delete one letter.
    pub fn take(&self, id: &LetterId) -> Result<SealedLetter, Error> {
        let _guard = self.lock()?;
        let letter = self.read_unlocked(id)?;
        fs::remove_file(self.letter_path(id))?;
        Ok(letter)
    }

    fn read_unlocked(&self, id: &LetterId) -> Result<SealedLetter, Error> {
        let bytes = match fs::read(self.letter_path(id)) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(Error::Missing),
            Err(err) => return Err(err.into()),
        };
        SealedLetter::from_bytes(&bytes)
    }

    fn letter_path(&self, id: &LetterId) -> PathBuf {
        self.dir.join(format!("{}.letter", id.as_str()))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>, Error> {
        self.lock
            .lock()
            .map_err(|_| Error::Mailbox("mailbox lock poisoned".into()))
    }
}

fn hex_val(byte: u8) -> Result<u8, Error> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(Error::Malformed),
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::order::{ListingHash, Order, PaymentRail};
    use crate::seal::{open_order, seal_order, ReplyKeypair, StoreKeypair};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("tormail-{nanos}-{}", std::process::id()));
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample_letter() -> (StoreKeypair, SealedLetter, Order) {
        let vendor = StoreKeypair::generate();
        let reply = ReplyKeypair::generate();
        let mut options = BTreeMap::new();
        options.insert("color".to_owned(), "green".to_owned());
        let order = Order::new(
            ListingHash([7u8; 32]),
            2,
            options,
            "SHIPTO-9f3c1a-buyer-visible-only-after-open",
            PaymentRail::Lightning,
            reply.public_key().clone(),
        )
        .unwrap();
        let sealed = seal_order(&order, vendor.public()).unwrap();
        (vendor, sealed, order)
    }

    #[test]
    fn accept_list_take_roundtrip_and_persist() {
        let dir = TempDir::new();
        let (vendor, sealed, order) = sample_letter();
        assert!(
            !sealed
                .to_bytes()
                .windows(order.ship_to.len())
                .any(|window| window == order.ship_to.as_bytes()),
            "the mailbox file is the sealed letter, not the ship-to"
        );

        let mailbox = Mailbox::open(&dir.0).unwrap();
        let first = mailbox.accept(&sealed).unwrap();
        let second = mailbox.accept(&sealed).unwrap();
        assert_ne!(first, second);
        let mut listed = mailbox.list().unwrap();
        listed.sort();
        assert_eq!(listed.len(), 2);

        drop(mailbox);
        let mailbox = Mailbox::open(&dir.0).unwrap();
        assert_eq!(mailbox.list().unwrap().len(), 2);

        let taken = mailbox.take(&first).unwrap();
        assert_eq!(open_order(&taken, vendor.secret()).unwrap(), order);
        assert!(mailbox.take(&first).is_err());
        assert_eq!(mailbox.list().unwrap(), vec![second]);
    }
}
