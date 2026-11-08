//! `POST /orders/{id}/pay/bump`: replace an unconfirmed direct payment with
//! one that pays a higher fee (replace-by-fee).
//!
//! Body: `{"feeRateSatVb": n, "maxFeeSats": n, "passphrase": s}`, and
//! `"confirm": true` on mainnet. The replacement spends the payment's inputs
//! (and more of the wallet's own coins if the fee needs them), pays the same
//! recipient the same amount, and takes the rest of the fee from change.
//!
//! As with the escrow bump, a spend ticket does not fit: the recipient and
//! amount were confirmed with the first payment and cannot change here, and
//! only the fee grows. So the request needs the passphrase again and a
//! `maxFeeSats` the replacement's whole fee may not exceed.

use bitcoin::{Address, FeeRate, Transaction, Txid};
use serde_json::{json, Value};

use crate::network::NetworkMode;
use crate::pay_order::{psbt_fee, PayError, MAX_FEE_RATE, MIN_FEE_RATE};
use crate::watch::{AWAITING_PAYMENT, PAYMENT_SEEN};
use crate::Node;

struct Bump<'a> {
    rate: f64,
    max_fee: u64,
    passphrase: &'a str,
    link_coins: bool,
}

fn parse(body: &Value, network: NetworkMode) -> Result<Bump<'_>, PayError> {
    let invalid = |message: &str| PayError::Invalid(message.into());
    let known = [
        "feeRateSatVb",
        "maxFeeSats",
        "passphrase",
        "confirm",
        "linkCoins",
    ];
    let map = body
        .as_object()
        .ok_or_else(|| invalid("Send a JSON object."))?;
    if map.keys().any(|key| !known.contains(&key.as_str())) {
        return Err(invalid("That bump has a field this node does not know."));
    }
    if network == NetworkMode::Mainnet && body["confirm"] != true {
        return Err(invalid(
            "This node is on mainnet and pays with real bitcoin. Send \"confirm\": true to raise the fee.",
        ));
    }
    let rate = body["feeRateSatVb"]
        .as_f64()
        .filter(|rate| rate.is_finite() && *rate >= MIN_FEE_RATE && *rate <= MAX_FEE_RATE)
        .ok_or_else(|| {
            PayError::Invalid(format!(
                "Give a fee rate of {MIN_FEE_RATE} to {MAX_FEE_RATE} sat/vB."
            ))
        })?;
    let max_fee = body["maxFeeSats"]
        .as_u64()
        .ok_or_else(|| invalid("Give the most the payment may pay in fees, in sats."))?;
    let passphrase = body["passphrase"]
        .as_str()
        .ok_or_else(|| invalid("Enter the wallet passphrase to raise the fee."))?;
    let link_coins = match body.get("linkCoins") {
        None => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => return Err(invalid("linkCoins must be true or false.")),
    };
    Ok(Bump {
        rate,
        max_fee,
        passphrase,
        link_coins,
    })
}

impl Node {
    pub fn bump_payment(&self, id: &str, body: &Value) -> Result<Value, PayError> {
        let bump = parse(body, self.network)?;
        self.lock.verify(bump.passphrase)?;
        let order = self.orders().get(id).cloned().ok_or(PayError::NotFound)?;
        if order.settlement != "direct" {
            return Err(PayError::Invalid(
                "Only a direct payment can be sped up.".into(),
            ));
        }
        let original: Txid = order
            .pay_txid
            .as_deref()
            .and_then(|txid| txid.parse().ok())
            .ok_or_else(|| {
                PayError::Conflict("This node has not sent a payment for this order.".into())
            })?;
        if order.confirmations > 0
            || ![AWAITING_PAYMENT, PAYMENT_SEEN].contains(&order.status.as_str())
        {
            return Err(PayError::Conflict(format!(
                "This order is {}; its payment needs no bump.",
                order.status
            )));
        }
        let address = self.order_address(order.payment.as_ref(), "address")?;
        // A replacement of a confirmed payment double-spends and the network refuses it.
        if let Err(err) = self.sync_buyer_wallet() {
            eprintln!("ob-node: order {id}: bumping without a fresh wallet sync: {err}");
        }
        let (tx, fee) = self.sign_replacement(original, &address, &bump)?;
        self.chain.broadcast(&tx).map_err(PayError::Chain)?;
        let txid = tx.compute_txid().to_string();
        self.after_broadcast(id, tx.clone(), &txid)?;
        eprintln!(
            "ob-node: order {id}: replaced {original} with {txid} at {} sat/vB",
            bump.rate
        );
        Ok(json!({
            "orderId": id,
            "replacedTxid": original.to_string(),
            "txid": txid,
            "feeSats": fee,
            "feeRateSatVb": bump.rate,
        }))
    }

