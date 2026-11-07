#![forbid(unsafe_code)]

use std::env;
use std::process::ExitCode;
use std::time::Duration;

use ob_node::{LocalConfig, NetworkMode, NodeConfig, RemoteConfig, ServeOptions, VaultChoice, VaultMode};

const USAGE: &str = "ob-node [print | --version | pair [port] [--network N] [--data-dir DIR] [--lan-page] | devices [port] [--remove ID [--rotate] | --remove-all --rotate] | serve [port] [--remote] [--lan] [--lan-listen ADDR] [--remote-listen 127.0.0.1:PORT] [--web-dir DIR] [--local-shop DIR] [--push-gateway http://ONION] [--network testnet4|mainnet] [--data-dir DIR] [--peer HOST:PORT[,...] | --esplora URL[,URL...]] [--public-fallback | --no-public-fallback] [--confirmations N] [--poll-secs S] [--escrow-vendor SELLER=URL] [--moderator URL] [--moderate] [--vault keystore|passphrase|file] [--shop-origin URL] [--no-pair-codes] [--proxy socks5h://HOST:PORT] [--no-rates] [--xmr-daemon URL[,URL...]] [--lightning [--ln-network signet|testnet4|regtest] [--ln-chain esplora:URL|electrum:URL|bitcoind:USER:PASS@HOST:PORT] [--lsp NODE_ID@HOST:PORT [--lsp-token T] | --no-lsp] [--lsp-max-fee-sats N] [--ln-rgs URL | --ln-p2p-gossip] [--ln-listen HOST:PORT]]]

Chain data comes from BIP 157/158 compact block filters by default.
--peer uses only your own node (bitcoind with blockfilterindex=1 and peerblockfilters=1).
--esplora uses exactly the Esplora servers you name.
--public-fallback asks public Esplora servers while no filter peer answers (default on testnet4 only).
--proxy also carries filter peer connections and broadcasts, and gives each Esplora query its own Tor circuit.
--xmr-daemon turns on Monero direct pay through monerod RPC: your own node (loopback or LAN, direct), or a remote node through --proxy (required for .onion, and on mainnet). There is no default node. Testnet4 pays on Monero stagenet.
--remote publishes the node's admin API as a Tor onion service that only paired devices can find (docs/home-node.md); `ob-node pair` prints a one-time pairing code and approves the device; `ob-node devices` lists and removes devices.
--local-shop serves the web shop export on the loopback listener for a browser on this machine. It does not mark that page as a remote home-node client, and it does not change who can call the onion service.
--lan adds the encrypted LAN channel on port 8790, mDNS discovery, and the first-run setup page at http://openbazaar.local:8790/.
--push-gateway sends content-free wake-ups to devices that turned push on, through that gateway over Tor. Off unless set.
--web-dir serves the web shop export to paired browsers over that onion.
--lightning runs an LDK Lightning node for direct pay (see docs/lightning.md). On testnet4 it runs on Mutinynet signet with the public Mutinynet LSP; --ln-network testnet4 or regtest needs --lsp or --no-lsp (regtest also --ln-chain). Mainnet refuses Lightning until its connections run over Tor. The LSP sees the amount and time of each payment to you; Lightning peers see your IP address (--proxy does not cover them).
--vault passphrase reads OB_VAULT_PASSPHRASE, or asks on the terminal.
--vault file keeps the key in the data dir; use it only on headless machines and in tests.";
const PASSPHRASE_ENV: &str = "OB_VAULT_PASSPHRASE";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("print") => print_catalog(),
        Some("serve") => serve(&args[1..]),
        Some("pair") => ob_node::pair_command(&args[1..]),
        Some("devices") => ob_node::devices_command(&args[1..]),
        Some("version") | Some("-V") | Some("--version") => {
            println!("ob-node {}", ob_node::BUILD.summary());
            ExitCode::SUCCESS
        }
        Some("help") | Some("-h") | Some("--help") => {
            eprintln!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("ob-node: unknown command {other}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn print_catalog() -> ExitCode {
    let json = ob_node::load_catalog(NetworkMode::Testnet4)
        .map_err(|err| err.to_string())
        .and_then(|catalog| ob_node::catalog_json(&catalog).map_err(|err| err.to_string()));
    match json {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("ob-node: {err}");
            ExitCode::from(1)
        }
    }
}

