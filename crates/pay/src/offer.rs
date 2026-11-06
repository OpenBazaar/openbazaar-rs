//! BOLT 12 offers.
//!
//! Parsing and building go through `lightning::offers`. The offer string is a
//! real bech32 `lno1…` encoding of the offer TLV. This module does not build
//! blinded paths, answer invoice requests, or run an LDK node. A vendor who is
//! only reachable on private channels needs the node crate to attach a blinded
//! path before the offer is published. An offer created here carries the
//! issuer signing pubkey, a bitcoin amount, and a description, which is enough
//! for a buyer whose wallet can reach that pubkey.

use std::fmt;
use std::str::FromStr;

use bitcoin::secp256k1::PublicKey;
use bitcoin::{Amount, Network};
use lightning::offers::offer::{Amount as OfferAmount, Offer, OfferBuilder};

use crate::error::{offer, PayError};

/// A bitcoin-denominated BOLT 12 offer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bolt12Offer {
    encoded: String,
    price_sats: u64,
}

impl Bolt12Offer {
    /// Build a deterministic offer for `price` on `network`.
    ///
    /// The same pubkey, price, description, and network always produce the
    /// same string. No blinded path is attached.
    pub fn create(
        signing_pubkey: PublicKey,
        price: Amount,
        description: &str,
        network: Network,
    ) -> Result<Self, PayError> {
        let amount_msats = sats_to_msats(price.to_sat())?;
        let built = OfferBuilder::new(signing_pubkey)
            .chain(network)
            .description(description.to_owned())
            .issuer("OpenBazaar".to_owned())
            .amount_msats(amount_msats)
            .build()
            .map_err(offer)?;
        Self::from_parsed(built)
    }

    /// Parse an `lno1…` offer. Malformed strings and non-bitcoin amounts fail.
    pub fn parse(encoded: &str) -> Result<Self, PayError> {
        let parsed: Offer = encoded.parse().map_err(offer)?;
        Self::from_parsed(parsed)
    }

    /// Price the offer asks for, in satoshis.
    pub fn price(&self) -> Amount {
        Amount::from_sat(self.price_sats)
    }

    /// Issuer signing pubkey, when the offer publishes one.
    pub fn signing_pubkey(&self) -> Option<PublicKey> {
        self.parsed().issuer_signing_pubkey()
    }

    /// Offer description.
    pub fn description(&self) -> Option<String> {
        self.parsed().description().map(|text| text.to_string())
    }

    fn parsed(&self) -> Offer {
        self.encoded
            .parse()
            .expect("a stored offer was parsed before it was stored")
    }

    fn from_parsed(parsed: Offer) -> Result<Self, PayError> {
        let price_sats = bitcoin_sats(&parsed)?;
        Ok(Self {
            encoded: parsed.to_string(),
            price_sats,
        })
    }
}

impl fmt::Display for Bolt12Offer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encoded)
    }
}

impl FromStr for Bolt12Offer {
    type Err = PayError;

    fn from_str(encoded: &str) -> Result<Self, Self::Err> {
        Self::parse(encoded)
    }
}

fn bitcoin_sats(parsed: &Offer) -> Result<u64, PayError> {
    match parsed.amount() {
        Some(OfferAmount::Bitcoin { amount_msats }) => msats_to_sats(amount_msats),
        Some(OfferAmount::Currency { .. }) => Err(PayError::Offer(
            "offer amount is not denominated in bitcoin".into(),
        )),
        None => Err(PayError::Offer("offer has no bitcoin amount".into())),
    }
}

fn sats_to_msats(sats: u64) -> Result<u64, PayError> {
    if sats == 0 {
        return Err(PayError::Offer(
            "offer price must be at least one satoshi".into(),
        ));
    }
    sats.checked_mul(1_000)
        .ok_or_else(|| PayError::Offer("offer price overflows millisatoshis".into()))
}

fn msats_to_sats(msats: u64) -> Result<u64, PayError> {
    if msats == 0 || msats % 1_000 != 0 {
        return Err(PayError::Offer(
            "offer amount must be a whole number of satoshis".into(),
        ));
    }
    Ok(msats / 1_000)
}

#[cfg(test)]
mod tests {
    use bitcoin::secp256k1::{Secp256k1, SecretKey};

    use super::*;

    #[test]
    fn builds_and_parses_a_deterministic_offer() {
        let first = sample_offer();
        let second = sample_offer();
        assert_eq!(first.to_string(), second.to_string());
        assert!(first.to_string().starts_with("lno1"));

        let parsed = Bolt12Offer::parse(&first.to_string()).unwrap();
        assert_eq!(parsed, first);
        assert_eq!(parsed.price(), Amount::from_sat(21_000));
        assert_eq!(parsed.signing_pubkey(), Some(signing_pubkey()));
        assert_eq!(parsed.description().as_deref(), Some("Cedar chest"));
    }

    #[test]
    fn malformed_offers_fail() {
        assert!(Bolt12Offer::parse("").is_err());
        assert!(Bolt12Offer::parse("not-an-offer").is_err());
        assert!(Bolt12Offer::parse("lno1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq").is_err());

        let mut chars: Vec<char> = sample_offer().to_string().chars().collect();
        let index = chars.len() - 2;
        chars[index] = if chars[index] == 'q' { 'p' } else { 'q' };
        let tampered: String = chars.into_iter().collect();
        assert!(Bolt12Offer::parse(&tampered).is_err());
    }

    fn sample_offer() -> Bolt12Offer {
        Bolt12Offer::create(
            signing_pubkey(),
            Amount::from_sat(21_000),
            "Cedar chest",
            Network::Bitcoin,
        )
        .unwrap()
    }

    fn signing_pubkey() -> PublicKey {
        let secp = Secp256k1::new();
        let secret = SecretKey::from_slice(&[42u8; 32]).unwrap();
        PublicKey::from_secret_key(&secp, &secret)
    }
}