    /// The replacement of `original`, signed: the same single payment to
    /// `address`, at `bump.rate`, for at most `bump.max_fee`.
    fn sign_replacement(
        &self,
        original: Txid,
        address: &Address,
        bump: &Bump<'_>,
    ) -> Result<(Transaction, u64), PayError> {
        let locked = self.escrow.coins.outpoints();
        let mut wallet = self.buyer_wallet();
        let original_tx = wallet
            .wallet
            .get_tx(original)
            .ok_or_else(|| PayError::Conflict("This node's wallet no longer has that payment; it may be confirmed or replaced.".into()))?
            .tx_node
            .tx
            .clone();
        let paid = original_tx
            .output
            .iter()
            .find(|out| out.script_pubkey == address.script_pubkey())
            .map(|out| out.value)
            .ok_or_else(|| {
                PayError::Conflict("That transaction does not pay this order.".into())
            })?;
        let own_inputs: std::collections::HashSet<_> = original_tx
            .input
            .iter()
            .map(|input| input.previous_output)
            .collect();
        let recipient = address.script_pubkey();
        let blocked: Vec<_> = crate::coin_provenance::blocked_coins(
            &wallet.wallet,
            Some(recipient.as_script()),
            bump.link_coins,
        )
        .into_iter()
        .filter(|outpoint| !own_inputs.contains(outpoint))
        .collect();
        let rate = FeeRate::from_sat_per_kwu((bump.rate * 250.0).ceil() as u64);
        let mut builder = wallet
            .wallet
            .build_fee_bump(original)
            .map_err(|err| PayError::Conflict(format!("That payment cannot be replaced: {err}")))?;
        builder.fee_rate(rate);
        for outpoint in locked.into_iter().chain(blocked.iter().copied()) {
            builder.add_unspendable(outpoint);
        }
        let mut psbt = builder.finish().map_err(|err| {
            let mut message = format!("The node wallet cannot pay that fee: {err}");
            if !blocked.is_empty() {
                message.push(' ');
                message.push_str(crate::coin_provenance::LINK_EXPLANATION);
            }
            PayError::Invalid(message)
        })?;
        let foreign: Vec<_> = psbt
            .unsigned_tx
            .output
            .iter()
            .filter(|out| !wallet.wallet.is_mine(out.script_pubkey.clone()))
            .collect();
        if foreign.len() != 1
            || foreign[0].script_pubkey != address.script_pubkey()
            || foreign[0].value != paid
        {
            return Err(PayError::Failed(
                "a replacement must pay exactly the same order".into(),
            ));
        }
        let fee = psbt_fee(&psbt)?;
        if fee > bump.max_fee {
            return Err(PayError::Conflict(format!(
                "That fee is {fee} sats, more than the {} allowed. Nothing was sent.",
                bump.max_fee
            )));
        }
        let seed = self.spend_seed(&wallet)?;
        if !wallet
            .sign(&mut psbt, &seed)
            .map_err(|err| PayError::Failed(err.to_string()))?
        {
            return Err(PayError::Failed(
                "the replacement could not be fully signed".into(),
            ));
        }
        let tx = psbt
            .extract_tx()
            .map_err(|err| PayError::Failed(err.to_string()))?;
        wallet
            .persist()
            .map_err(|err| PayError::Failed(err.to_string()))?;
        Ok((tx, fee))
    }
}
