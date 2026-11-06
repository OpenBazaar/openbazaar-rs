//! Public fixture keys for the sample shop.
//!
//! These secrets are not a wallet. They exist so the catalog signatures are
//! real. Derivation from a backup phrase is not implemented; see [`KeyRole`](crate::KeyRole).

use crate::dispute::{DisputeEnvelope, DisputePlaintext};
use crate::escrow::EscrowTerms;
use crate::listing::{Listing, ListingKind};
use crate::order::{OrderEnvelope, OrderPlaintext};
use crate::payment::DirectPay;
use crate::sign;
use crate::store::Store;
use crate::Error;

const STORE_KEY: &str = "openbazaar/phase0/fixture/store";
const VENDOR_SPEND_KEY: &str = "openbazaar/phase0/fixture/vendor-spend";
const BUYER_SPEND_KEY: &str = "openbazaar/phase0/fixture/buyer-spend";
const ADAPTOR_POINT: &str = "openbazaar/phase0/fixture/adaptor-point";

struct Draft {
    title: &'static str,
    category: &'static str,
    price_sats: u64,
    ship_from: &'static str,
    kind: ListingKind,
}

const LISTINGS: &[Draft] = &[
    Draft {
        title: "Paperback novel",
        category: "Books",
        price_sats: 18_000,
        ship_from: "Oregon",
        kind: ListingKind::Physical,
    },
    Draft {
        title: "Whole bean coffee, 12 oz",
        category: "Grocery",
        price_sats: 9_500,
        ship_from: "Oregon",
        kind: ListingKind::Physical,
    },
    Draft {
        title: "Linen dish towel",
        category: "Home",
        price_sats: 7_000,
        ship_from: "Oregon",
        kind: ListingKind::Physical,
    },
    Draft {
        title: "Field recordings",
        category: "Music",
        price_sats: 4_000,
        ship_from: "Digital",
        kind: ListingKind::Digital,
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalog {
    pub store: Store,
    pub listings: Vec<Listing>,
}

impl Catalog {
    pub fn verify(&self) -> Result<(), Error> {
        self.store.verify()?;
        for listing in &self.listings {
            listing.verify()?;
            if listing.store_id != self.store.id {
                return Err(Error::StoreMismatch);
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = self.store.encode()?;
        for listing in &self.listings {
            bytes.extend(listing.encode()?);
        }
        Ok(bytes)
    }
}

pub fn fixture_catalog() -> Catalog {
    let store_secret = sign::secret_from_label(STORE_KEY);
    let store = Store::sign(
        "Marlow Goods",
        "Oregon",
        DirectPay {
            bolt12_offer: "bolt12-offer:fixture:marlow-goods".to_string(),
            silent_payment_code: "bip352-code:fixture:marlow-goods".to_string(),
        },
        &store_secret,
    )
    .expect("fixture store signs");
    let listings = LISTINGS
        .iter()
        .map(|draft| {
            Listing::sign(
                store.id,
                draft.title,
                draft.category,
                draft.price_sats,
                draft.ship_from,
                draft.kind,
                &store_secret,
            )
            .expect("fixture listing is signed by the fixture store")
        })
        .collect();
    Catalog { store, listings }
}

pub fn fixture_order(catalog: &Catalog) -> OrderEnvelope {
    let buyer = sign::secret_from_label(BUYER_SPEND_KEY);
    let book = catalog
        .listings
        .iter()
        .find(|listing| listing.title == "Paperback novel")
        .expect("fixture shop has a book");
    OrderEnvelope::sign(
        OrderPlaintext {
            buyer: sign::xonly(&buyer),
            listing_hash: book.content_hash().expect("book body encodes"),
            quantity: 1,
            note: "Please pack it flat.".to_string(),
            ship_to: "12 Cedar Street".to_string(),
        },
        &buyer,
    )
    .expect("fixture order signs")
}

pub fn fixture_dispute(catalog: &Catalog) -> DisputeEnvelope {
    let buyer_secret = sign::secret_from_label(BUYER_SPEND_KEY);
    let vendor_spend = sign::xonly(&sign::secret_from_label(VENDOR_SPEND_KEY));
    let buyer = sign::xonly(&buyer_secret);
    let OrderEnvelope::Plaintext { order, .. } = fixture_order(catalog) else {
        panic!("fixture order is plaintext");
    };
    DisputeEnvelope::sign(
        DisputePlaintext {
            opener: buyer,
            order,
            claim: "The cover arrived torn.".to_string(),
            escrow: EscrowTerms {
                buyer,
                vendor: vendor_spend,
                refund_locktime: 1008,
                presigned_refund: Vec::new(),
                adaptor_point: sign::xonly(&sign::secret_from_label(ADAPTOR_POINT)),
            },
        },
        &buyer_secret,
    )
    .expect("fixture dispute signs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_catalog_verifies_and_keeps_the_order_private() {
        let catalog = fixture_catalog();
        catalog.verify().unwrap();
        assert_eq!(catalog.listings.len(), 4);

        let order = fixture_order(&catalog);
        let order_bytes = order.encode().unwrap();
        assert_eq!(OrderEnvelope::decode(&order_bytes).unwrap(), order);
        order.verify().unwrap();

        let dispute = fixture_dispute(&catalog);
        let dispute_bytes = dispute.encode().unwrap();
        let decoded = DisputeEnvelope::decode(&dispute_bytes).unwrap();
        assert_eq!(decoded, dispute);
        dispute.verify().unwrap();

        let catalog_bytes = catalog.encode().unwrap();
        let ship_to = b"12 Cedar Street";
        assert!(
            !catalog_bytes
                .windows(ship_to.len())
                .any(|window| window == ship_to),
            "the public catalog must not carry the order"
        );

        let DisputeEnvelope::Plaintext {
            mut dispute,
            signature,
        } = decoded
        else {
            panic!("plaintext dispute");
        };
        assert!(dispute.escrow.presigned_refund.is_empty());
        assert_ne!(dispute.escrow.buyer, *catalog.store.id.as_bytes());
        assert_ne!(dispute.escrow.vendor, *catalog.store.id.as_bytes());
        assert_ne!(dispute.escrow.buyer, dispute.escrow.vendor);
        dispute.claim.push('!');
        let tampered = DisputeEnvelope::Plaintext { dispute, signature };
        assert_eq!(tampered.verify(), Err(Error::BadSignature));
    }
}
