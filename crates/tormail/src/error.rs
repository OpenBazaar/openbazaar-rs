use thiserror::Error;
use tor_hscrypto::pk::HsIdParseError;

/// Failures from sealing, the mailbox, or the Tor transport.
#[derive(Debug, Error)]
pub enum Error {
    /// The string is not a v3 onion address.
    #[error("invalid v3 onion address")]
    BadOnion(#[source] HsIdParseError),

    /// The order plaintext breaks a field rule.
    #[error("invalid order: {0}")]
    InvalidOrder(&'static str),

    /// The sealed blob is the wrong length, version, or layout.
    #[error("sealed letter is truncated or malformed")]
    Malformed,

    /// HPKE rejected the letter. A wrong key and a damaged tag both land here.
    #[error("letter could not be opened")]
    Seal,

    /// `take` was asked for an id the mailbox does not have.
    #[error("no letter with that id")]
    Missing,

    /// The mailbox directory could not finish a bookkeeping step.
    #[error("mailbox: {0}")]
    Mailbox(String),

    /// The in-memory transport lock was poisoned.
    #[error("transport: {0}")]
    Transport(&'static str),

    /// Arti returned an error while building a client, dialing, or serving.
    #[error("arti: {0}")]
    Arti(String),

    /// The onion service's request stream ended.
    #[error("onion listener stopped")]
    ListenerClosed,

    /// The listener rejected a stream or could not be configured.
    #[error("onion listener: {0}")]
    Listener(&'static str),

    /// Filesystem failure while reading or writing the mailbox directory.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
