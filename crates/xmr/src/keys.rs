//! Monero keys from the node's BIP 39 seed.
//!
//! The node derives one 32-byte secret per wallet at the hardened BIP 32
//! path `m/1984'/coin'/5'/0'` (see the node's `seed.rs`) and hands it here.
//!
//! ```text
//! b = HMAC-SHA512(key = "OpenBazaar Monero spend key v1", data = secret) mod l   (wide reduction)
//! a = Hs(b) = keccak256(b) mod l                                                  (Monero's rule)
//! B = bG, A = aG
//! ```
//!
//! `a` from `b` is how every Monero wallet derives the view key, so the
//! private spend key alone restores the wallet in monero-wallet-cli or
//! Feather. The wide reduction keeps `b` uniform. The BIP 32 secret is used
//! for nothing else, and the path is hardened, so the wallet's xpubs reveal
//! nothing about it.
//!
//! Tiers: `b` (spend) is never stored; the node derives it from the phrase
//! in the spend tier for one signature. `a` and `B` (view) are the receive
//! tier and stay usable while the wallet is locked.

use curve25519_dalek::constants::ED25519_BASEPOINT_TABLE;
use hmac::{Hmac, Mac};
use monero_wallet::address::{MoneroAddress, SubaddressIndex};
use monero_wallet::ed25519::{CompressedPoint, Point, Scalar};
use monero_wallet::ViewPair;
use serde_json::{json, Value};
use sha2::Sha512;
use zeroize::Zeroizing;

use crate::XmrNetwork;

pub const SPEND_KEY_LABEL: &[u8] = b"OpenBazaar Monero spend key v1";

/// The private spend key. Held only for one signature.
pub struct SpendKey(Zeroizing<Scalar>);

impl SpendKey {
    pub fn from_seed_secret(secret: &[u8; 32]) -> Self {
        let mut mac = Hmac::<Sha512>::new_from_slice(SPEND_KEY_LABEL).expect("HMAC takes a key of any length");
        mac.update(secret);
        let wide = Zeroizing::new(<[u8; 64]>::from(mac.finalize().into_bytes()));
        let scalar = curve25519_dalek::Scalar::from_bytes_mod_order_wide(&wide);
        Self(Zeroizing::new(Scalar::from(scalar)))
    }

    pub fn view_keys(&self) -> ViewKeys {
        let bytes = Zeroizing::new(<[u8; 32]>::from(*self.0));
        let b: Zeroizing<curve25519_dalek::Scalar> = Zeroizing::new((*self.0).into());
        let spend = Point::from(&*b * ED25519_BASEPOINT_TABLE);
        ViewKeys {
            view: Zeroizing::new(Scalar::hash(bytes.as_ref())),
            spend,
        }
    }

    pub(crate) fn scalar(&self) -> &Zeroizing<Scalar> {
        &self.0
    }
}

/// The private view key and the public spend key: enough to find and value
/// incoming outputs, not to spend them.
#[derive(Clone)]
pub struct ViewKeys {
    view: Zeroizing<Scalar>,
    spend: Point,
}

impl ViewKeys {
    pub fn view_pair(&self) -> ViewPair {
        ViewPair::new(self.spend, self.view.clone()).expect("a spend key made as bG is torsion-free")
    }

    pub fn primary_address(&self, network: XmrNetwork) -> MoneroAddress {
        self.view_pair().legacy_address(network.address_network())
    }

    /// Subaddress `index` of account 0. Index 0 is the primary address, so it is refused.
    pub fn subaddress(&self, network: XmrNetwork, index: u32) -> MoneroAddress {
        let index = SubaddressIndex::new(0, index).expect("subaddress index 0 is the primary address");
        self.view_pair().subaddress(network.address_network(), index)
    }

    /// The private view key as hex (receive tier only; tests check it never leaves the node).
    pub fn view_key_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(hex::encode(<[u8; 32]>::from(*self.view)))
    }

    pub fn encode(&self) -> Zeroizing<Vec<u8>> {
        let value = json!({
            "version": 1,
            "view": self.view_key_hex().as_str(),
            "spend": hex::encode(self.spend.compress().to_bytes()),
        });
        Zeroizing::new(serde_json::to_vec(&value).expect("view keys are plain json"))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let bad = || "the Monero view keys could not be read".to_string();
        let value: Value = serde_json::from_slice(bytes).map_err(|_| bad())?;
        let field = |key: &str| -> Result<Zeroizing<[u8; 32]>, String> {
            let mut out = Zeroizing::new([0u8; 32]);
            hex::decode_to_slice(value.get(key).and_then(Value::as_str).ok_or_else(bad)?, out.as_mut()).map_err(|_| bad())?;
            Ok(out)
        };
        let view = Scalar::read(&mut field("view")?.as_slice()).map_err(|_| bad())?;
        let spend = CompressedPoint::from(*field("spend")?).decompress().ok_or_else(bad)?;
        ViewPair::new(spend, Zeroizing::new(view)).map_err(|_| bad())?;
        Ok(Self { view: Zeroizing::new(view), spend })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_key_is_monero_hs_of_the_spend_key() {
        let spend = SpendKey::from_seed_secret(&[7u8; 32]);
        let keys = spend.view_keys();
        let expected = Scalar::hash(<[u8; 32]>::from(**spend.scalar()));
        assert_eq!(keys.view_key_hex().as_str(), hex::encode(<[u8; 32]>::from(expected)));
        let again = ViewKeys::decode(&keys.encode()).unwrap();
        assert_eq!(again.primary_address(XmrNetwork::Stagenet), keys.primary_address(XmrNetwork::Stagenet));
    }

    #[test]
    fn different_secrets_and_networks_give_different_addresses() {
        let one = SpendKey::from_seed_secret(&[1u8; 32]).view_keys();
        let two = SpendKey::from_seed_secret(&[2u8; 32]).view_keys();
        assert_ne!(one.primary_address(XmrNetwork::Mainnet), two.primary_address(XmrNetwork::Mainnet));
        let main = one.primary_address(XmrNetwork::Mainnet).to_string();
        let stage = one.primary_address(XmrNetwork::Stagenet).to_string();
        assert!(main.starts_with('4') && stage.starts_with('5'), "{main} {stage}");
        assert!(one.subaddress(XmrNetwork::Mainnet, 1).to_string().starts_with('8'));
        assert!(one.subaddress(XmrNetwork::Stagenet, 1).to_string().starts_with('7'));
    }
}
