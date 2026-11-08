//! `POST /escrow/bump`: CPFP an escrow spend through its P2A anchor.
//!
//! Body: `{"escrow": id, "spend": "refund"|"claim"|"settle",
//! "feeRateSatVb": n, "maxFeeSats": n, "passphrase": s}`. The spend is the
//! buyer's refund or dispute outcome, or the vendor's claim or dispute
//! outcome, as stored on this node. The parent is broadcast (a refund or
//! claim is entered first), then a child that pays from the node wallet so
//! the pair reaches `feeRateSatVb`.
//!
//! The child spends the node's own coins for a fee the caller names, so the
//! request needs the spend tier plus the passphrase again, and a
//! `maxFeeSats` the child may not exceed. A spend ticket does not fit: it
//! pins a recipient and amount, and a child pays no one.

use bitcoin::Transaction;
use serde_json::{json, Value};

use crate::escrow_fee::{cpfp_child, Child};
use crate::escrow_mail::{parse_id, EscrowId};
use crate::instructions::ESCROW_MAINNET_REFUSAL;
use crate::network::NetworkMode;
use crate::pay_order::PayError;
use crate::Node;

struct Bump<'a> {
    escrow: EscrowId,
    spend: &'a str,
    rate: f64,
    max_fee: u64,
    passphrase: &'a str,
    link_coins: bool,
}

fn parse(body: &Value) -> Result<Bump<'_>, PayError> {
    let invalid = |message: &str| PayError::Invalid(message.into());
    let known = [
        "escrow",
        "spend",
        "feeRateSatVb",
        "maxFeeSats",
        "passphrase",
        "linkCoins",
    ];
    let map = body
        .as_object()
        .ok_or_else(|| invalid("Send a JSON object."))?;
    if map.keys().any(|key| !known.contains(&key.as_str())) {
        return Err(invalid("That bump has a field this node does not know."));
    }
    let escrow = body["escrow"]
        .as_str()
        .and_then(parse_id)
        .ok_or_else(|| invalid("Name the escrow id to bump."))?;
    let spend = body["spend"]
        .as_str()
        .filter(|spend| ["refund", "claim", "settle"].contains(spend))
        .ok_or_else(|| invalid("The spend is refund, claim or settle."))?;
    let rate = body["feeRateSatVb"]
        .as_f64()
        .filter(|rate| rate.is_finite() && *rate >= 1.0 && *rate <= 10_000.0)
        .ok_or_else(|| invalid("Give a fee rate of 1 to 10000 sat/vB."))?;
    let max_fee = body["maxFeeSats"]
        .as_u64()
        .ok_or_else(|| invalid("Give the most the bump may pay, in sats."))?;
    let passphrase = body["passphrase"]
        .as_str()
        .ok_or_else(|| invalid("Enter the wallet passphrase to pay a bump."))?;
    let link_coins = match body.get("linkCoins") {
        None => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => return Err(invalid("linkCoins must be true or false.")),
    };
    Ok(Bump {
        escrow,
        spend,
        rate,
        max_fee,
        passphrase,
        link_coins,
    })
}

/// What `parent` pays alone: the funding it spends minus its outputs.
fn parent_fee(parent: &Transaction, funding: u64) -> u64 {
    let out: u64 = parent.output.iter().map(|out| out.value.to_sat()).sum();
    funding.saturating_sub(out)
}

impl Node {
    pub fn bump_escrow(&self, body: &Value) -> Result<Value, PayError> {
        if self.network != NetworkMode::Testnet4 {
            return Err(PayError::Invalid(ESCROW_MAINNET_REFUSAL.into()));
        }
        let bump = parse(body)?;
        self.lock.verify(bump.passphrase)?;
        let lock = self.escrow.locks.get(&bump.escrow);
        let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
        let (parent, funding) = self
            .bump_parent(&bump.escrow, bump.spend)?
            .ok_or(PayError::NotFound)?;
        // The parent goes out even if no child can be paid for: it was entered.
        if let Err(err) = self.chain.broadcast(&parent) {
            // Already in the mempool or mined: the child still applies.
            eprintln!(
                "ob-node: escrow bump parent {}: {err}",
                parent.compute_txid()
            );
        }
        self.sync_buyer_wallet().map_err(PayError::Chain)?;
        let child = self.sign_bump(&parent, parent_fee(&parent, funding), &bump)?;
        self.chain.broadcast(&child.tx).map_err(PayError::Chain)?;
        {
            let mut wallet = self.buyer_wallet();
            wallet
                .wallet
                .apply_unconfirmed_txs([(child.tx.clone(), crate::escrow_outbox::now_secs())]);
            wallet
                .persist()
                .map_err(|err| PayError::Failed(err.to_string()))?;
        }
        Ok(json!({
            "parentTxid": parent.compute_txid().to_string(),
            "childTxid": child.tx.compute_txid().to_string(),
            "childFeeSats": child.fee_sats,
            "packageRateSatVb": child.package_rate,
        }))
    }

    fn bump_parent(
        &self,
        id: &EscrowId,
        spend: &str,
    ) -> Result<Option<(Transaction, u64)>, PayError> {
        let found = match spend {
            "refund" => self.buyer_bump_parent(id, spend),
            "claim" => self.vendor_bump_parent(id, spend),
            _ => match self.buyer_bump_parent(id, spend) {
                Ok(None) => self.vendor_bump_parent(id, spend),
                other => other,
            },
        };
        found.map_err(PayError::Conflict)
    }

    fn sign_bump(
        &self,
        parent: &Transaction,
        parent_fee: u64,
        bump: &Bump<'_>,
    ) -> Result<Child, PayError> {
        let locked = self.escrow.coins.outpoints();
        let mut wallet = self.buyer_wallet();
        let allowed = self.funding_script(&bump.escrow);
        let blocked = crate::coin_provenance::blocked_coins(
            &wallet.wallet,
            allowed.as_deref(),
            bump.link_coins,
        );
        let unspendable: Vec<_> = locked.into_iter().chain(blocked.iter().copied()).collect();
        let seed = self.spend_seed(&wallet)?;
        let child = cpfp_child(
            &mut wallet,
            &seed,
            parent,
            parent_fee,
            bump.rate,
            &unspendable,
        )
        .map_err(|err| {
            let mut message = err;
            if !blocked.is_empty() {
                message.push(' ');
                message.push_str(crate::coin_provenance::LINK_EXPLANATION);
            }
            PayError::Invalid(message)
        })?;
        if child.fee_sats > bump.max_fee {
            return Err(PayError::Conflict(format!(
                "That bump needs {} sats, more than the {} allowed.",
                child.fee_sats, bump.max_fee
            )));
        }
        Ok(child)
    }

    /// The escrow funding address, when this node has the order. Change from
    /// funding that escrow may pay its fee bump. Anything else stays blocked
    /// unless the caller set `linkCoins`.
    fn funding_script(&self, id: &crate::escrow_mail::EscrowId) -> Option<bitcoin::ScriptBuf> {
        let wanted = crate::escrow_mail::id_hex(id);
        let orders = self.orders().list_value();
        let orders = orders.get("orders")?.as_array()?;
        for order in orders {
            let escrow = order.get("escrow")?;
            if escrow.get("id").and_then(Value::as_str) != Some(wanted.as_str()) {
                continue;
            }
            let address = escrow.get("address").and_then(Value::as_str)?;
            return crate::network::address_for(address, self.network)
                .ok()
                .map(|address| address.script_pubkey());
        }
        None
    }
}
