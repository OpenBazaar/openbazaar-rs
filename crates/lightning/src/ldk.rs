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

/// The TCP address LDK binds, and the onion it may announce.
///
/// LDK's start binds every `listening_addresses` entry with `TcpListener`
/// after `ToSocketAddrs`. `SocketAddress::OnionV3` returns
/// "Resolution of OnionV3 addresses is currently unsupported", so the onion
/// cannot be that list. The bind is loopback or LAN. The announcement, when
/// present, is an onion and nothing else.
pub(crate) fn inbound_sockets(listen: &str, announce: Option<&str>) -> Result<(SocketAddress, Option<SocketAddress>), LnError> {
    let bind = socket(listen)?;
    if bind_is_public(&bind) {
        return Err(LnError::Invalid(format!(
            "Lightning will not bind {listen}: that is a public clearnet address"
        )));
    }
    let announce = match announce {
        Some(address) => Some(onion_announcement(address)?),
        None => None,
    };
    Ok((bind, announce))
}

fn bind_is_public(address: &SocketAddress) -> bool {
    match address {
        SocketAddress::TcpIpV4 { addr, port } => {
            if *port == 0 {
                return true;
            }
            let ip = std::net::Ipv4Addr::from(*addr);
            !(ip.is_loopback() || ip.is_private() || ip.is_link_local())
        }
        SocketAddress::TcpIpV6 { addr, port } => {
            if *port == 0 {
                return true;
            }
            let ip = std::net::Ipv6Addr::from(*addr);
            let ula = (ip.segments()[0] & 0xfe00) == 0xfc00;
            let link_local = (ip.segments()[0] & 0xffc0) == 0xfe80;
            !(ip.is_loopback() || ula || link_local)
        }
        SocketAddress::Hostname { hostname, port } => *port == 0 || !crate::config::host_is_local(hostname.as_str()),
        SocketAddress::OnionV2(_) | SocketAddress::OnionV3 { .. } => true,
    }
}

fn onion_announcement(address: &str) -> Result<SocketAddress, LnError> {
    match SocketAddress::from_str(address) {
        Ok(onion @ SocketAddress::OnionV3 { port, .. }) if port != 0 => Ok(onion),
        _ => Err(LnError::Invalid(format!(
            "Lightning will not announce {address}: a peer would be told a clearnet address"
        ))),
    }
}

fn payment_id(hash: &str) -> Option<PaymentId> {
    let bytes: [u8; 32] = hex::decode(hash).ok()?.try_into().ok()?;
    Some(PaymentId(bytes))
}

impl LdkRail {
    /// Build and start the LDK node. Returns once it runs; chain sync goes on in the background.
    pub fn start(config: LnConfig, secret: &[u8; 32]) -> Result<Self, LnError> {
        if let Some(peer) = config.dials_in_the_clear() {
            return Err(LnError::Invalid(format!(
                "Lightning is not started: connecting to {peer} in the clear would show this machine's IP"
            )));
        }
        std::fs::create_dir_all(&config.storage_dir).map_err(|err| LnError::Failed(err.to_string()))?;
        let mut ldk = Config::default();
        ldk.network = config.network;
        ldk.storage_dir_path = config.storage_dir.to_string_lossy().into_owned();
        ldk.socks_proxy = config.proxy.clone();
        let mut announce_onion = false;
        if let Some(listen) = &config.listen {
            let (bind, announce) = inbound_sockets(listen, config.announce.as_deref())?;
            ldk.listening_addresses = Some(vec![bind]);
            if let Some(onion) = announce {
                ldk.announcement_addresses = Some(vec![onion]);
                announce_onion = true;
            }
        }
        if let (Some(lsp), Some(anchors)) = (&config.lsp, ldk.anchor_channels_config.as_mut()) {
            // The LSP's just-in-time channel arrives before this node has
            // on-chain funds for an anchor reserve.
            anchors.trusted_peers_no_reserve.push(lsp.peer.node_id);
        }
        let mut builder = Builder::from_config(ldk);
        if announce_onion {
            // Gossip runs only when an alias is set. The addresses it broadcasts
            // are `announcement_addresses` (the onion), not the loopback bind.
            builder
                .set_node_alias("openbazaar".to_string())
                .map_err(|err| LnError::Failed(format!("Lightning alias: {err}")))?;
        }
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

#[cfg(test)]
mod tests {
    use super::{inbound_sockets, LdkRail};
    use crate::rail::LightningRail;
    use crate::{ChainSource, GossipSource, LnConfig, LspConfig};
    use ldk_node::lightning::ln::msgs::SocketAddress;

    #[test]
    fn a_public_lightning_peer_is_not_dialed() {
        let dir = std::env::temp_dir().join(format!("ob-ln-clear-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = LnConfig {
            network: ldk_node::bitcoin::Network::Signet,
            storage_dir: dir,
            chain: ChainSource::Esplora("https://mutinynet.com/api".into()),
            gossip: GossipSource::Rgs("https://rgs.mutinynet.com/snapshot".into()),
            lsp: Some(LspConfig {
                peer: "0371d6fd7d75de2d0372d03ea00e8bacdacb50c27d0eaea0a76a0622eff1f5ef2b@44.219.111.31:39735".parse().unwrap(),
                token: Some("JZWN9YLW".into()),
            }),
            listen: None,
            announce: None,
            max_lsp_fee_bps: 500,
            min_lsp_fee_cap_sats: 10_000,
            proxy: None,
        };
        let started = std::time::Instant::now();
        match LdkRail::start(config, &[0x5a; 32]) {
            Err(err) => assert!(err.to_string().contains("in the clear"), "{err}"),
            Ok(rail) => {
                rail.stop();
                panic!("a public Lightning peer was started");
            }
        }
        assert!(started.elapsed() < std::time::Duration::from_millis(500), "start dialed the public peer");
    }

    /// The dial LDK uses for a public peer. It fails unless the only TCP connection is to the proxy
    /// and the proxy is told the peer's name (so this machine does not resolve it).
    #[test]
    fn a_public_lightning_peer_is_dialed_through_the_proxy() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{mpsc, Arc, Mutex};
        use std::thread;
        use std::time::Duration;

        let seen = Arc::new(Mutex::new(None));
        let record = Arc::clone(&seen);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("socks5h://{}", listener.local_addr().unwrap());
        thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut greet = [0u8; 3];
            socket.read_exact(&mut greet).unwrap();
            assert_eq!(greet, [0x05, 0x01, 0x00], "the greeting is not a SOCKS5 handshake");
            socket.write_all(&[0x05, 0x00]).unwrap();
            let mut hdr = [0u8; 4];
            socket.read_exact(&mut hdr).unwrap();
            assert_eq!(&hdr[..3], &[0x05, 0x01, 0x00]);
            let dest = match hdr[3] {
                0x01 => {
                    let mut ip = [0u8; 4];
                    socket.read_exact(&mut ip).unwrap();
                    std::net::Ipv4Addr::from(ip).to_string()
                }
                0x03 => {
                    let mut len = [0u8; 1];
                    socket.read_exact(&mut len).unwrap();
                    let mut name = vec![0u8; len[0] as usize];
                    socket.read_exact(&mut name).unwrap();
                    String::from_utf8(name).unwrap()
                }
                other => panic!("public peer address type {other} is not a name or an IPv4"),
            };
            let mut port = [0u8; 2];
            socket.read_exact(&mut port).unwrap();
            *record.lock().unwrap() = Some(format!("{dest}:{}", u16::from_be_bytes(port)));
            socket.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).unwrap();
            let mut hold = [0u8; 1];
            let _ = socket.read(&mut hold);
        });

