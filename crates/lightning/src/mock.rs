//! An in-memory [`LightningRail`] for offline tests.
//!
//! Invoices are real signed BOLT 11 strings, so [`crate::decode_invoice`]
//! reads them. Mocks made with [`MockRail::linked`] share one ledger: paying
//! an invoice one of them issued settles it on the issuer. `inbound_sats`
//! starts at zero, so every invoice is a just-in-time one while an LSP is
//! set; [`MockRail::set_lsp_fee`] chooses what the "LSP" keeps.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ldk_node::bitcoin::hashes::{sha256, Hash};
use ldk_node::bitcoin::secp256k1::{Secp256k1, SecretKey};
use ldk_node::bitcoin::Network;
use ldk_node::lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use rand::RngCore;

use crate::config::PeerAddress;
use crate::rail::{decode_invoice, Balances, Channel, Incoming, IssuedInvoice, LightningRail, LnError, NodeInfo, Outgoing};

#[derive(Default)]
struct Ledger {
    /// payment hash -> (issuer id, amount, state)
    invoices: HashMap<String, (usize, u64, Incoming)>,
    next_id: usize,
}

struct State {
    outgoing: HashMap<String, Outgoing>,
    lightning_sats: u64,
    lsp_fee_msat: u64,
    lsp: bool,
    running: bool,
    fail_pay: Option<LnError>,
    channels: Vec<Channel>,
}

pub struct MockRail {
    id: usize,
    key: SecretKey,
    network: Network,
    ledger: Arc<Mutex<Ledger>>,
    state: Mutex<State>,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

impl MockRail {
    pub fn new(network: Network) -> Self {
        Self::on(network, Arc::default())
    }

    /// Another mock on the same ledger as `self`.
    pub fn linked(&self) -> Self {
        Self::on(self.network, Arc::clone(&self.ledger))
    }

    fn on(network: Network, ledger: Arc<Mutex<Ledger>>) -> Self {
        let id = {
            let mut ledger = guard(&ledger);
            ledger.next_id += 1;
            ledger.next_id
        };
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        Self {
            id,
            key: SecretKey::from_slice(&bytes).expect("32 random bytes are a key"),
            network,
            ledger,
            state: Mutex::new(State {
                outgoing: HashMap::new(),
                lightning_sats: 0,
                lsp_fee_msat: 0,
                lsp: true,
                running: true,
                fail_pay: None,
                channels: Vec::new(),
            }),
        }
    }

    pub fn set_lightning_sats(&self, sats: u64) {
        guard(&self.state).lightning_sats = sats;
    }

    pub fn set_lsp_fee(&self, msat: u64) {
        guard(&self.state).lsp_fee_msat = msat;
    }

    pub fn set_lsp(&self, on: bool) {
        guard(&self.state).lsp = on;
    }

    pub fn fail_next_pay(&self, err: LnError) {
        guard(&self.state).fail_pay = Some(err);
    }

    /// Settle an invoice this mock issued, as if an outside wallet paid it.
    pub fn settle(&self, payment_hash: &str) -> bool {
        let fee = guard(&self.state).lsp_fee_msat;
        let mut ledger = guard(&self.ledger);
        match ledger.invoices.get_mut(payment_hash) {
            Some((issuer, amount, state)) if *issuer == self.id && *state == Incoming::Pending => {
                *state = Incoming::Paid { received_msat: amount.saturating_sub(fee), lsp_fee_msat: fee, at: now() };
                true
            }
            _ => false,
        }
    }

    fn currency(&self) -> Currency {
        match self.network {
            Network::Bitcoin => Currency::Bitcoin,
            Network::Signet => Currency::Signet,
            Network::Regtest => Currency::Regtest,
            _ => Currency::BitcoinTestnet,
        }
    }
}

impl LightningRail for MockRail {
    fn info(&self) -> NodeInfo {
        let state = guard(&self.state);
        NodeInfo {
            node_id: ldk_node::bitcoin::secp256k1::PublicKey::from_secret_key(&Secp256k1::new(), &self.key).to_string(),
            network: self.network,
            running: state.running,
            lsp: state.lsp.then(|| ("02".to_string() + &"11".repeat(32), true)),
            best_height: 100,
            last_wallet_sync: Some(now()),
            last_lightning_sync: Some(now()),
        }
    }

    fn balances(&self) -> Balances {
        Balances { lightning_sats: guard(&self.state).lightning_sats, ..Balances::default() }
    }

    fn channels(&self) -> Vec<Channel> {
        guard(&self.state).channels.clone()
    }

