//! Dispute filing fees over Lightning.
//!
//! A moderator offers Lightning only when its own channels can take the fee:
//! a just-in-time channel would cost an LSP round trip for every reviewed
//! letter, paid or not, and the LSP's fee could exceed the filing fee. The
//! filer pays the invoice from its own Lightning node, the same as any remote
//! invoice; the preimage it gets back is its proof of payment.

use openbazaar_lightning::{decode_invoice, Incoming, Outgoing};

use super::pay::fee_cap;
use crate::Node;

/// An invoice for one fee request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FeeInvoice {
    pub network: &'static str,
    pub bolt11: String,
    pub payment_hash: String,
}

/// How a filer's Lightning payment of a fee request stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FeePaid {
    Pending,
    Paid { fee_msat: u64, preimage: Option<[u8; 32]> },
    Failed,
}

/// Fee invoices expire after an hour; a re-sent letter gets a fresh one.
pub(crate) const FEE_INVOICE_SECS: u32 = 3_600;

impl Node {
    /// An invoice for `amount_sats`, or `None` when Lightning is off or the
    /// fee would need a just-in-time channel.
    pub(crate) fn ln_fee_invoice(&self, amount_sats: u64, label: &str) -> Option<FeeInvoice> {
        if self.ln_blocked() {
            return None;
        }
        let rail = self.ln.rail()?;
        let config = self.ln.config()?;
        let amount_msat = amount_sats.checked_mul(1_000)?;
        if self.ln_fee_room_msat()? < amount_msat {
            return None;
        }
        let issued = rail.receive(amount_msat, &format!("OpenBazaar dispute fee {label}"), FEE_INVOICE_SECS).ok()?;
        if issued.jit {
            return None;
        }
        Some(FeeInvoice {
            network: super::options::network_name(config.network),
            bolt11: issued.bolt11,
            payment_hash: issued.payment_hash,
        })
    }

    /// The most one usable channel can receive, in msat. `None` when Lightning is off.
    fn ln_fee_room_msat(&self) -> Option<u64> {
        let rail = self.ln.rail()?;
        self.ln.config()?;
        Some(rail.channels().iter().filter(|channel| channel.usable).map(|channel| channel.inbound_msat).max().unwrap_or(0))
    }

    /// Why filers would pay a filing fee of `fee_sats` on-chain, if they
    /// would: Lightning is off here, or no channel has room for the fee. A
    /// fee under the LSP's fee cap could not pay for a new channel anyway.
    pub(crate) fn ln_fee_warning(&self, fee_sats: u64) -> Option<String> {
        if fee_sats == 0 {
            return None;
        }
        let room = self.ln_fee_room_msat().filter(|_| !self.ln_blocked());
        let Some(room) = room else {
            return Some("Lightning is off on this node, so filers pay the filing fee on-chain.".into());
        };
        let fee_msat = fee_sats.saturating_mul(1_000);
        if room >= fee_msat {
            return None;
        }
        let cap_sats = self.ln.config()?.lsp_fee_cap_msat(fee_msat) / 1_000;
        Some(if fee_sats <= cap_sats {
            format!("No Lightning channel here has room for {fee_sats} sats, and a new channel can cost up to {cap_sats} sats, more than the fee. Filers pay it on-chain.")
        } else {
            format!("No Lightning channel here has room for {fee_sats} sats, so filers pay the fee on-chain until one does.")
        })
    }

    /// What this node received for a fee invoice it issued, in msat.
    pub(crate) fn ln_fee_received(&self, payment_hash: &str) -> Option<u64> {
        match self.ln.rail()?.incoming(payment_hash) {
            Incoming::Paid { received_msat, .. } => Some(received_msat),
            _ => None,
        }
    }

    /// Whether this node can pay `bolt11` on `network` for exactly
    /// `amount_sats`: its Lightning node runs on that network, did not issue
    /// the invoice, and holds the amount plus the routing fee cap.
    pub(crate) fn ln_can_pay_fee(&self, network: &str, bolt11: &str, payment_hash: &str, amount_sats: u64) -> Result<u64, String> {
        if self.ln_blocked() {
            return Err(super::MAINNET_BLOCKED.into());
        }
        let (Some(rail), Some(config)) = (self.ln.rail(), self.ln.config()) else {
            return Err("Lightning is not running on this node.".into());
        };
        if super::options::network_name(config.network) != network {
            return Err(format!("The moderator's invoice is for {network}; this node's Lightning runs on {}.", super::options::network_name(config.network)));
        }
        let info = decode_invoice(bolt11, config.network).map_err(|err| err.to_string())?;
        let amount_msat = amount_sats.saturating_mul(1_000);
        if info.payment_hash != payment_hash || info.amount_msat != Some(amount_msat) {
            return Err("The moderator's invoice does not match its signed request.".into());
        }
        if info.payee == rail.info().node_id {
            return Err("This node issued that invoice.".into());
        }
        if info.expires_at <= crate::buyer_orders::now_secs() {
            return Err("The moderator's invoice has expired. Send the dispute again for a fresh one.".into());
        }
        let cap = fee_cap(amount_msat);
        if rail.balances().lightning_sats.saturating_mul(1_000) < amount_msat.saturating_add(cap) {
            return Err("This node's Lightning balance cannot cover the fee.".into());
        }
        Ok(cap)
    }

    /// Pay a fee invoice already checked by [`Self::ln_can_pay_fee`].
    pub(crate) fn ln_pay_fee(&self, bolt11: &str, fee_cap_msat: u64) -> Result<String, String> {
        let rail = self.ln.rail().ok_or("Lightning is not running on this node.")?;
        rail.pay(bolt11, fee_cap_msat).map_err(|err| err.to_string())
    }

    pub(crate) fn ln_fee_paid(&self, payment_hash: &str) -> FeePaid {
        let Some(rail) = self.ln.rail() else { return FeePaid::Pending };
        match rail.outgoing(payment_hash) {
            Outgoing::Paid { fee_msat, preimage, .. } => FeePaid::Paid { fee_msat, preimage },
            Outgoing::Failed => FeePaid::Failed,
            Outgoing::Pending | Outgoing::Unknown => FeePaid::Pending,
        }
    }
}
