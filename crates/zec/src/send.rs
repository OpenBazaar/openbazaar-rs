//! One Ironwood payment to one address, change back to the wallet.
//!
//! A quote picks confirmed notes (largest first) and prices the transaction
//! with ZIP 317, which does not depend on the network. Signing builds and
//! proves the same shape and refuses if the fee is above the cap the user
//! approved. Orchard notes are not spent here: moving them into Ironwood is
//! a pool migration, not a payment.

use std::cmp::max;

use orchard::builder::BundleType;
use orchard::bundle::BundleVersion;
use orchard::keys::{FullViewingKey, Scope, SpendAuthorizingKey};
use rand10::rngs::SysRng;
use rand_core::{Rng, UnwrapErr};
use sapling_crypto::prover::{OutputProver, SpendProver};
use zcash_keys::keys::UnifiedSpendingKey;
use zcash_primitives::transaction::builder::{BuildConfig, Builder, BundlePadding};
use zcash_primitives::transaction::fees::zip317::{FeeRule, GRACE_ACTIONS, MARGINAL_FEE};
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::memo::MemoBytes;
use zcash_protocol::value::Zatoshis;
use zcash_transparent::builder::TransparentSigningSet;
use zeroize::Zeroizing;
use zip32::AccountId;

use crate::amount::format_zec;
use crate::tx;
use crate::wallet::ZecWallet;
use crate::ZecNetwork;

const MAX_INPUTS: usize = 10;

/// The phrase's seed. Held only while a payment is signed, then dropped.
pub struct SpendKey {
    seed: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for SpendKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SpendKey")
    }
}

