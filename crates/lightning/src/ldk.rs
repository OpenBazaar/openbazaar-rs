//! [`LightningRail`] over an embedded `ldk_node::Node`.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ldk_node::bitcoin::secp256k1::PublicKey;
use ldk_node::bitcoin::Address;
use ldk_node::config::Config;
use ldk_node::lightning::ln::channelmanager::PaymentId;
use ldk_node::lightning::ln::msgs::SocketAddress;
use ldk_node::lightning::routing::router::RouteParametersConfig;
use ldk_node::lightning_invoice::{Bolt11Invoice, Bolt11InvoiceDescription, Description};
use ldk_node::logger::{LogLevel, LogRecord, LogWriter};
use ldk_node::payment::{PaymentDirection, PaymentKind, PaymentStatus};
use ldk_node::{Builder, Event, Node, NodeError, UserChannelId};

use crate::config::{ChainSource, GossipSource, LnConfig, PeerAddress};
use crate::keys::{ldk_entropy, store_key};
use crate::rail::{Balances, Channel, Incoming, IssuedInvoice, LightningRail, LnError, NodeInfo, Outgoing};
use crate::sealed_store::SealedStore;

/// Warnings and errors to stderr, each text at most once per [`REPEAT`];
/// LDK's debug log names payments and peers, so it is never written to disk.
#[derive(Default)]
struct StderrLog {
    seen: Mutex<HashMap<String, Instant>>,
}

const REPEAT: Duration = Duration::from_secs(600);

impl LogWriter for StderrLog {
    fn log(&self, record: LogRecord) {
        if record.level < LogLevel::Warn {
            return;
        }
        let text = record.args.to_string();
        let mut seen = self.seen.lock().unwrap_or_else(|err| err.into_inner());
        let now = Instant::now();
        seen.retain(|_, at| now.duration_since(*at) < REPEAT);
        if seen.insert(text.clone(), now).is_none() {
            eprintln!("ob-node: lightning: {text}");
        }
    }
}

pub struct LdkRail {
    node: Arc<Node>,
    config: LnConfig,
    stop: Arc<AtomicBool>,
    events: Mutex<Option<JoinHandle<()>>>,
}

fn failed(err: NodeError) -> LnError {
    match err {
        NodeError::NotRunning => LnError::NotRunning,
        NodeError::InsufficientFunds => LnError::Insufficient("Not enough funds for that.".into()),
        NodeError::LiquiditySourceUnavailable | NodeError::LiquidityRequestFailed | NodeError::LiquidityFeeTooHigh => {
            LnError::Lsp(format!("The LSP could not help: {err}."))
        }
        NodeError::InvalidInvoice | NodeError::InvalidAmount | NodeError::InvalidAddress | NodeError::DuplicatePayment => {
            LnError::Invalid(format!("{err}."))
        }
        other => LnError::Failed(format!("{other}.")),
    }
}

fn socket(address: &str) -> Result<SocketAddress, LnError> {
    SocketAddress::from_str(address).map_err(|_| LnError::Invalid(format!("{address} is not HOST:PORT.")))
}

fn payment_id(hash: &str) -> Option<PaymentId> {
    let bytes: [u8; 32] = hex::decode(hash).ok()?.try_into().ok()?;
    Some(PaymentId(bytes))
}

