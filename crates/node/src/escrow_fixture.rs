//! TESTNET4 ONLY. The published moderator record and the mailbox keys the
//! escrow demo uses.
//!
//! Every secret here is derived from a public fixture label, so anyone can
//! rebuild the moderator keys and the vendor mailbox keys from this source.
//! That is acceptable only for testnet4 coins. [`moderator_record`] refuses
//! every other network, so a mainnet node cannot build escrow terms with it.

use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{Keypair, Secp256k1};
use bitcoin::{Amount, ScriptBuf};
use openbazaar_escrow::{Context, ModeratorKeys, ModeratorRecord, Policy};
use openbazaar_protocol::{fixture_keys, vendor_mail_key};
use tormail::{StoreKey, StoreSecret};

use crate::network::NetworkMode;
use crate::Node;

/// The fixture moderator's fee, paid only by a dispute outcome.
pub const MODERATOR_FEE_SATS: u64 = 1_000;

const MODERATOR_MAIL_LABEL: &[u8] = b"openbazaar/testnet4/fixture/moderator-mail";

fn testnet_only(network: NetworkMode) -> Result<(), String> {
    if network != NetworkMode::Testnet4 {
        return Err(crate::instructions::ESCROW_MAINNET_REFUSAL.to_string());
    }
    Ok(())
}

/// The fixture moderator's secrets. Public; testnet4 only.
pub(crate) fn moderator_keys(network: NetworkMode) -> Result<ModeratorKeys, String> {
    testnet_only(network)?;
    ModeratorKeys::from_seed(&fixture_keys().moderator.secret_bytes())
        .map_err(|err| err.to_string())
}

/// The record the fixture moderator publishes: its keys, payout, fee, and policy.
pub(crate) fn moderator_record(network: NetworkMode) -> Result<ModeratorRecord, String> {
    let keys = moderator_keys(network)?;
    let secp = Secp256k1::new();
    let payout_key = Keypair::from_secret_key(&secp, &fixture_keys().moderator)
        .x_only_public_key()
        .0;
    let payout = ScriptBuf::new_p2tr(&secp, payout_key, None);
    Ok(keys.record(
        payout,
        Amount::from_sat(MODERATOR_FEE_SATS),
        Policy::default(),
    ))
}

pub(crate) fn moderator_mail_secret() -> StoreSecret {
    let bytes = sha256::Hash::hash(MODERATOR_MAIL_LABEL).to_byte_array();
    StoreSecret::from_bytes(bytes).expect("any 32 bytes are an X25519 secret")
}

pub(crate) fn moderator_mail_key() -> StoreKey {
    moderator_mail_secret().public_key()
}

/// The mailbox secret a seller's order letters are sealed to.
pub(crate) fn vendor_mail_secret(seller: &str) -> Option<StoreSecret> {
    let secret = vendor_mail_key(seller)?;
    StoreSecret::from_bytes(secret.secret_bytes()).ok()
}

impl Node {
    /// The context every escrow check on this node runs against: the chain
    /// tip from this node's chain source and the fixture moderator record.
    pub(crate) fn escrow_context(&self) -> Result<Context, String> {
        let moderator = moderator_record(self.network)?;
        let tip = self.chain.tip_height()?;
        Ok(Context {
            tip,
            moderator,
            policy: Policy::default(),
        })
    }
}