        let (tx, rx) = mpsc::channel();
        let proxy_for_dial = proxy.clone();
        thread::spawn(move || {
            let opened = ldk_node::connect_via_socks(&proxy_for_dial, "public-peer.invalid", 9735);
            let _ = tx.send(opened.map(|stream| stream.peer_addr().ok()));
        });
        let peer = rx.recv_timeout(Duration::from_secs(2)).unwrap_or_else(|_| {
            panic!("a public Lightning peer was dialed in the clear");
        });
        let peer = peer.expect("the proxy did not accept the peer").expect("the dial has no peer");
        assert!(peer.ip().is_loopback(), "the TCP connection went to {peer}, not the proxy");
        assert_eq!(seen.lock().unwrap().as_deref(), Some("public-peer.invalid:9735"));
    }

    /// LDK binds `listening_addresses` and, once an alias is set, gossips
    /// `announcement_addresses` or else those bind addresses. A public IP in
    /// either list fails this test.
    #[test]
    fn lightning_does_not_bind_or_announce_a_public_address() {
        const ONION: &str = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid.onion:9735";
        let (bind, announced) = inbound_sockets("127.0.0.1:9735", Some(ONION)).expect("loopback bind and onion announcement");
        let announced = announced.expect("the onion is announced");
        assert!(
            matches!(bind, SocketAddress::TcpIpV4 { addr: [127, 0, 0, 1], port: 9735 }),
            "bound {bind}"
        );
        assert!(matches!(announced, SocketAddress::OnionV3 { port: 9735, .. }), "announced {announced}");
        let announced = announced.to_string();
        assert!(announced.ends_with(".onion:9735"), "{announced}");
        assert!(!announced.contains("127.0.0.1"), "{announced}");
        for (listen, announce) in [
            ("1.2.3.4:9735", None),
            ("0.0.0.0:9735", Some(ONION)),
            ("8.8.8.8:9735", Some(ONION)),
            ("203.0.113.9:9735", Some(ONION)),
            ("ln.example.com:9735", Some(ONION)),
            ("127.0.0.1:0", Some(ONION)),
            (ONION, None),
            ("127.0.0.1:9735", Some("1.2.3.4:9735")),
            ("127.0.0.1:9735", Some("0.0.0.0:9735")),
            ("127.0.0.1:9735", Some("127.0.0.1:9735")),
            ("[::]:9735", Some(ONION)),
        ] {
            let err = inbound_sockets(listen, announce).expect_err(listen).to_string();
            assert!(
                err.contains("public clearnet") || err.contains("clearnet address"),
                "{listen} / {announce:?} was accepted or said: {err}"
            );
        }
        let (lan, announced) = inbound_sockets("192.168.1.9:9735", Some(ONION)).expect("a LAN socket is not a public bind");
        assert!(matches!(lan, SocketAddress::TcpIpV4 { addr: [192, 168, 1, 9], .. }));
        assert!(matches!(announced, Some(SocketAddress::OnionV3 { .. })));
        let (only_bind, announced) = inbound_sockets("127.0.0.1:9736", None).expect("loopback without an announcement");
        assert!(matches!(only_bind, SocketAddress::TcpIpV4 { addr: [127, 0, 0, 1], port: 9736 }));
        assert!(announced.is_none(), "a loopback bind is not announced");
        let (local_name, announced) = inbound_sockets("localhost:9735", None).expect("localhost is a loopback bind");
        assert!(matches!(local_name, SocketAddress::Hostname { .. }));
        assert!(announced.is_none(), "localhost is not announced");
    }
}