    fn receive(&self, amount_msat: u64, description: &str, expiry_secs: u32) -> Result<IssuedInvoice, LnError> {
        let lsp = guard(&self.state).lsp;
        if !lsp {
            return Err(LnError::Insufficient("No channel can take this payment and no LSP is set. Open a channel first.".into()));
        }
        let mut preimage = [0u8; 32];
        rand::rng().fill_bytes(&mut preimage);
        let hash = sha256::Hash::hash(&preimage);
        let secp = Secp256k1::new();
        let invoice = InvoiceBuilder::new(self.currency())
            .description(description.to_string())
            .payment_hash(hash)
            .payment_secret(PaymentSecret(preimage))
            .amount_milli_satoshis(amount_msat)
            .current_timestamp()
            .expiry_time(Duration::from_secs(u64::from(expiry_secs)))
            .min_final_cltv_expiry_delta(144)
            .build_signed(|message| secp.sign_ecdsa_recoverable(message, &self.key))
            .map_err(|err| LnError::Failed(format!("{err:?}")))?;
        let payment_hash = hash.to_string();
        guard(&self.ledger).invoices.insert(payment_hash.clone(), (self.id, amount_msat, Incoming::Pending));
        Ok(IssuedInvoice {
            bolt11: invoice.to_string(),
            payment_hash,
            amount_msat,
            expires_at: now() + u64::from(expiry_secs),
            jit: true,
            lsp_fee_cap_msat: Some(amount_msat / 20),
        })
    }

    fn incoming(&self, payment_hash: &str) -> Incoming {
        match guard(&self.ledger).invoices.get(payment_hash) {
            Some((issuer, _, state)) if *issuer == self.id => state.clone(),
            _ => Incoming::Unknown,
        }
    }

    fn pay(&self, bolt11: &str, _max_fee_msat: u64) -> Result<String, LnError> {
        let info = decode_invoice(bolt11, self.network)?;
        let amount = info.amount_msat.ok_or_else(|| LnError::Invalid("That invoice has no amount.".into()))?;
        let mut state = guard(&self.state);
        if let Some(err) = state.fail_pay.take() {
            return Err(err);
        }
        if state.lightning_sats.saturating_mul(1_000) < amount {
            return Err(LnError::Insufficient("Not enough funds for that.".into()));
        }
        state.lightning_sats -= amount.div_ceil(1_000);
        state.outgoing.insert(info.payment_hash.clone(), Outgoing::Paid { fee_msat: 0, at: now() });
        drop(state);
        let mut ledger = guard(&self.ledger);
        if let Some((issuer, _, paid)) = ledger.invoices.get_mut(&info.payment_hash) {
            if *issuer != self.id && *paid == Incoming::Pending {
                *paid = Incoming::Paid { received_msat: amount, lsp_fee_msat: 0, at: now() };
            }
        }
        Ok(info.payment_hash)
    }

    fn outgoing(&self, payment_hash: &str) -> Outgoing {
        guard(&self.state).outgoing.get(payment_hash).cloned().unwrap_or(Outgoing::Unknown)
    }

    fn onchain_address(&self) -> Result<String, LnError> {
        Ok("bcrt1qmockaddress".into())
    }

    fn sweep_onchain(&self, _address: &str) -> Result<String, LnError> {
        Ok("00".repeat(32))
    }

    fn open_channel(&self, peer: &PeerAddress, sats: u64) -> Result<String, LnError> {
        let mut state = guard(&self.state);
        let id = (state.channels.len() + 1).to_string();
        state.channels.push(Channel {
            id: id.clone(),
            peer: peer.node_id.to_string(),
            capacity_sats: sats,
            outbound_msat: sats * 1_000,
            inbound_msat: 0,
            ready: false,
            usable: false,
            funding_txid: None,
        });
        Ok(id)
    }

    fn close_channel(&self, id: &str, _force: bool) -> Result<(), LnError> {
        let mut state = guard(&self.state);
        let before = state.channels.len();
        state.channels.retain(|channel| channel.id != id);
        if state.channels.len() == before {
            return Err(LnError::Invalid("No channel with that id.".into()));
        }
        Ok(())
    }

    fn stop(&self) {
        guard(&self.state).running = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_mocks_pay_each_other_with_real_invoices() {
        let seller = MockRail::new(Network::Signet);
        let buyer = seller.linked();
        buyer.set_lightning_sats(10_000);
        seller.set_lsp_fee(1_000);
        let invoice = seller.receive(5_000_000, "order 1", 600).unwrap();
        let info = decode_invoice(&invoice.bolt11, Network::Signet).unwrap();
        assert_eq!(info.amount_msat, Some(5_000_000));
        assert_eq!(info.payment_hash, invoice.payment_hash);
        assert_eq!(info.description, "order 1");
        assert!(decode_invoice(&invoice.bolt11, Network::Bitcoin).is_err());
        assert_eq!(seller.incoming(&invoice.payment_hash), Incoming::Pending);
        assert_eq!(buyer.pay(&invoice.bolt11, 1_000).unwrap(), invoice.payment_hash);
        assert!(matches!(seller.incoming(&invoice.payment_hash), Incoming::Paid { received_msat: 5_000_000, .. }));
        assert!(matches!(buyer.outgoing(&invoice.payment_hash), Outgoing::Paid { .. }));
        assert_eq!(buyer.balances().lightning_sats, 5_000);

        let second = seller.receive(1_000_000, "order 2", 600).unwrap();
        assert!(seller.settle(&second.payment_hash));
        assert_eq!(seller.incoming(&second.payment_hash), Incoming::Paid { received_msat: 999_000, lsp_fee_msat: 1_000, at: now() });
    }
}
