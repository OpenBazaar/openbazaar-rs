use core::fmt;

use crate::Phase;

/// Failure of an escrow transition or a signature check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The phase graph has no edge between these states.
    InvalidTransition { from: Phase, to: Phase },
    /// Bitcoin Core `IsFinalTx` rejects this absolute locktime at `height`.
    LockTimeNotSatisfied { lock_time: u32, height: u32 },
    /// A partial signature does not belong to the claimed signer.
    BadPartial,
    /// An aggregated or adapted signature does not verify.
    BadSignature,
    /// The secret does not match the party required for this step.
    WrongKey,
    /// The dispute letter is truncated, oversized, or inconsistent.
    BadPayload,
    /// A caller-supplied parameter cannot construct an escrow.
    InvalidParams(&'static str),
    /// The MuSig aggregate and the taproot output key diverged.
    KeyMismatch,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidTransition { from, to } => {
                write!(f, "invalid escrow transition from {from:?} to {to:?}")
            }
            Error::LockTimeNotSatisfied { lock_time, height } => {
                write!(
                    f,
                    "nLockTime {lock_time} is not final at block height {height}"
                )
            }
            Error::BadPartial => write!(f, "partial signature failed verification"),
            Error::BadSignature => write!(f, "signature failed verification"),
            Error::WrongKey => write!(f, "secret key does not match the required party"),
            Error::BadPayload => write!(f, "dispute payload is invalid"),
            Error::InvalidParams(msg) => write!(f, "invalid escrow parameters: {msg}"),
            Error::KeyMismatch => {
                write!(f, "taproot output key does not match the MuSig aggregate")
            }
        }
    }
}

impl std::error::Error for Error {}
