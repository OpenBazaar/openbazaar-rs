use core::fmt;

use crate::Phase;

/// Failure of an escrow transition, a proof, or a signature check.
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
    /// The dispute letter or ruling is inconsistent with the escrow it names.
    BadPayload,
    /// Bytes are truncated, oversized, or carry the wrong magic or version.
    Malformed,
    /// A proof of knowledge, DLEQ, or verifiable encryption does not verify.
    BadProof,
    /// A presigned transaction pays a zero fee or is below the fee-rate floor.
    FeeTooLow,
    /// The terms name a different moderator than the caller expects.
    ModeratorMismatch,
    /// The terms are not signed by both per-order keys.
    BadTermsSignature,
    /// The funding PSBT or transaction does not match the escrow.
    BadFunding(&'static str),
    /// The letter arrived after `dispute_deadline`.
    DisputeWindowClosed,
    /// A caller-supplied parameter cannot construct an escrow.
    InvalidParams(&'static str),
    /// The MuSig aggregate and the taproot output key diverged.
    KeyMismatch,
    /// The network fee exceeds the policy maximum.
    FeeTooHigh,
    /// The timeline is too close to, or too far from, the current tip.
    HeightsOutOfRange,
    /// An output script is not a standard payout type or is below its dust limit.
    NonStandardOutput,
    /// The moderator already ruled for the other branch of this escrow.
    AlreadyRuled,
    /// The funding output named by the letter is not on the moderator's chain.
    FundingNotOnChain,
    /// The ruling store failed.
    RulingStore(&'static str),
    /// The caller's gate refused the letter before its proofs were checked.
    RateLimited,
    /// The moderator's chain view cannot answer yet (for example, compact
    /// block filters are still scanning for the funding script). Try again.
    ChainNotReady,
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
            Error::Malformed => write!(f, "encoded escrow data is malformed"),
            Error::BadProof => write!(f, "escrow proof failed verification"),
            Error::FeeTooLow => write!(f, "presigned transaction fee is below the floor"),
            Error::ModeratorMismatch => write!(f, "terms name a different moderator"),
            Error::BadTermsSignature => write!(f, "escrow terms are not signed by both parties"),
            Error::BadFunding(msg) => write!(f, "invalid funding: {msg}"),
            Error::DisputeWindowClosed => write!(f, "the dispute deadline has passed"),
            Error::InvalidParams(msg) => write!(f, "invalid escrow parameters: {msg}"),
            Error::KeyMismatch => {
                write!(f, "taproot output key does not match the MuSig aggregate")
            }
            Error::FeeTooHigh => write!(f, "network fee exceeds the policy maximum"),
            Error::HeightsOutOfRange => {
                write!(f, "escrow heights are out of range for the current tip")
            }
            Error::NonStandardOutput => {
                write!(f, "output script is non-standard or below its dust limit")
            }
            Error::AlreadyRuled => write!(f, "the moderator already ruled for the other branch"),
            Error::FundingNotOnChain => write!(f, "funding output is not on the moderator's chain"),
            Error::RulingStore(msg) => write!(f, "ruling store failed: {msg}"),
            Error::RateLimited => write!(f, "too many dispute letters; try again later"),
            Error::ChainNotReady => write!(f, "the moderator's chain view is not ready; try again"),
        }
    }
}

impl std::error::Error for Error {}
