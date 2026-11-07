//! What a recipient remembers so that authenticated letters stay authentic:
//! the message ids it has accepted, and the sender key each conversation
//! started with.
//!
//! Both are append-only files, so a restart does not reopen a replay window
//! or let a new key take over a conversation. They hold random ids and
//! per-conversation keys only, nothing that names a person. A seen id costs
//! 16 bytes; a pin costs 49.

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::letter::{MessageId, SenderId};
use crate::Error;

const PIN_LEN: usize = 16 + 1 + 32;

fn append(path: &Path, record: &[u8]) -> Result<(), Error> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(record)?;
    file.sync_all()?;
    Ok(())
}

fn read_records(path: &Path, len: usize) -> Result<Vec<Vec<u8>>, Error> {
    match fs::read(path) {
        // A torn final record from a crash mid-write is dropped.
        Ok(bytes) => Ok(bytes.chunks_exact(len).map(<[u8]>::to_vec).collect()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err.into()),
    }
}

/// Message ids already accepted.
pub struct ReplayGuard {
    path: PathBuf,
    seen: Mutex<HashSet<MessageId>>,
}

impl ReplayGuard {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, Error> {
        let path = path.into();
        let seen = read_records(&path, 16)?
            .into_iter()
            .map(|record| MessageId(record.try_into().expect("16 bytes")))
            .collect();
        Ok(Self {
            path,
            seen: Mutex::new(seen),
        })
    }

    /// Record `id`, or refuse it with [`Error::Replay`] if it was recorded before.
    pub fn admit(&self, id: &MessageId) -> Result<(), Error> {
        let mut seen = lock(&self.seen)?;
        if seen.contains(id) {
            return Err(Error::Replay);
        }
        append(&self.path, &id.0)?;
        seen.insert(*id);
        Ok(())
    }
}

/// The sender key each conversation is bound to, per peer role.
///
/// A conversation is a 16-byte id both sides know (an escrow id); the role
/// separates the two or three peers that write into one conversation.
pub struct SenderPins {
    path: PathBuf,
    pins: Mutex<HashMap<([u8; 16], u8), SenderId>>,
}

impl SenderPins {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, Error> {
        let path = path.into();
        let mut pins = HashMap::new();
        for record in read_records(&path, PIN_LEN)? {
            let conversation: [u8; 16] = record[..16].try_into().expect("16 bytes");
            let sender = SenderId(record[17..].try_into().expect("32 bytes"));
            pins.entry((conversation, record[16])).or_insert(sender);
        }
        Ok(Self {
            path,
            pins: Mutex::new(pins),
        })
    }

    /// Refuse `sender` with [`Error::WrongSender`] if the conversation is
    /// pinned to another key. An unpinned conversation passes.
    pub fn check(&self, conversation: &[u8; 16], role: u8, sender: &SenderId) -> Result<(), Error> {
        match lock(&self.pins)?.get(&(*conversation, role)) {
            Some(pinned) if pinned != sender => Err(Error::WrongSender),
            _ => Ok(()),
        }
    }

    /// Bind the conversation to `sender`. Call this only after the first
    /// letter was handled successfully, so a forged first letter that the
    /// handler rejects cannot claim the conversation.
    pub fn pin(&self, conversation: &[u8; 16], role: u8, sender: &SenderId) -> Result<(), Error> {
        let mut pins = lock(&self.pins)?;
        match pins.get(&(*conversation, role)) {
            Some(pinned) if pinned == sender => Ok(()),
            Some(_) => Err(Error::WrongSender),
            None => {
                let mut record = Vec::with_capacity(PIN_LEN);
                record.extend_from_slice(conversation);
                record.push(role);
                record.extend_from_slice(&sender.0);
                append(&self.path, &record)?;
                pins.insert((*conversation, role), *sender);
                Ok(())
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, Error> {
    mutex.lock().map_err(|_| Error::Mailbox("guard lock poisoned".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::letter::SenderKey;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("tormail-guard-{name}-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn replayed_ids_are_refused_across_restarts() {
        let path = temp("seen");
        let guard = ReplayGuard::open(&path).unwrap();
        let id = MessageId::generate();
        guard.admit(&id).unwrap();
        assert!(matches!(guard.admit(&id), Err(Error::Replay)));
        guard.admit(&MessageId::generate()).unwrap();
        drop(guard);
        let reopened = ReplayGuard::open(&path).unwrap();
        assert!(matches!(reopened.admit(&id), Err(Error::Replay)));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_conversation_keeps_its_first_sender_across_restarts() {
        let path = temp("pins");
        let pins = SenderPins::open(&path).unwrap();
        let (first, other) = (SenderKey::generate().id(), SenderKey::generate().id());
        let conversation = [3u8; 16];
        pins.check(&conversation, 1, &other).unwrap();
        pins.pin(&conversation, 1, &first).unwrap();
        pins.pin(&conversation, 1, &first).unwrap();
        assert!(matches!(pins.check(&conversation, 1, &other), Err(Error::WrongSender)));
        assert!(matches!(pins.pin(&conversation, 1, &other), Err(Error::WrongSender)));
        pins.check(&conversation, 2, &other).unwrap();
        pins.check(&[4u8; 16], 1, &other).unwrap();
        drop(pins);
        let reopened = SenderPins::open(&path).unwrap();
        assert!(matches!(reopened.check(&conversation, 1, &other), Err(Error::WrongSender)));
        reopened.check(&conversation, 1, &first).unwrap();
        let _ = fs::remove_file(&path);
    }
}
