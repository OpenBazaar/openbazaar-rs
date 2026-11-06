//! One payment to one address, change back to the wallet.
//!
//! A quote picks inputs (largest first) and prices the transaction with
//! made-up ring members, so it needs no network. Paying picks inputs again,
//! fetches real ring members from the daemon, and refuses to sign if the fee
//! has grown past the cap the user approved. Ring members change the
//! transaction's size by a few bytes, which is what the cap absorbs.

use monero_wallet::address::MoneroAddress;
use monero_wallet::interface::FeeRate;
use monero_wallet::ringct::RctType;
use monero_wallet::send::{Change, SignableTransaction};
use monero_wallet::transaction::Transaction;
use monero_wallet::{OutputWithDecoys, WalletOutput};
use rand_core::{OsRng, RngCore};
use zeroize::Zeroizing;

use crate::decoys::DecoySource;
use crate::{SpendKey, XmrWallet};

/// Ring size, fixed by consensus since hard fork 15.
pub const RING_LEN: u8 = 16;
/// Monero allows more, but a payment needing this many is better split by hand.
const MAX_INPUTS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    /// The destination is an integrated address or on another network.
    Destination(String),
    NotEnough { needed: u64, unlocked: u64 },
    FeeAboveCap { fee: u64, cap: u64 },
    /// The spend key is not this wallet's.
    WrongKey,
    Library(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use crate::amount::format_xmr;
        match self {
            SendError::Destination(why) => f.write_str(why),
            SendError::NotEnough { needed, unlocked } => write!(
                f,
                "This needs {} XMR including the fee; the wallet has {} XMR that can be spent now.",
                format_xmr(*needed),
                format_xmr(*unlocked)
            ),
            SendError::FeeAboveCap { fee, cap } => write!(
                f,
                "The network fee is now {} XMR, above the {} XMR you approved. Get a new quote.",
                format_xmr(*fee),
                format_xmr(*cap)
            ),
            SendError::WrongKey => f.write_str("The spend key does not belong to this Monero wallet."),
            SendError::Library(err) => write!(f, "Building the Monero transaction failed: {err}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PaymentQuote {
    pub destination: MoneroAddress,
    pub amount: u64,
    pub fee: u64,
    pub fee_rate: FeeRate,
    pub inputs: Vec<WalletOutput>,
}

pub struct SignedPayment {
    pub tx: Transaction,
    pub txid: [u8; 32],
    pub fee: u64,
    pub inputs: Vec<WalletOutput>,
}

/// Pick inputs for `amount` to `destination` and price the transaction.
pub fn quote_payment(
    wallet: &XmrWallet,
    destination: &MoneroAddress,
    amount: u64,
    fee_rate: FeeRate,
) -> Result<PaymentQuote, SendError> {
    if destination.network() != wallet.network().address_network() {
        return Err(SendError::Destination(format!("That address is not on Monero {}.", wallet.network())));
    }
    if destination.payment_id().is_some() {
        return Err(SendError::Destination("Integrated addresses (with a payment ID) are not supported.".into()));
    }
    if amount == 0 {
        return Err(SendError::Destination("The amount must be above zero.".into()));
    }
    let spendable = wallet.spendable();
    let unlocked: u64 = spendable.iter().map(|output| output.commitment().amount).sum();
    let mut fee = 0;
    for count in 1..=spendable.len().min(MAX_INPUTS) {
        let inputs = spendable[..count].to_vec();
        let total: u64 = inputs.iter().map(|output| output.commitment().amount).sum();
        if total < amount.saturating_add(fee) {
            continue;
        }
        let decoys = DecoySource::for_estimate(&inputs);
        let decoyed = decoys.select(inputs.clone(), decoys.tip()).map_err(SendError::Library)?;
        fee = match signable(wallet, decoyed, destination, amount, fee_rate) {
            Ok(tx) => tx.necessary_fee(),
            Err(monero_wallet::send::SendError::NotEnoughFunds { necessary_fee: Some(fee), .. }) => {
                // Not enough with this many inputs; `fee` now prices the next try.
                fee
            }
            Err(err) => return Err(SendError::Library(err.to_string())),
        };
        if total >= amount.saturating_add(fee) {
            return Ok(PaymentQuote { destination: destination.clone(), amount, fee, fee_rate, inputs });
        }
    }
    Err(SendError::NotEnough { needed: amount.saturating_add(fee), unlocked })
}

/// Sign `quote` with ring members from the daemon (`decoyed`, in the order of `quote.inputs`).
pub fn sign_payment(
    wallet: &XmrWallet,
    quote: &PaymentQuote,
    decoyed: Vec<OutputWithDecoys>,
    spend: &SpendKey,
    fee_cap: u64,
) -> Result<SignedPayment, SendError> {
    if spend.view_keys().primary_address(wallet.network()) != wallet.primary_address() {
        return Err(SendError::WrongKey);
    }
    let tx = signable(wallet, decoyed, &quote.destination, quote.amount, quote.fee_rate)
        .map_err(|err| SendError::Library(err.to_string()))?;
    let fee = tx.necessary_fee();
    if fee > fee_cap {
        return Err(SendError::FeeAboveCap { fee, cap: fee_cap });
    }
    let tx = tx.sign(&mut OsRng, spend.scalar()).map_err(|err| SendError::Library(err.to_string()))?;
    Ok(SignedPayment { txid: tx.hash(), tx, fee, inputs: quote.inputs.clone() })
}

fn signable(
    wallet: &XmrWallet,
    inputs: Vec<OutputWithDecoys>,
    destination: &MoneroAddress,
    amount: u64,
    fee_rate: FeeRate,
) -> Result<SignableTransaction, monero_wallet::send::SendError> {
    // A fresh outgoing view key per transaction; reusing one across conflicting transactions leaks.
    let mut outgoing_view_key = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(outgoing_view_key.as_mut());
    SignableTransaction::new(
        RctType::ClsagBulletproofPlus,
        outgoing_view_key,
        inputs,
        vec![(destination.clone(), amount)],
        Change::new(wallet.keys().view_pair(), None),
        Vec::new(),
        fee_rate,
    )
}
