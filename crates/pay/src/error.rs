use std::fmt;

/// Failure from a payment-rail constructor or parser.
#[derive(Debug)]
pub enum PayError {
    /// BIP 352 code, scan key, or spend key was rejected.
    Silent(String),
    /// BOLT 12 offer was rejected.
    Offer(String),
    /// Payjoin session or vendor input contribution was rejected.
    Payjoin(String),
    /// The offer's bitcoin amount and the direct-pay price differ.
    Price { expected_sats: u64, offer_sats: u64 },
    /// Directory URL could not be parsed.
    Directory(String),
}

impl fmt::Display for PayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PayError::Silent(message)
            | PayError::Offer(message)
            | PayError::Payjoin(message)
            | PayError::Directory(message) => f.write_str(message),
            PayError::Price {
                expected_sats,
                offer_sats,
            } => write!(
                f,
                "direct-pay price is {expected_sats} sats and the offer asks for {offer_sats} sats"
            ),
        }
    }
}

impl std::error::Error for PayError {}

pub(crate) fn silent(err: impl fmt::Display) -> PayError {
    PayError::Silent(err.to_string())
}

pub(crate) fn offer(err: impl fmt::Debug) -> PayError {
    PayError::Offer(format!("{err:?}"))
}

pub(crate) fn payjoin(err: impl fmt::Display) -> PayError {
    PayError::Payjoin(err.to_string())
}
