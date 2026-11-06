//! One backup phrase per wallet, with a separate derivation path for each key role.
//!
//! The phrase is BIP 39 (12 words), generated on first boot, and sealed with
//! the data directory's vault key in a 0600 file that is written once. Roles:
//!
//! | role          | path                         |
//! |---------------|------------------------------|
//! | spend wallet  | `m/86'/coin'/0'/{0,1}/*` (BIP 86 taproot) |
//! | store signing | `m/1984'/coin'/0'/0'`        |
//! | escrow keys   | `m/1984'/coin'/1'/index'`    |
//! | BIP 352 scan  | `m/352'/coin'/0'/1'/0`       |
//! | BIP 352 spend | `m/352'/coin'/0'/0'/0`       |
//!
//! `coin` is 0 on mainnet and 1 on testnet4, so the same phrase never yields
//! the same key on both networks.

use std::path::Path;
use std::str::FromStr;

use bdk_wallet::keys::bip39::Mnemonic;
use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use crate::network::NetworkMode;
use crate::vault::Vault;
use crate::Error;

pub(crate) struct Seed {
    master: Xpriv,
    network: NetworkMode,
}

impl Seed {
    /// Load the phrase in `path`, or create one if the file does not exist.
    pub(crate) fn load_or_create(path: &Path, vault: &Vault, network: NetworkMode) -> Result<Self, Error> {
        let phrase = match vault.read_sealed(path)? {
            Some(bytes) => String::from_utf8(bytes)
                .map_err(|_| Error::Detail("the backup phrase could not be read".to_string()))?,
            None => {
                let mut entropy = [0u8; 16];
                StdRng::from_os_rng().fill_bytes(&mut entropy);
                let phrase = Mnemonic::from_entropy(&entropy)
                    .map_err(|err| Error::Detail(err.to_string()))?
                    .to_string();
                vault.write_sealed(path, phrase.as_bytes())?;
                phrase
            }
        };
        Self::from_phrase(&phrase, network)
    }

    pub(crate) fn from_phrase(phrase: &str, network: NetworkMode) -> Result<Self, Error> {
        let mnemonic = Mnemonic::parse(phrase)
            .map_err(|_| Error::Detail("the backup phrase is not valid BIP 39".to_string()))?;
        let master = Xpriv::new_master(network.bitcoin(), &mnemonic.to_seed(""))
            .map_err(|err| Error::Detail(err.to_string()))?;
        Ok(Self { master, network })
    }

    /// External and change descriptors for the spend wallet, with private keys.
    pub(crate) fn wallet_descriptors(&self) -> (String, String) {
        let coin = self.network.coin_type();
        let xprv = self.master;
        (
            format!("tr({xprv}/86'/{coin}'/0'/0/*)"),
            format!("tr({xprv}/86'/{coin}'/0'/1/*)"),
        )
    }

    pub(crate) fn store_signing_key(&self) -> SecretKey {
        self.derive(&format!("m/1984'/{}'/0'/0'", self.network.coin_type()))
    }

    pub(crate) fn escrow_key(&self, index: u32) -> SecretKey {
        self.derive(&format!("m/1984'/{}'/1'/{index}'", self.network.coin_type()))
    }

    pub(crate) fn scan_key(&self) -> SecretKey {
        self.derive(&format!("m/352'/{}'/0'/1'/0", self.network.coin_type()))
    }

    pub(crate) fn silent_spend_key(&self) -> SecretKey {
        self.derive(&format!("m/352'/{}'/0'/0'/0", self.network.coin_type()))
    }

    fn derive(&self, path: &str) -> SecretKey {
        let path = DerivationPath::from_str(path).expect("role paths are well formed");
        self.master
            .derive_priv(&Secp256k1::new(), &path)
            .expect("hardened derivation from a valid master key")
            .private_key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn roles_use_separate_keys_and_networks_do_not_share_them() {
        let test = Seed::from_phrase(PHRASE, NetworkMode::Testnet4).unwrap();
        let main = Seed::from_phrase(PHRASE, NetworkMode::Mainnet).unwrap();
        let roles = |seed: &Seed| {
            vec![
                seed.store_signing_key(),
                seed.escrow_key(0),
                seed.escrow_key(1),
                seed.scan_key(),
                seed.silent_spend_key(),
            ]
        };
        let mut all = roles(&test);
        all.extend(roles(&main));
        for (i, left) in all.iter().enumerate() {
            for right in &all[i + 1..] {
                assert_ne!(left, right);
            }
        }
        let (external, change) = test.wallet_descriptors();
        assert!(external.starts_with("tr(tprv") && external.contains("/86'/1'/0'/0/*"));
        assert!(change.contains("/86'/1'/0'/1/*"));
        assert!(main.wallet_descriptors().0.starts_with("tr(xprv"));
    }
}
