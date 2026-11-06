use core::fmt;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Truncated,
    TrailingBytes,
    BadString,
    BadMagic,
    BadVersion,
    BadSignature,
    BadKey,
    UnknownKind,
    UnknownEnvelope,
    StoreMismatch,
    FieldTooLong,
    /// A sealed envelope has no plaintext to check in this phase.
    Sealed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Error::Truncated => "truncated record",
            Error::TrailingBytes => "trailing bytes after a canonical record",
            Error::BadString => "string is not utf-8",
            Error::BadMagic => "unexpected record magic",
            Error::BadVersion => "unsupported record version",
            Error::BadSignature => "signature did not verify",
            Error::BadKey => "key bytes are not valid",
            Error::UnknownKind => "unknown listing kind",
            Error::UnknownEnvelope => "unknown envelope tag",
            Error::StoreMismatch => "listing is not signed by this store",
            Error::FieldTooLong => "field exceeds the canonical length limit",
            Error::Sealed => "sealed envelope is not opened in this phase",
        };
        f.write_str(message)
    }
}

impl std::error::Error for Error {}
