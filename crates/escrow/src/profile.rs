//! Signing and reading a moderator's public profile.
//!
//! The profile is signed by the key that signs the moderator's rulings and
//! carries its [`ModeratorRecord`]. [`read_profile`] accepts a profile only
//! when the signature verifies and the record's signing key is the key that
//! signed, so the record a party copies into escrow terms is the one the
//! moderator itself published.

use bitcoin::secp256k1::SecretKey;
use openbazaar_protocol::{Error as ProtocolError, ModeratorProfile, Signed, SignedModeratorProfile};

use crate::moderator::{ModeratorKeys, ModeratorRecord};
use crate::Error;

/// A profile whose signature and record have been checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedProfile {
    pub profile: ModeratorProfile,
    pub record: ModeratorRecord,
    /// The signed bytes, as they were read.
    pub bytes: Vec<u8>,
}

impl VerifiedProfile {
    /// The moderator's id: its ruling key, x-only.
    pub fn id(&self) -> [u8; 32] {
        self.profile.moderator_key
    }
}

impl ModeratorKeys {
    /// Sign `profile` with the ruling key. The profile must name this key
    /// and carry a record for these keys. The error is the protocol's, so a
    /// caller can say which text field was refused.
    pub fn sign_profile(&self, profile: ModeratorProfile) -> Result<VerifiedProfile, ProtocolError> {
        let record = ModeratorRecord::from_bytes(&profile.record)
            .map_err(|_| ProtocolError::BadTerms("the moderator record is not valid"))?;
        if record.signing_key != self.signing_public() || record.encryption_key != self.encryption_public() {
            return Err(ProtocolError::BadKey);
        }
        let secret = SecretKey::from_slice(&self.signing().serialize()).map_err(|_| ProtocolError::BadKey)?;
        let signed = Signed::sign(profile, &secret)?;
        let bytes = signed.encode()?;
        // Decoding runs the field checks a reader will run.
        SignedModeratorProfile::decode(&bytes)?;
        Ok(VerifiedProfile { profile: signed.terms, record, bytes })
    }
}

/// Read a signed profile: its signature, its record, and that the record's
/// ruling key is the key that signed.
pub fn read_profile(bytes: &[u8]) -> Result<VerifiedProfile, Error> {
    let signed = SignedModeratorProfile::decode(bytes).map_err(|_| Error::Malformed)?;
    signed.verify().map_err(|_| Error::BadSignature)?;
    let record = ModeratorRecord::from_bytes(&signed.terms.record)?;
    if record.id() != signed.terms.moderator_key {
        return Err(Error::ModeratorMismatch);
    }
    Ok(VerifiedProfile { profile: signed.terms, record, bytes: bytes.to_vec() })
}
