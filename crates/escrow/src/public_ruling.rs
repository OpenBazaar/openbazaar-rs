//! The public record next to a ModeratorRuling v2.
//!
//! The ruling bytes are unchanged. This record cites their digest and names
//! the store signing key and the listing the store-signed offer covered.

use bitcoin::secp256k1::{Keypair, Secp256k1, SecretKey};
use openbazaar_protocol::{ruling_digest, DisputeOutcome, PublicRuling, Signed, FAVORED_BUYER, FAVORED_VENDOR};

use crate::crypto;
use crate::letter::ModeratorRuling;
use crate::moderator::ModeratorKeys;
use crate::{Error, Party};

/// Who was paid, and the store and listing the public record names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublishedSale {
    pub favored: Party,
    pub store_key: [u8; 32],
    pub listing_id: [u8; 16],
}

impl ModeratorKeys {
    /// Sign the public record with the same key that signs rulings.
    pub fn sign_public_ruling(
        &self,
        ruling_bytes: &[u8],
        store_key: [u8; 32],
        listing_id: [u8; 16],
    ) -> Result<Vec<u8>, Error> {
        let secret = SecretKey::from_slice(&self.signing().serialize()).map_err(|_| Error::WrongKey)?;
        let moderator_key = Keypair::from_secret_key(&Secp256k1::new(), &secret).x_only_public_key().0.serialize();
        let signed = Signed::sign(
            PublicRuling {
                ruling_digest: ruling_digest(ruling_bytes),
                store_key,
                listing_id,
                moderator_key,
            },
            &secret,
        )
        .map_err(|_| Error::BadSignature)?;
        signed.encode().map_err(|_| Error::BadSignature)
    }

    /// Sign the outcome record: which party the ruling paid, and nothing else.
    pub fn sign_dispute_outcome(&self, ruling_bytes: &[u8]) -> Result<Vec<u8>, Error> {
        let ruling = ModeratorRuling::from_bytes(ruling_bytes)?;
        let favored = match ruling.favored() {
            Party::Buyer => FAVORED_BUYER,
            Party::Vendor => FAVORED_VENDOR,
        };
        let secret = SecretKey::from_slice(&self.signing().serialize()).map_err(|_| Error::WrongKey)?;
        let moderator_key = Keypair::from_secret_key(&Secp256k1::new(), &secret).x_only_public_key().0.serialize();
        let signed = Signed::sign(
            DisputeOutcome { ruling_digest: ruling_digest(ruling_bytes), favored, moderator_key },
            &secret,
        )
        .map_err(|_| Error::BadSignature)?;
        signed.encode().map_err(|_| Error::BadSignature)
    }
}

/// Anyone with the outcome record learns which party a ruling paid, and the
/// digest that ties it to that ruling; no party, store, or listing.
///
/// Checks the moderator's signature and that the moderator's signing key is
/// the one in the record.
pub fn read_dispute_outcome(record: &[u8], moderator_signing_key: &[u8; 33]) -> Result<(Party, [u8; 32]), Error> {
    let signed = Signed::<DisputeOutcome>::decode(record).map_err(|_| Error::BadPayload)?;
    signed.verify().map_err(|_| Error::BadSignature)?;
    let xonly: [u8; 32] = moderator_signing_key[1..].try_into().map_err(|_| Error::WrongKey)?;
    if signed.terms.moderator_key != xonly {
        return Err(Error::BadPayload);
    }
    let _ = crypto::point_from_compressed(moderator_signing_key)?;
    let favored = match signed.terms.favored {
        FAVORED_BUYER => Party::Buyer,
        FAVORED_VENDOR => Party::Vendor,
        _ => return Err(Error::BadPayload),
    };
    Ok((favored, signed.terms.ruling_digest))
}

/// A buyer with the ruling bytes and the public record.
///
/// Checks the moderator's signature on both, and that the record cites this
/// ruling. The favored party is who was paid.
pub fn read_published_ruling(
    ruling_bytes: &[u8],
    record: &[u8],
    moderator_signing_key: &[u8; 33],
) -> Result<PublishedSale, Error> {
    let ruling = ModeratorRuling::from_bytes(ruling_bytes)?;
    ruling.verify(moderator_signing_key)?;
    let signed = Signed::<PublicRuling>::decode(record).map_err(|_| Error::BadPayload)?;
    signed.verify().map_err(|_| Error::BadSignature)?;
    let xonly: [u8; 32] = moderator_signing_key[1..].try_into().map_err(|_| Error::WrongKey)?;
    if signed.terms.moderator_key != xonly || signed.terms.ruling_digest != ruling_digest(ruling_bytes) {
        return Err(Error::BadPayload);
    }
    let _ = crypto::point_from_compressed(moderator_signing_key)?;
    Ok(PublishedSale {
        favored: ruling.favored(),
        store_key: signed.terms.store_key,
        listing_id: signed.terms.listing_id,
    })
}
