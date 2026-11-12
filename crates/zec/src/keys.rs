//! Zcash keys from the node's BIP 39 phrase.
//!
//! ```text
//! seed  = BIP 39 seed of the phrase, empty passphrase (64 bytes)
//! usk   = ZIP 32 unified spending key of `seed`, account 0
//!         (coin type 133 on mainnet, 1 on testnet)
//! ufvk  = its unified full viewing key
//! order = Orchard-protocol receiver of `ufvk` at diversifier index j, external scope
//! ```
//!
//! ZIP 316 has one receiver type for the Orchard protocol, and Ironwood is a
//! pool of that protocol: since NU6.3 a payment to the receiver lands in
//! Ironwood, the default privacy pool, and the same viewing key finds it.
//! Orchard, the older pool of the protocol, is outbound only.
//!
//! This is the derivation Zcash wallets use for their first account, so the
//! phrase restored in Zodl or any ZIP 32 wallet finds every payment. ZIP 32's
//! Orchard derivation hashes the seed with BLAKE2b under its own
//! personalization, while BIP 32 uses HMAC-SHA512 keyed with `Bitcoin seed`,
//! so these keys share nothing with the node's bitcoin, Monero, Lightning,
//! store, or escrow keys from the same phrase.
//!
//! Tiers: the spending key is never stored. The UFVK is the receive tier: it
//! finds and values payments, and it computes nullifiers, so the wallet also
//! sees spends made by another wallet restored from the same phrase.

use orchard::keys::{FullViewingKey, Scope};
use zcash_keys::address::UnifiedAddress;
use zcash_keys::keys::{UnifiedFullViewingKey, UnifiedSpendingKey};
use zeroize::Zeroizing;
use zip32::AccountId;

use crate::ZecNetwork;

/// The viewing key of one wallet: enough to find, value, and see the spend
/// of every payment to it, not to spend.
#[derive(Clone)]
pub struct ViewKey {
    network: ZecNetwork,
    ufvk: UnifiedFullViewingKey,
    orchard: FullViewingKey,
}

impl ViewKey {
    /// From the 64-byte BIP 39 seed of the wallet phrase (not the phrase itself).
    pub fn from_seed(seed: &[u8], network: ZecNetwork) -> Result<Self, String> {
        if seed.len() < 32 {
            return Err("a ZIP 32 seed has at least 32 bytes".into());
        }
        let usk = UnifiedSpendingKey::from_seed(&network.params(), seed, AccountId::ZERO)
            .map_err(|err| format!("Zcash key derivation failed: {err:?}"))?;
        Self::from_ufvk(usk.to_unified_full_viewing_key(), network)
    }

    fn from_ufvk(ufvk: UnifiedFullViewingKey, network: ZecNetwork) -> Result<Self, String> {
        let orchard = ufvk.orchard().cloned().ok_or("that viewing key has no Orchard key")?;
        Ok(Self { network, ufvk, orchard })
    }

    pub fn network(&self) -> ZecNetwork {
        self.network
    }

    /// The UFVK in its standard encoding (`uview1…` or `uviewtest1…`). A
    /// Zcash wallet imports it as a watch-only account.
    pub fn ufvk(&self) -> Zeroizing<String> {
        // A key derived from a seed, or decoded from such an encoding, has
        // shielded items and no metadata, so revision 0 encodes it.
        Zeroizing::new(self.ufvk.encode(&self.network.params()).expect("a seed-derived UFVK has an encoding"))
    }

    pub fn encode(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.ufvk().as_bytes().to_vec())
    }

    pub fn decode(bytes: &[u8], network: ZecNetwork) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "the Zcash viewing key could not be read".to_string())?;
        let ufvk = UnifiedFullViewingKey::decode(&network.params(), text.trim())
            .map_err(|_| format!("that is not a Zcash {network} viewing key"))?;
        Self::from_ufvk(ufvk, network)
    }

    /// The Orchard-protocol receiver at diversifier index `index`. A payment
    /// to it lands in Ironwood.
    pub fn receiver(&self, index: u32) -> orchard::Address {
        self.orchard.address_at(index, Scope::External)
    }

    /// The Unified Address with only the Orchard-protocol receiver at
    /// `index`, so a payment lands in Ironwood: no Sapling receiver, and no
    /// transparent one a buyer could pay in the clear.
    pub fn address(&self, index: u32) -> String {
        UnifiedAddress::from_receivers(Some(self.receiver(index)), None, None, None, None)
            .expect("an Orchard receiver is a valid Unified Address")
            .encode(&self.network.params())
    }

    /// The index `address` was issued at, if it is one of this key's
    /// external addresses. Change (the internal scope) has none.
    pub fn index_of(&self, address: &orchard::Address) -> Option<u32> {
        let index = self.orchard.to_ivk(Scope::External).diversifier_index(address)?;
        u32::try_from(index).ok()
    }

    pub(crate) fn orchard(&self) -> &FullViewingKey {
        &self.orchard
    }
}

