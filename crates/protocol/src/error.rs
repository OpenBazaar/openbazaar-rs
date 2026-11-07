use core::fmt;

use crate::public_text::PrivateText;

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
    UnknownRating,
    UnknownEnvelope,
    StoreMismatch,
    /// A label set entry is out of order, repeated, or not canonical.
    BadLabel,
    FieldTooLong,
    /// A sealed envelope has no plaintext to check in this phase.
    Sealed,
    /// A public free-text field looks like contact or location data.
    PrivateText {
        field: &'static str,
        kind: PrivateText,
    },
    /// A store profile field is out of range or not canonical.
    BadProfile(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Error::PrivateText { field, kind } = self {
            return write!(f, "the {field} looks like it contains {}; public records cannot carry it", kind.as_str());
        }
        if let Error::BadProfile(what) = self {
            return write!(f, "store profile: {what}");
        }
        let message = match self {
            Error::Truncated => "truncated record",
            Error::TrailingBytes => "trailing bytes after a canonical record",
            Error::BadString => "string is not utf-8",
            Error::BadMagic => "unexpected record magic",
            Error::BadVersion => "unsupported record version",
            Error::BadSignature => "signature did not verify",
            Error::BadKey => "key bytes are not valid",
            Error::UnknownKind => "unknown listing kind",
            Error::UnknownRating => "unknown content rating",
            Error::UnknownEnvelope => "unknown envelope tag",
            Error::StoreMismatch => "listing is not signed by this store",
            Error::BadLabel => "label set entry is not canonical",
            Error::FieldTooLong => "field exceeds the canonical length limit",
            Error::Sealed => "sealed envelope is not opened in this phase",
            Error::PrivateText { .. } | Error::BadProfile(_) => unreachable!("handled above"),
        };
        f.write_str(message)
    }
}

impl std::error::Error for Error {}