fn serve(args: &[String]) -> ExitCode {
    let (remote, args) = match RemoteConfig::extract(args) {
        Ok(split) => split,
        Err(err) => {
            eprintln!("ob-node: {err}");
            return ExitCode::from(2);
        }
    };
    let options = match ServeOptions::parse(&args) {
        Ok(options) => options,
        Err(err) => {
            eprintln!("ob-node: {err}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let network = options.network;
    let banner = match network {
        NetworkMode::Testnet4 => "TESTNET4 (test coins, no value)",
        NetworkMode::Mainnet => "MAINNET (real bitcoin)",
    };
    eprintln!("ob-node {}", ob_node::BUILD.summary());
    eprintln!("ob-node: ==== network {banner} ====");
    eprintln!("ob-node: data dir {}", options.data_dir().display());
    let plan = options.chain_plan();
    eprintln!("ob-node: chain data from {}", plan.describe());
    eprintln!("ob-node: paid after {} confirmation(s)", options.confirmations);
    if options.uses_public_server() {
        eprintln!("ob-node: WARNING: a public Esplora server sees every address it is asked about; drop --esplora/--public-fallback, or pass --peer with your own node, to keep them private.");
    }
    match &options.proxy {
        Some(proxy) => eprintln!("ob-node: rates and chain queries go through {proxy}"),
        None if options.uses_public_server() || options.live_rates => eprintln!(
            "ob-node: WARNING: rates and chain queries go over clearnet and show this machine's IP address; pass --proxy socks5h://127.0.0.1:9050 to use Tor{}.",
            if options.live_rates { ", or --no-rates to skip prices" } else { "" }
        ),
        None => {}
    }
    let xmr = options.xmr_config();
    match &xmr {
        Some(config) => eprintln!(
            "ob-node: Monero {} direct pay via {} ({}). {}",
            config.network,
            config.nodes.iter().map(|node| node.url.as_str()).collect::<Vec<_>>().join(", "),
            config.privacy(),
            config.reveals()
        ),
        None => eprintln!("ob-node: Monero is off (no --xmr-daemon)"),
    }
    let lightning = options.lightning.plan(network).expect("parse already checked the Lightning flags");
    match &lightning {
        Some(config) => eprintln!(
            "ob-node: Lightning on {} via {}, LSP {}",
            config.network,
            config.chain.describe(),
            config.lsp.as_ref().map_or("none (open channels by hand)".to_string(), |lsp| lsp.peer.to_string())
        ),
        None if network == NetworkMode::Mainnet => eprintln!("ob-node: Lightning is off on mainnet until its connections run over Tor"),
        None => eprintln!("ob-node: Lightning is off (no --lightning)"),
    }
    let vault = match vault_mode(&options) {
        Ok(vault) => vault,
        Err(err) => {
            eprintln!("ob-node: {err}");
            return ExitCode::from(2);
        }
    };
    let catalog = match ob_node::load_catalog(network) {
        Ok(catalog) => catalog,
        Err(err) => {
            eprintln!("ob-node: fixture catalog failed to verify: {err}");
            return ExitCode::from(1);
        }
    };
    let chain = match plan.build(network, &options.egress()) {
        Ok(chain) => chain,
        Err(err) => {
            eprintln!("ob-node: chain client: {err}");
            return ExitCode::from(1);
        }
    };
    let config = NodeConfig {
        network,
        confirmations: options.confirmations,
        public_server: options.uses_public_server(),
        chain,
        escrow: options.escrow_config(),
    };
    let local = LocalConfig {
        vault,
        shop_origin: options.shop_origin.clone(),
        pair_codes: options.pair_codes,
        egress: options.egress(),
        local_shop: options.local_shop.clone(),
    };
    let node = match ob_node::Node::open_local(options.data_dir(), catalog, config, local) {
        Ok(node) => node,
        Err(err) => {
            eprintln!("ob-node: {err}");
            return ExitCode::from(1);
        }
    };
    if let Some(config) = xmr {
        let chain = match openbazaar_xmr::DaemonChain::new(config) {
            Ok(chain) => chain,
            Err(err) => {
                eprintln!("ob-node: Monero client: {err}");
                return ExitCode::from(1);
            }
        };
        if let Err(err) = node.set_xmr_chain(std::sync::Arc::new(chain)) {
            eprintln!("ob-node: {err}");
            return ExitCode::from(1);
        }
    }
    if let Err(err) = node.set_remote(remote) {
        eprintln!("ob-node: {err}");
        return ExitCode::from(1);
    }
    if let Some(config) = lightning {
        if let Err(err) = node.enable_lightning(config) {
            eprintln!("ob-node: {err}");
            return ExitCode::from(1);
        }
    }
    for url in &options.follows {
        if let Err(err) = node.add_follow(url) {
            eprintln!("ob-node: --follow {url}: {err}");
            return ExitCode::from(2);
        }
    }
    // First boot only. The token file in the data dir is kept, and a
    // later start does not print it again.
    if let Some(token) = node.fresh_pairing_token() {
        eprintln!("ob-node pairing token: {token}");
    }
    if env::var_os("OB_EXIT_ON_STDIN_EOF").is_some() {
        ob_node::exit_when_stdin_closes();
    }
    if let Err(err) = node.serve(options.port, Duration::from_secs(options.poll_secs)) {
        eprintln!("ob-node: {err}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn vault_mode(options: &ServeOptions) -> Result<VaultMode, String> {
    match options.vault {
        VaultChoice::Keystore => Ok(VaultMode::Keystore),
        VaultChoice::File => {
            eprintln!("ob-node: WARNING: --vault file keeps the vault key beside the files it encrypts.");
            Ok(VaultMode::File)
        }
        VaultChoice::Passphrase => {
            if let Ok(passphrase) = env::var(PASSPHRASE_ENV) {
                return Ok(VaultMode::Passphrase(passphrase));
            }
            let new = !options.data_dir().join("vault.lock").exists();
            let read = |prompt: &str| rpassword::prompt_password(prompt).map_err(|err| format!("the passphrase could not be read: {err}"));
            let passphrase = read("Vault passphrase: ")?;
            if new && read("Again: ")? != passphrase {
                return Err("the passphrases differ".into());
            }
            Ok(VaultMode::Passphrase(passphrase))
        }
    }
}