impl LdkRail {
    /// Build and start the LDK node. Returns once it runs; chain sync goes on in the background.
    pub fn start(config: LnConfig, secret: &[u8; 32]) -> Result<Self, LnError> {
        std::fs::create_dir_all(&config.storage_dir).map_err(|err| LnError::Failed(err.to_string()))?;
        let mut ldk = Config::default();
        ldk.network = config.network;
        ldk.storage_dir_path = config.storage_dir.to_string_lossy().into_owned();
        if let Some(listen) = &config.listen {
            ldk.listening_addresses = Some(vec![socket(listen)?]);
        }
        if let (Some(lsp), Some(anchors)) = (&config.lsp, ldk.anchor_channels_config.as_mut()) {
            // The LSP's just-in-time channel arrives before this node has
            // on-chain funds for an anchor reserve.
            anchors.trusted_peers_no_reserve.push(lsp.peer.node_id);
        }
        let mut builder = Builder::from_config(ldk);
        builder.set_entropy_seed_bytes(*ldk_entropy(secret));
        builder.set_custom_logger(Arc::new(StderrLog::default()));
        match &config.chain {
            ChainSource::Esplora(url) => builder.set_chain_source_esplora(url.clone(), None),
            ChainSource::Electrum(url) => builder.set_chain_source_electrum(url.clone(), None),
            ChainSource::BitcoindRpc { host, port, user, password } => {
                builder.set_chain_source_bitcoind_rpc(host.clone(), *port, user.clone(), password.clone())
            }
        };
        match &config.gossip {
            GossipSource::P2p => builder.set_gossip_source_p2p(),
            GossipSource::Rgs(url) => builder.set_gossip_source_rgs(url.clone()),
        };
        if let Some(lsp) = &config.lsp {
            builder.set_liquidity_source_lsps2(lsp.peer.node_id, socket(&lsp.peer.address)?, lsp.token.clone());
        }
        let store = Arc::new(SealedStore::new(config.storage_dir.join("sealed"), &store_key(secret)));
        let node = Arc::new(builder.build_with_store(store).map_err(|err| LnError::Failed(format!("LDK node: {err}")))?);
        node.start().map_err(failed)?;
        let stop = Arc::new(AtomicBool::new(false));
        let events = std::thread::Builder::new()
            .name("ob-lightning-events".into())
            .spawn({
                let node = Arc::clone(&node);
                let stop = Arc::clone(&stop);
                move || drain_events(&node, &stop)
            })
            .map_err(|err| LnError::Failed(err.to_string()))?;
        Ok(Self { node, config, stop, events: Mutex::new(Some(events)) })
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    fn channel(&self, id: &str) -> Result<ldk_node::ChannelDetails, LnError> {
        let wanted: u128 = id.parse().map_err(|_| LnError::Invalid("That is not a channel id.".into()))?;
        self.node
            .list_channels()
            .into_iter()
            .find(|channel| channel.user_channel_id == UserChannelId(wanted))
            .ok_or_else(|| LnError::Invalid("No channel with that id.".into()))
    }
}

/// LDK queues events until they are handled. Payments and channels are read
/// from LDK's own stores, so this only logs what matters and moves on.
fn drain_events(node: &Node, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let Some(event) = node.next_event() else {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        };
        match &event {
            Event::ChannelReady { counterparty_node_id, .. } => {
                eprintln!("ob-node: lightning: channel ready with {}", counterparty_node_id.map(|id| id.to_string()).unwrap_or_default());
            }
            Event::ChannelClosed { reason, .. } => eprintln!("ob-node: lightning: channel closed ({reason:?})"),
            Event::PaymentFailed { reason, .. } => eprintln!("ob-node: lightning: payment failed ({reason:?})"),
            _ => {}
        }
        if node.event_handled().is_err() {
            return;
        }
    }
}

impl LightningRail for LdkRail {
    fn info(&self) -> NodeInfo {
        let status = self.node.status();
        let lsp = self.config.lsp.as_ref().map(|lsp| {
            let connected = self.node.list_peers().iter().any(|peer| peer.node_id == lsp.peer.node_id && peer.is_connected);
            (lsp.peer.node_id.to_string(), connected)
        });
        NodeInfo {
            node_id: self.node.node_id().to_string(),
            network: self.config.network,
            running: status.is_running,
            lsp,
            best_height: status.current_best_block.height,
            last_wallet_sync: status.latest_onchain_wallet_sync_timestamp,
            last_lightning_sync: status.latest_lightning_wallet_sync_timestamp,
        }
    }

    fn balances(&self) -> Balances {
        let onchain = self.node.list_balances();
        let channels = self.node.list_channels();
        let usable = channels.iter().filter(|channel| channel.is_usable);
        Balances {
            lightning_sats: usable.clone().map(|channel| channel.outbound_capacity_msat).sum::<u64>() / 1_000,
            inbound_sats: usable.map(|channel| channel.inbound_capacity_msat).sum::<u64>() / 1_000,
            onchain_total_sats: onchain.total_onchain_balance_sats,
            onchain_spendable_sats: onchain.spendable_onchain_balance_sats,
        }
    }

    fn channels(&self) -> Vec<Channel> {
        self.node
            .list_channels()
            .into_iter()
            .map(|channel| Channel {
                id: channel.user_channel_id.0.to_string(),
                peer: channel.counterparty_node_id.to_string(),
                capacity_sats: channel.channel_value_sats,
                outbound_msat: channel.outbound_capacity_msat,
                inbound_msat: channel.inbound_capacity_msat,
                ready: channel.is_channel_ready,
                usable: channel.is_usable,
                funding_txid: channel.funding_txo.map(|outpoint| outpoint.txid.to_string()),
            })
            .collect()
    }

