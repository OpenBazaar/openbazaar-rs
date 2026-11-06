//! BIP 352 silent-payment codes.
//!
//! A code is the bech32m string a buyer pays when they have no Lightning channel.
//! Construction and parsing use the `silentpayments` crate: compressed scan and
//! spend public keys, bech32m checksum, and the `sp` / `tsp` / `sprt` human-readable
//! parts. Public keys are derived with `bitcoin`'s secp256k1.
//!
//! Output scanning, label tweaks, and spend-key derivation are the home node's
//! scan loop. This module stops at the code the node publishes.

use std::fmt;
use std::str::FromStr;

use bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};
use bitcoin::Network;

use crate::error::{silent, PayError};

/// A BIP 352 silent-payment code (`sp1…`, `tsp1…`, or `sprt1…`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SilentPaymentCode {
    inner: silentpayments::SilentPaymentCode,
}

impl SilentPaymentCode {
    /// Build a version-0 code from a scan secret and a spend secret.
    pub fn from_keys(
        scan: &SecretKey,
        spend: &SecretKey,
        network: Network,
    ) -> Result<Self, PayError> {
        let secp = Secp256k1::new();
        let scan_pk = PublicKey::from_secret_key(&secp, scan);
        let spend_pk = PublicKey::from_secret_key(&secp, spend);
        Self::from_public_keys(&scan_pk, &spend_pk, network)
    }

    /// Build a version-0 code from the scan and spend public keys.
    pub fn from_public_keys(
        scan: &PublicKey,
        spend: &PublicKey,
        network: Network,
    ) -> Result<Self, PayError> {
        let inner = silentpayments::SilentPaymentCode::new_v0(*scan, *spend, sp_network(network)?);
        Ok(Self { inner })
    }

    /// Parse a silent-payment code. A bad checksum or a tampered payload fails.
    pub fn parse(code: &str) -> Result<Self, PayError> {
        let inner = silentpayments::SilentPaymentCode::from_str(code).map_err(silent)?;
        Ok(Self { inner })
    }

    /// Scan public key (`B_scan`).
    pub fn scan_pubkey(&self) -> PublicKey {
        self.inner.scan_key()
    }

    /// Spend public key (`B_spend`), unlabeled.
    pub fn spend_pubkey(&self) -> PublicKey {
        self.inner.m_pubkey()
    }

    /// Network implied by the human-readable part.
    ///
    /// Signet codes share the testnet prefix `tsp`, so a parsed signet code
    /// reports [`Network::Testnet`].
    pub fn network(&self) -> Network {
        match self.inner.network() {
            silentpayments::Network::Mainnet => Network::Bitcoin,
            silentpayments::Network::Testnet => Network::Testnet,
            silentpayments::Network::Regtest => Network::Regtest,
        }
    }
}

impl fmt::Display for SilentPaymentCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl FromStr for SilentPaymentCode {
    type Err = PayError;

    fn from_str(code: &str) -> Result<Self, Self::Err> {
        Self::parse(code)
    }
}

fn sp_network(network: Network) -> Result<silentpayments::Network, PayError> {
    Ok(match network {
        Network::Bitcoin => silentpayments::Network::Mainnet,
        Network::Regtest => silentpayments::Network::Regtest,
        Network::Testnet | Network::Signet => silentpayments::Network::Testnet,
        _ => {
            return Err(PayError::Silent(format!(
                "silent payments have no code prefix for {network}"
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BIP 352 send/receive vector 0: unlabeled code for this scan and spend key.
    const SCAN_PRIV: &str = "0f694e068028a717f8af6b9411f9a133dd3565258714cc226594b34db90c1f2c";
    const SPEND_PRIV: &str = "9d6ad855ce3417ef84e836892e5a56392bfba05fa5d97ccea30e266f540e08b3";
    const SCAN_PUB: &str = "0220bcfac5b99e04ad1a06ddfb016ee13582609d60b6291e98d01a9bc9a16c96d4";
    const SPEND_PUB: &str = "025cc9856d6f8375350e123978daac200c260cb5b5ae83106cab90484dcd8fcf36";
    const CODE: &str = "sp1qqgste7k9hx0qftg6qmwlkqtwuy6cycyavzmzj85c6qdfhjdpdjtdgqjuexzk6murw56suy3e0rd2cgqvycxttddwsvgxe2usfpxumr70xc9pkqwv";

    #[test]
    fn vector_code_round_trips() {
        let code =
            SilentPaymentCode::from_keys(&secret(SCAN_PRIV), &secret(SPEND_PRIV), Network::Bitcoin)
                .unwrap();
        assert_eq!(code.to_string(), CODE);
        assert_eq!(code.network(), Network::Bitcoin);
        assert_eq!(hex::encode(code.scan_pubkey().serialize()), SCAN_PUB);
        assert_eq!(hex::encode(code.spend_pubkey().serialize()), SPEND_PUB);

        let parsed = SilentPaymentCode::parse(CODE).unwrap();
        assert_eq!(parsed, code);
        assert_eq!(parsed.scan_pubkey(), code.scan_pubkey());
        assert_eq!(parsed.spend_pubkey(), code.spend_pubkey());
    }

    #[test]
    fn public_keys_match_secrets() {
        let scan = secret(SCAN_PRIV);
        let spend = secret(SPEND_PRIV);
        let secp = Secp256k1::new();
        let from_pub = SilentPaymentCode::from_public_keys(
            &PublicKey::from_secret_key(&secp, &scan),
            &PublicKey::from_secret_key(&secp, &spend),
            Network::Bitcoin,
        )
        .unwrap();
        let from_sec = SilentPaymentCode::from_keys(&scan, &spend, Network::Bitcoin).unwrap();
        assert_eq!(from_pub, from_sec);
    }

    #[test]
    fn tampered_and_truncated_codes_fail() {
        let mut chars: Vec<char> = CODE.chars().collect();
        let last = chars.len() - 1;
        chars[last] = if chars[last] == 'q' { 'p' } else { 'q' };
        let tampered: String = chars.into_iter().collect();
        assert!(SilentPaymentCode::parse(&tampered).is_err());
        assert!(SilentPaymentCode::parse("sp1qqqqqqqqqqqqqqqq").is_err());
        assert!(SilentPaymentCode::parse("").is_err());
        assert!(SilentPaymentCode::parse("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4").is_err());
    }

    fn secret(hex_key: &str) -> SecretKey {
        let bytes = decode_hex(hex_key);
        SecretKey::from_slice(&bytes).unwrap()
    }

    fn decode_hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
            .collect()
    }

    mod hex {
        pub fn encode(bytes: impl AsRef<[u8]>) -> String {
            bytes
                .as_ref()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        }
    }
}