impl SpendKey {
    pub fn from_seed(seed: &[u8]) -> Result<Self, SendError> {
        if seed.len() < 32 {
            return Err(SendError::Library("a ZIP 32 seed has at least 32 bytes".into()));
        }
        Ok(Self { seed: Zeroizing::new(seed.to_vec()) })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    Address(String),
    NotEnough { needed: u64, spendable: u64 },
    FeeAboveCap { fee: u64, cap: u64 },
    Sync(String),
    WrongKey,
    Library(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::Address(why) => f.write_str(why),
            SendError::NotEnough { needed, spendable } => write!(
                f,
                "This needs {} ZEC including the fee; the wallet has {} ZEC that can be spent now.",
                format_zec(*needed),
                format_zec(*spendable)
            ),
            SendError::FeeAboveCap { fee, cap } => write!(
                f,
                "The fee is {} ZEC, above the {} ZEC you approved. Get a new quote.",
                format_zec(*fee),
                format_zec(*cap)
            ),
            SendError::Sync(why) => f.write_str(why),
            SendError::WrongKey => f.write_str("The spend key does not belong to this Zcash wallet."),
            SendError::Library(err) => write!(f, "Building the Zcash transaction failed: {err}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaymentQuote {
    pub destination: String,
    pub amount: u64,
    pub fee: u64,
}

pub struct SignedPayment {
    pub raw: Vec<u8>,
    pub txid: [u8; 32],
    pub fee: u64,
    pub nullifiers: Vec<[u8; 32]>,
    pub expiry_height: u64,
}

struct Plan {
    nullifiers: Vec<[u8; 32]>,
    fee: u64,
    change: u64,
}

/// Price `amount` zatoshi to `destination`. No network, and no proof.
pub fn quote_payment(wallet: &ZecWallet, destination: &str, amount: u64) -> Result<PaymentQuote, SendError> {
    check_destination(wallet.network(), destination)?;
    let plan = plan(wallet, amount)?;
    Ok(PaymentQuote { destination: destination.to_string(), amount, fee: plan.fee })
}

/// Build, prove, and sign the payment. `fee_cap` is what the user approved.
pub fn sign_payment(wallet: &ZecWallet, key: &SpendKey, destination: &str, amount: u64, fee_cap: u64) -> Result<SignedPayment, SendError> {
    let recipient = check_destination(wallet.network(), destination)?;
    let plan = plan(wallet, amount)?;
    if plan.fee > fee_cap {
        return Err(SendError::FeeAboveCap { fee: plan.fee, cap: fee_cap });
    }
    let (ask, fvk) = spend_key(wallet, key)?;
    let anchor = wallet.ironwood_anchor().ok_or_else(|| SendError::Sync("The Zcash wallet has no note-commitment anchor yet.".into()))?;
    let notes = wallet.ironwood_inputs(&plan.nullifiers).map_err(SendError::Library)?;
    let height = next_height(wallet)?;
    let mut builder = Builder::new(
        wallet.network().params(),
        height,
        BuildConfig::Standard {
            sapling_anchor: None,
            orchard_anchor: None,
            ironwood_anchor: Some(anchor),
            orchard_padding: BundlePadding::DEFAULT,
            ironwood_padding: BundlePadding::DEFAULT,
        },
    );
    for note in &notes {
        builder.add_ironwood_spend::<Zip317Error>(fvk.clone(), note.note.clone(), note.path.clone()).map_err(|err| SendError::Library(err.to_string()))?;
    }
    let ovk = fvk.to_ovk(Scope::External);
    builder
        .add_ironwood_output::<Zip317Error>(Some(ovk.clone()), recipient, zatoshis(amount)?, MemoBytes::empty())
        .map_err(|err| SendError::Library(err.to_string()))?;
    if plan.change > 0 {
        let change = fvk.address_at(0u32, Scope::Internal);
        builder
            .add_ironwood_output::<Zip317Error>(Some(ovk), change, zatoshis(plan.change)?, MemoBytes::empty())
            .map_err(|err| SendError::Library(err.to_string()))?;
    }
    let priced = builder.get_fee(&FeeRule::standard()).map_err(|err| SendError::Library(err.to_string()))?;
    if priced.into_u64() != plan.fee {
        return Err(SendError::Library(format!("the fee was priced at {} zatoshi and built as {}", plan.fee, priced.into_u64())));
    }
    let built = builder
        .build(&TransparentSigningSet::new(), &[], &[ask], UnwrapErr(SysRng), &NoSapling, &NoSapling, &FeeRule::standard())
        .map_err(|err| SendError::Library(err.to_string()))?;
    let mut raw = Vec::new();
    built.transaction().write(&mut raw).map_err(|err| SendError::Library(err.to_string()))?;
    let (txid, _) = tx::ironwood_compact(&raw).map_err(SendError::Library)?;
    let expiry_height = tx::expiry_of(&raw).map_err(SendError::Library)?;
    Ok(SignedPayment {
        expiry_height,
        raw,
        txid,
        fee: plan.fee,
        nullifiers: plan.nullifiers,
    })
}

fn plan(wallet: &ZecWallet, amount: u64) -> Result<Plan, SendError> {
    if amount == 0 {
        return Err(SendError::Address("The amount must be above zero.".into()));
    }
    if wallet.scanned() != wallet.tip() || wallet.scanned().is_none() {
        return Err(SendError::Sync("The Zcash wallet is still syncing.".into()));
    }
    if wallet.trees_height() != wallet.scanned() {
        return Err(SendError::Sync("The Zcash wallet is still building note witnesses.".into()));
    }
    let notes = wallet.ironwood_spendable();
    let spendable = notes.iter().map(|note| note.value).sum();
    for count in 1..=notes.len().min(MAX_INPUTS) {
        let chosen = &notes[..count];
        let total: u64 = chosen.iter().map(|note| note.value).sum();
        let with_change = fee_for(count, 2)?;
        let exact = fee_for(count, 1)?;
        let (fee, change) = if total == amount.saturating_add(exact) {
            (exact, 0)
        } else if total > amount.saturating_add(with_change) {
            (with_change, total - amount - with_change)
        } else {
            continue;
        };
        return Ok(Plan { nullifiers: chosen.iter().map(|note| note.nullifier).collect(), fee, change });
    }
    let fee = fee_for(1, 2).unwrap_or(MARGINAL_FEE.into_u64() * GRACE_ACTIONS as u64);
    Err(SendError::NotEnough { needed: amount.saturating_add(fee), spendable })
}

fn fee_for(spends: usize, outputs: usize) -> Result<u64, SendError> {
    let flags = BundleVersion::ironwood_v3().default_flags();
    let actions = BundleType::DEFAULT.num_actions(flags, spends, outputs).map_err(|err| SendError::Library(err.into()))?;
    let logical = max(GRACE_ACTIONS, actions);
    MARGINAL_FEE.into_u64().checked_mul(logical as u64).ok_or_else(|| SendError::Library("the fee overflowed".into()))
}

fn check_destination(network: ZecNetwork, address: &str) -> Result<orchard::Address, SendError> {
    let params = network.params();
    let ua = zcash_keys::address::UnifiedAddress::decode(&params, address)
        .map_err(|_| SendError::Address(format!("That is not a Zcash {network} address.")))?;
    ua.orchard().copied().ok_or_else(|| SendError::Address("That address has no shielded receiver this wallet can pay.".into()))
}

fn spend_key(wallet: &ZecWallet, key: &SpendKey) -> Result<(SpendAuthorizingKey, FullViewingKey), SendError> {
    let params = wallet.network().params();
    let usk = UnifiedSpendingKey::from_seed(&params, &key.seed, AccountId::ZERO).map_err(|err| SendError::Library(format!("Zcash key derivation failed: {err:?}")))?;
    let fvk = FullViewingKey::from(usk.orchard());
    if fvk.to_bytes() != wallet.keys().orchard().to_bytes() {
        return Err(SendError::WrongKey);
    }
    Ok((SpendAuthorizingKey::from(usk.orchard()), fvk))
}

fn next_height(wallet: &ZecWallet) -> Result<BlockHeight, SendError> {
    let tip = wallet.tip().ok_or_else(|| SendError::Sync("The Zcash wallet has not read the chain yet.".into()))?;
    let next = tip.checked_add(1).ok_or_else(|| SendError::Library("the chain tip is out of range".into()))?;
    u32::try_from(next).map(BlockHeight::from_u32).map_err(|_| SendError::Library("the chain tip is out of range".into()))
}

fn zatoshis(amount: u64) -> Result<Zatoshis, SendError> {
    Zatoshis::from_u64(amount).map_err(|_| SendError::Library("the amount is out of range".into()))
}

/// Named so the builder's unused fee-error parameter can be written down.
type Zip317Error = zcash_primitives::transaction::fees::zip317::FeeError;

/// Sapling is not spent or created. The builder still asks for these provers.
struct NoSapling;

impl SpendProver for NoSapling {
    type Proof = ();

    fn prepare_circuit(
        _proof_generation_key: sapling_crypto::keys::ProofGenerationKey,
        _diversifier: sapling_crypto::keys::Diversifier,
        _rseed: sapling_crypto::Rseed,
        _value: sapling_crypto::value::NoteValue,
        _alpha: jubjub::Fr,
        _rcv: sapling_crypto::value::ValueCommitTrapdoor,
        _anchor: bls12_381::Scalar,
        _merkle_path: sapling_crypto::MerklePath,
    ) -> Option<sapling_crypto::circuit::Spend> {
        panic!("this wallet does not spend Sapling notes");
    }

    fn create_proof<R: Rng>(&self, _circuit: sapling_crypto::circuit::Spend, _rng: &mut R) -> Self::Proof {
        panic!("this wallet does not spend Sapling notes");
    }

    fn encode_proof(_proof: Self::Proof) -> sapling_crypto::bundle::GrothProofBytes {
        [0u8; 192]
    }
}

impl OutputProver for NoSapling {
    type Proof = ();

    fn prepare_circuit(
        _esk: &sapling_crypto::keys::EphemeralSecretKey,
        _payment_address: sapling_crypto::PaymentAddress,
        _rcm: jubjub::Fr,
        _value: sapling_crypto::value::NoteValue,
        _rcv: sapling_crypto::value::ValueCommitTrapdoor,
    ) -> sapling_crypto::circuit::Output {
        panic!("this wallet does not create Sapling outputs");
    }

    fn create_proof<R: Rng>(&self, _circuit: sapling_crypto::circuit::Output, _rng: &mut R) -> Self::Proof {
        panic!("this wallet does not create Sapling outputs");
    }

    fn encode_proof(_proof: Self::Proof) -> sapling_crypto::bundle::GrothProofBytes {
        [0u8; 192]
    }
}