/// Whether `address` is the kind this crate hands out for an order: a Unified
/// Address on `network` whose only receiver is Orchard-protocol.
pub fn is_order_address(address: &str, network: ZecNetwork) -> bool {
    let params = network.params();
    let receiver = UnifiedAddress::decode(&params, address).ok().and_then(|ua| ua.orchard().copied());
    let only = receiver.and_then(|receiver| UnifiedAddress::from_receivers(Some(receiver), None, None, None, None));
    only.is_some_and(|only| only.encode(&params) == address)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: [u8; 64] = [7u8; 64];

    #[test]
    fn the_phrase_restores_in_any_zip32_wallet() {
        // Same path as zcash_keys' own account derivation, which Zcash wallets use.
        for network in [ZecNetwork::Mainnet, ZecNetwork::Testnet] {
            let key = ViewKey::from_seed(&SEED, network).unwrap();
            let usk = UnifiedSpendingKey::from_seed(&network.params(), &SEED, AccountId::ZERO).unwrap();
            let expected = orchard::keys::FullViewingKey::from(usk.orchard());
            assert_eq!(key.orchard().to_bytes(), expected.to_bytes());
        }
        let main = ViewKey::from_seed(&SEED, ZecNetwork::Mainnet).unwrap();
        let test = ViewKey::from_seed(&SEED, ZecNetwork::Testnet).unwrap();
        assert_ne!(main.orchard().to_bytes(), test.orchard().to_bytes(), "coin types differ");
        assert!(ViewKey::from_seed(&[1u8; 31], ZecNetwork::Mainnet).is_err());
    }

    #[test]
    fn the_viewing_key_round_trips_and_is_network_bound() {
        let key = ViewKey::from_seed(&SEED, ZecNetwork::Testnet).unwrap();
        assert!(key.ufvk().starts_with("uviewtest1"));
        let again = ViewKey::decode(&key.encode(), ZecNetwork::Testnet).unwrap();
        assert_eq!(again.orchard().to_bytes(), key.orchard().to_bytes());
        assert!(ViewKey::decode(&key.encode(), ZecNetwork::Mainnet).is_err());
        assert!(ViewKey::decode(b"uview1nope", ZecNetwork::Mainnet).is_err());
        assert!(ViewKey::from_seed(&SEED, ZecNetwork::Mainnet).unwrap().ufvk().starts_with("uview1"));
    }

    #[test]
    fn addresses_are_orchard_only_and_map_back_to_their_index() {
        let key = ViewKey::from_seed(&SEED, ZecNetwork::Mainnet).unwrap();
        let params = ZecNetwork::Mainnet.params();
        for index in [0u32, 1, 2, 1_000, u32::MAX] {
            let text = key.address(index);
            assert!(text.starts_with("u1"), "{text}");
            let parsed = UnifiedAddress::decode(&params, &text).unwrap();
            assert!(parsed.has_orchard() && !parsed.has_sapling() && !parsed.has_transparent());
            assert_eq!(key.index_of(parsed.orchard().unwrap()), Some(index));
        }
        assert_ne!(key.address(1), key.address(2));
        let other = ViewKey::from_seed(&[8u8; 64], ZecNetwork::Mainnet).unwrap();
        assert_eq!(key.index_of(&other.receiver(1)), None);
        let change = key.orchard().address_at(1u32, Scope::Internal);
        assert_eq!(key.index_of(&change), None, "change is not an order address");
        assert!(ViewKey::from_seed(&SEED, ZecNetwork::Testnet).unwrap().address(1).starts_with("utest1"));
    }

    #[test]
    fn an_order_address_is_orchard_only_and_on_its_network() {
        let key = ViewKey::from_seed(&SEED, ZecNetwork::Testnet).unwrap();
        assert!(is_order_address(&key.address(4), ZecNetwork::Testnet));
        assert!(!is_order_address(&key.address(4), ZecNetwork::Mainnet), "another network");
        use zcash_address::unified::{Address, Encoding, Receiver, Revision, Uitem};
        let items = vec![Uitem::Data(Receiver::Orchard(key.receiver(4).to_raw_address_bytes())), Uitem::Data(Receiver::P2pkh([7; 20]))];
        let both = Address::try_from_items(Revision::R0, items).unwrap();
        let both = both.encode(&zcash_protocol::consensus::NetworkType::Test);
        assert!(UnifiedAddress::decode(&ZecNetwork::Testnet.params(), &both).is_ok() && !is_order_address(&both, ZecNetwork::Testnet), "a transparent receiver could be paid in the clear");
        assert!(!is_order_address("tmNope", ZecNetwork::Testnet));
    }
}