    fn receive(&self, amount_msat: u64, description: &str, expiry_secs: u32) -> Result<IssuedInvoice, LnError> {
        let description = Description::new(description.to_string()).map_err(|_| LnError::Invalid("The invoice description is too long.".into()))?;
        let description = Bolt11InvoiceDescription::Direct(description);
        let room = self
            .node
            .list_channels()
            .iter()
            .filter(|channel| channel.is_usable)
            .map(|channel| channel.inbound_capacity_msat)
            .max()
            .unwrap_or(0);
        let payments = self.node.bolt11_payment();
        let (invoice, jit, cap): (Bolt11Invoice, bool, Option<u64>) = if room >= amount_msat {
            (payments.receive(amount_msat, &description, expiry_secs).map_err(failed)?, false, None)
        } else if self.config.lsp.is_some() {
            let cap = self.config.lsp_fee_cap_msat(amount_msat);
            (payments.receive_via_jit_channel(amount_msat, &description, expiry_secs, Some(cap)).map_err(failed)?, true, Some(cap))
        } else {
            return Err(LnError::Insufficient(
                "No channel can take this payment and no LSP is set. Open a channel first.".into(),
            ));
        };
        let expires_at = invoice.duration_since_epoch().as_secs().saturating_add(invoice.expiry_time().as_secs());
        Ok(IssuedInvoice {
            bolt11: invoice.to_string(),
            payment_hash: invoice.payment_hash().to_string(),
            amount_msat,
            expires_at,
            jit,
            lsp_fee_cap_msat: cap,
        })
    }

    fn incoming(&self, payment_hash: &str) -> Incoming {
        let Some(details) = payment_id(payment_hash).and_then(|id| self.node.payment(&id)) else {
            return Incoming::Unknown;
        };
        if details.direction != PaymentDirection::Inbound {
            return Incoming::Unknown;
        }
        match details.status {
            PaymentStatus::Pending => Incoming::Pending,
            PaymentStatus::Failed => Incoming::Failed,
            PaymentStatus::Succeeded => Incoming::Paid {
                received_msat: details.amount_msat.unwrap_or(0),
                lsp_fee_msat: match details.kind {
                    PaymentKind::Bolt11Jit { counterparty_skimmed_fee_msat, .. } => counterparty_skimmed_fee_msat.unwrap_or(0),
                    _ => 0,
                },
                at: details.latest_update_timestamp,
            },
        }
    }

    fn pay(&self, bolt11: &str, max_fee_msat: u64) -> Result<String, LnError> {
        let invoice = Bolt11Invoice::from_str(bolt11.trim()).map_err(|_| LnError::Invalid("That is not a BOLT 11 invoice.".into()))?;
        if invoice.amount_milli_satoshis().is_none() {
            return Err(LnError::Invalid("That invoice has no amount.".into()));
        }
        let route = RouteParametersConfig { max_total_routing_fee_msat: Some(max_fee_msat), ..RouteParametersConfig::default() };
        let id = self.node.bolt11_payment().send(&invoice, Some(route)).map_err(failed)?;
        Ok(hex::encode(id.0))
    }

    fn outgoing(&self, payment_hash: &str) -> Outgoing {
        let Some(details) = payment_id(payment_hash).and_then(|id| self.node.payment(&id)) else {
            return Outgoing::Unknown;
        };
        if details.direction != PaymentDirection::Outbound {
            return Outgoing::Unknown;
        }
        match details.status {
            PaymentStatus::Pending => Outgoing::Pending,
            PaymentStatus::Failed => Outgoing::Failed,
            PaymentStatus::Succeeded => {
                Outgoing::Paid { fee_msat: details.fee_paid_msat.unwrap_or(0), at: details.latest_update_timestamp }
            }
        }
    }

    fn onchain_address(&self) -> Result<String, LnError> {
        self.node.onchain_payment().new_address().map(|address| address.to_string()).map_err(failed)
    }

    fn sweep_onchain(&self, address: &str) -> Result<String, LnError> {
        let address = Address::from_str(address)
            .map_err(|_| LnError::Invalid("That is not a bitcoin address.".into()))?
            .require_network(self.config.network)
            .map_err(|_| LnError::Invalid("That address is for another network.".into()))?;
        let txid = self.node.onchain_payment().send_all_to_address(&address, true, None).map_err(failed)?;
        Ok(txid.to_string())
    }

    fn open_channel(&self, peer: &PeerAddress, sats: u64) -> Result<String, LnError> {
        let id = self.node.open_channel(peer.node_id, socket(&peer.address)?, sats, None, None).map_err(failed)?;
        Ok(id.0.to_string())
    }

    fn close_channel(&self, id: &str, force: bool) -> Result<(), LnError> {
        let channel = self.channel(id)?;
        let peer: PublicKey = channel.counterparty_node_id;
        if force {
            self.node.force_close_channel(&channel.user_channel_id, peer, None).map_err(failed)
        } else {
            self.node.close_channel(&channel.user_channel_id, peer).map_err(failed)
        }
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.events.lock().unwrap_or_else(|err| err.into_inner()).take() {
            let _ = handle.join();
        }
        let _ = self.node.stop();
    }
}

impl Drop for LdkRail {
    fn drop(&mut self) {
        self.stop();
    }
}