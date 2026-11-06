//! One price, two ways to pay it.
//!
//! Lightning is the BOLT 12 offer. On-chain is the BIP 352 code. Nothing in
//! between quotes a rate or swaps the payment.

use std::fmt;

use bitcoin::Amount;

use crate::error::PayError;
use crate::offer::Bolt12Offer;
use crate::silent::SilentPaymentCode;

/// The two direct-pay rails for a single satoshi price.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectPay {
    price: Amount,
    offer: Bolt12Offer,
    on_chain: SilentPaymentCode,
}

impl DirectPay {
    /// Pair an offer and a silent-payment code that both collect `price`.
    ///
    /// The offer's bitcoin amount has to be that same price. The silent-payment
    /// code carries no amount of its own; the price is shown beside it.
    pub fn new(
        price: Amount,
        offer: Bolt12Offer,
        on_chain: SilentPaymentCode,
    ) -> Result<Self, PayError> {
        let offer_sats = offer.price().to_sat();
        let expected_sats = price.to_sat();
        if offer_sats != expected_sats {
            return Err(PayError::Price {
                expected_sats,
                offer_sats,
            });
        }
        Ok(Self {
            price,
            offer,
            on_chain,
        })
    }

    pub fn price(&self) -> Amount {
        self.price
    }

    pub fn offer(&self) -> &Bolt12Offer {
        &self.offer
    }

    pub fn silent_payment_code(&self) -> &SilentPaymentCode {
        &self.on_chain
    }
}

impl fmt::Display for DirectPay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} sats\nlightning: {}\non-chain: {}",
            self.price.to_sat(),
            self.offer,
            self.on_chain
        )
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use bitcoin::Network;

    use super::*;
    use crate::offer::Bolt12Offer;
    use crate::silent::SilentPaymentCode;

    #[test]
    fn shows_both_rails_for_one_price() {
        let price = Amount::from_sat(21_000);
        let pay = DirectPay::new(price, sample_offer(price), sample_code()).unwrap();
        let text = pay.to_string();
        assert!(text.starts_with("21000 sats\n"));
        assert!(text.contains(&format!("lightning: {}", pay.offer())));
        assert!(text.contains(&format!("on-chain: {}", pay.silent_payment_code())));
        assert_eq!(pay.price(), price);
    }

    #[test]
    fn rejects_an_offer_priced_differently() {
        let offer = sample_offer(Amount::from_sat(21_000));
        let error = DirectPay::new(Amount::from_sat(22_000), offer, sample_code()).unwrap_err();
        assert!(matches!(
            error,
            PayError::Price {
                expected_sats: 22_000,
                offer_sats: 21_000
            }
        ));
    }

    fn sample_offer(price: Amount) -> Bolt12Offer {
        let secp = Secp256k1::new();
        let secret = SecretKey::from_slice(&[42u8; 32]).unwrap();
        Bolt12Offer::create(
            bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &secret),
            price,
            "Cedar chest",
            Network::Bitcoin,
        )
        .unwrap()
    }

    fn sample_code() -> SilentPaymentCode {
        let scan = SecretKey::from_slice(&[11u8; 32]).unwrap();
        let spend = SecretKey::from_slice(&[13u8; 32]).unwrap();
        SilentPaymentCode::from_keys(&scan, &spend, Network::Bitcoin).unwrap()
    }
}
