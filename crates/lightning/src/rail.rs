//! What ob-node asks of a Lightning node.

use std::fmt;
use std::str::FromStr;

use ldk_node::bitcoin::Network;
use ldk_node::lightning_invoice::{Bolt11Invoice, Bolt11InvoiceDescriptionRef};

use crate::config::PeerAddress;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LnError {
    /// The node is not running (yet), or stopped.
    NotRunning,
    /// The invoice or argument is unusable; the text says why.
    Invalid(String),
    /// Not enough outbound capacity, on-chain funds, or inbound room.
    Insufficient(String),
    /// The LSP did not answer or refused.
    Lsp(String),
    /// Anything else LDK reported.
    Failed(String),
}

impl fmt::Display for LnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => f.write_str("the Lightning node is not running"),
            Self::Invalid(text) | Self::Insufficient(text) | Self::Lsp(text) | Self::Failed(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for LnError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo {
    pub node_id: String,
    pub network: Network,
    pub running: bool,
    /// The LSP's node id, and whether this node is connected to it now.
    pub lsp: Option<(String, bool)>,
    /// Best block LDK has seen.
    pub best_height: u32,
    pub last_wallet_sync: Option<u64>,
    pub last_lightning_sync: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Balances {
    /// What this node can send over its channels now.
    pub lightning_sats: u64,
    /// What it can receive over existing channels without a new one.
    pub inbound_sats: u64,
    /// LDK's own on-chain wallet: channel closes land here.
    pub onchain_total_sats: u64,
    pub onchain_spendable_sats: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    /// Stable handle for close requests (LDK's user channel id, decimal).
    pub id: String,
    pub peer: String,
    pub capacity_sats: u64,
    pub outbound_msat: u64,
    pub inbound_msat: u64,
    pub ready: bool,
    pub usable: bool,
    /// The funding transaction, once known.
    pub funding_txid: Option<String>,
}

/// An invoice this node issued.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuedInvoice {
    pub bolt11: String,
    pub payment_hash: String,
    pub amount_msat: u64,
    pub expires_at: u64,
    /// Routed through the LSP, which opens a channel as the payment arrives.
    pub jit: bool,
    /// The most the LSP may keep for that channel (`jit` only).
    pub lsp_fee_cap_msat: Option<u64>,
}

/// An invoice this node was asked to pay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvoiceInfo {
    pub payment_hash: String,
    pub amount_msat: Option<u64>,
    pub payee: String,
    pub expires_at: u64,
    pub description: String,
    pub network: Network,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incoming {
    /// Unknown to this node.
    Unknown,
    Pending,
    /// Claimed: the money is this node's. `received_msat` is after any LSP fee.
    Paid { received_msat: u64, lsp_fee_msat: u64, at: u64 },
    /// Expired or failed.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outgoing {
    Unknown,
    Pending,
    Paid { fee_msat: u64, at: u64 },
    Failed,
}

/// A Lightning node, as ob-node uses it. Every call is blocking and quick
/// except where noted; the LDK node runs its own tasks in the background.
pub trait LightningRail: Send + Sync {
    fn info(&self) -> NodeInfo;
    fn balances(&self) -> Balances;
    fn channels(&self) -> Vec<Channel>;

    /// An invoice for `amount_msat`. Uses an existing channel when one has
    /// room, else the LSP's just-in-time channel (asks the LSP; may take
    /// seconds), else fails with [`LnError::Insufficient`].
    fn receive(&self, amount_msat: u64, description: &str, expiry_secs: u32) -> Result<IssuedInvoice, LnError>;
    fn incoming(&self, payment_hash: &str) -> Incoming;

    /// Pay `bolt11` (which must carry an amount), spending at most `max_fee_msat` in routing fees.
    fn pay(&self, bolt11: &str, max_fee_msat: u64) -> Result<String, LnError>;
    fn outgoing(&self, payment_hash: &str) -> Outgoing;

    /// A fresh address of LDK's on-chain wallet (to fund a channel by hand).
    fn onchain_address(&self) -> Result<String, LnError>;
    /// Send LDK's whole on-chain balance to `address` (the node wallet's own address).
    fn sweep_onchain(&self, address: &str) -> Result<String, LnError>;
    fn open_channel(&self, peer: &PeerAddress, sats: u64) -> Result<String, LnError>;
    /// Cooperative close, or a force close when the peer is gone.
    fn close_channel(&self, id: &str, force: bool) -> Result<(), LnError>;
    fn stop(&self);
}

/// Read a BOLT 11 invoice. Refuses one for another network.
pub fn decode_invoice(text: &str, network: Network) -> Result<InvoiceInfo, LnError> {
    let invoice = Bolt11Invoice::from_str(text.trim()).map_err(|_| LnError::Invalid("That is not a BOLT 11 invoice.".into()))?;
    let invoice_network = Network::from(invoice.network());
    let same = invoice_network == network
        || matches!((invoice_network, network), (Network::Testnet, Network::Testnet4) | (Network::Testnet4, Network::Testnet));
    if !same {
        return Err(LnError::Invalid(format!("That invoice is for {invoice_network}, not {network}.")));
    }
    let description = match invoice.description() {
        Bolt11InvoiceDescriptionRef::Direct(text) => text.to_string(),
        Bolt11InvoiceDescriptionRef::Hash(_) => String::new(),
    };
    let expires_at = invoice.duration_since_epoch().as_secs().saturating_add(invoice.expiry_time().as_secs());
    Ok(InvoiceInfo {
        payment_hash: invoice.payment_hash().to_string(),
        amount_msat: invoice.amount_milli_satoshis(),
        payee: invoice.get_payee_pub_key().to_string(),
        expires_at,
        description,
        network,
    })
}
