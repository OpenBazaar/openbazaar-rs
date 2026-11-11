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
//! | escrow mail   | `m/1984'/coin'/2'/0'`        |
//! | store mailbox | `m/1984'/coin'/3'/0'` (X25519, in the signed store record) |
//! | letter root   | `m/1984'/coin'/4'/0'` (hashed with a conversation id into one signing key per order, escrow, or dispute) |
//! | Monero        | `m/1984'/coin'/5'/0'` (secret for `openbazaar_xmr::SpendKey`) |
//! | Lightning     | `m/1984'/coin'/6'/0'` (secret for `openbazaar_lightning::keys`: LDK entropy and the state key) |
//! | moderator     | `m/1984'/coin'/7'/0'` (seed of the ruling and share keys), `7'/1'` (X25519 mailbox for dispute letters) |
//! | BIP 352 scan  | `m/352'/coin'/0'/1'/0`       |
//! | BIP 352 spend | `m/352'/coin'/0'/0'/0`       |
//! | Zcash         | ZIP 32 from the BIP 39 seed, account 0 (see `openbazaar_zec::keys`), not BIP 32 |
//!
//! `coin` is 0 on mainnet and 1 on testnet4, so the same phrase never yields
//! the same key on both networks. ZIP 32 uses Zcash's coin type (133 on
//! mainnet, 1 on testnet) and hashes the seed under its own personalization,
//! so a Zcash key is never one of the BIP 32 keys above, and the phrase
//! restores the Zcash funds in any ZIP 32 wallet.

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
    /// The BIP 39 seed of the phrase, empty passphrase. Only ZIP 32 reads it.
    bip39: zeroize::Zeroizing<[u8; 64]>,
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
        let bip39 = zeroize::Zeroizing::new(mnemonic.to_seed(""));
        let master = Xpriv::new_master(network.bitcoin(), &bip39[..])
            .map_err(|err| Error::Detail(err.to_string()))?;
        Ok(Self { master, bip39, network })
    }

    /// The Zcash viewing key of this phrase: ZIP 32 account 0 of the BIP 39 seed.
    pub(crate) fn zcash_view_key(&self) -> Result<openbazaar_zec::ViewKey, Error> {
        openbazaar_zec::ViewKey::from_seed(&self.bip39[..], self.network.zcash()).map_err(Error::Detail)
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

    /// Opens escrow messages sent to this node as buyer.
    pub(crate) fn mail_key(&self) -> SecretKey {
        self.derive(&format!("m/1984'/{}'/2'/0'", self.network.coin_type()))
    }

    /// The secret the Monero spend and view keys are derived from (see `openbazaar_xmr::keys`).
    pub(crate) fn monero_secret(&self) -> zeroize::Zeroizing<[u8; 32]> {
        zeroize::Zeroizing::new(self.derive(&format!("m/1984'/{}'/5'/0'", self.network.coin_type())).secret_bytes())
    }

    /// The secret the LDK node's keys and its state key come from (see `openbazaar_lightning::keys`).
    pub(crate) fn lightning_secret(&self) -> zeroize::Zeroizing<[u8; 32]> {
        zeroize::Zeroizing::new(self.derive(&format!("m/1984'/{}'/6'/0'", self.network.coin_type())).secret_bytes())
    }

    /// The X25519 secret of the mailbox key published in this node's store
    /// record. Its own path: never the store signing scalar.
    pub(crate) fn store_mailbox_secret(&self) -> [u8; 32] {
        self.derive(&format!("m/1984'/{}'/3'/0'", self.network.coin_type()))
            .secret_bytes()
    }

    /// Root of the per-conversation letter keys. It never signs anything itself.
    pub(crate) fn letter_root(&self) -> [u8; 32] {
        self.derive(&format!("m/1984'/{}'/4'/0'", self.network.coin_type()))
            .secret_bytes()
    }

    /// The seed of this node's moderator keys (`openbazaar_escrow::ModeratorKeys`).
    pub(crate) fn moderator_seed(&self) -> zeroize::Zeroizing<[u8; 32]> {
        zeroize::Zeroizing::new(self.derive(&format!("m/1984'/{}'/7'/0'", self.network.coin_type())).secret_bytes())
    }

    /// The X25519 secret of the mailbox key in this node's moderator profile.
    pub(crate) fn moderator_mailbox_secret(&self) -> [u8; 32] {
        self.derive(&format!("m/1984'/{}'/7'/1'", self.network.coin_type()))
            .secret_bytes()
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
                seed.mail_key(),
                SecretKey::from_slice(&seed.store_mailbox_secret()).unwrap(),
                SecretKey::from_slice(&seed.letter_root()).unwrap(),
                seed.scan_key(),
                seed.silent_spend_key(),
                SecretKey::from_slice(&seed.monero_secret()[..]).unwrap(),
                SecretKey::from_slice(&seed.lightning_secret()[..]).unwrap(),
                SecretKey::from_slice(&seed.moderator_seed()[..]).unwrap(),
                SecretKey::from_slice(&seed.moderator_mailbox_secret()).unwrap(),
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

    #[test]
    fn zcash_keys_come_from_the_bip39_seed_as_in_a_zcash_wallet() {
        let test = Seed::from_phrase(PHRASE, NetworkMode::Testnet4).unwrap();
        // BIP 39's published seed for this phrase with an empty passphrase.
        let expected = "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc1\
                        9a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4";
        assert_eq!(crate::escrow_mail::hex(&test.bip39[..]), expected);
        let key = test.zcash_view_key().unwrap();
        let wallet = openbazaar_zec::ViewKey::from_seed(&test.bip39[..], openbazaar_zec::ZecNetwork::Testnet).unwrap();
        assert_eq!(*key.ufvk(), *wallet.ufvk(), "what a Zcash wallet restored from the phrase sees");
        assert!(key.ufvk().starts_with("uviewtest1"), "testnet4 mode is Zcash testnet");
        let main = Seed::from_phrase(PHRASE, NetworkMode::Mainnet).unwrap().zcash_view_key().unwrap();
        assert!(main.ufvk().starts_with("uview1"));
        assert_ne!(main.address(1), key.address(1));
    }
}
