#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use ob_node::{LocalConfig, NetworkMode, NodeConfig, RemoteConfig, ServeOptions, VaultChoice, VaultMode};

const USAGE: &str = "ob-node [print | --version | pair [port] [--network N] [--data-dir DIR] [--lan-page] | devices [port] [--remove ID [--rotate] | --remove-all --rotate] | serve [port] [--remote] [--lan] [--lan-listen ADDR] [--remote-listen 127.0.0.1:PORT] [--web-dir DIR] [--web-listen ADDR --web-origin https://HOST] [--local-shop DIR] [--push-gateway http://ONION] [--network testnet4|mainnet] [--data-dir DIR] [--peer HOST:PORT[,...] | --esplora URL[,URL...]] [--public-fallback | --no-public-fallback] [--confirmations N] [--poll-secs S] [--escrow-vendor SELLER=URL] [--moderator URL] [--moderate] [--vault keystore|passphrase|file] [--shop-origin URL] [--no-pair-codes] [--proxy socks5h://HOST:PORT] [--no-rates] [--fixed-rates BTCUSD=N,ZECUSD=N] [--no-introductions] [--xmr-daemon URL[,URL...]] [--zec-indexer URL[,URL...]] [--onion] [--lightning [--ln-network signet|testnet4|regtest] [--ln-chain esplora:URL|electrum:URL|bitcoind:USER:PASS@HOST:PORT] [--lsp NODE_ID@HOST:PORT [--lsp-token T] | --no-lsp] [--lsp-max-fee-sats N] [--ln-rgs URL | --ln-p2p-gossip] [--ln-listen HOST:PORT]] | send-letter --to ONION|http://127.0.0.1:PORT --file FILE [--data-dir DIR] [--network testnet4|mainnet] [--timeout SECS] | poll-mail [--data-dir DIR] [--network testnet4|mainnet] [--expect SHA256] | sign-seed --key FILE --sequence N [--introductions ONION,ONION] --out FILE | fetch-seed --data-dir DIR --from ONION [--from ONION...]]

Chain data comes from BIP 157/158 compact block filters by default.
--peer uses only your own node (bitcoind with blockfilterindex=1 and peerblockfilters=1).
--esplora uses exactly the Esplora servers you name.
--public-fallback asks public Esplora servers for the tip, fees, and broadcasts while no filter peer answers (default on testnet4 only), and only through --proxy. Addresses are never sent. Without --proxy those servers are not contacted.
--proxy carries filter peer connections, broadcasts, Esplora queries, exchange rates, and Lightning peers, chain sync, and gossip, and gives each Esplora query its own Tor circuit. Without it, public peers and rate servers are not contacted, and Lightning stays on this machine or the LAN.
--fixed-rates prices orders from the dollar figures you give for 1 BTC and 1 ZEC, in whole dollars, and asks no rate server, as with --no-rates. Monero has no price. It is for tests on testnet4 and is refused on mainnet.
--no-introductions does not ask the introductions for the directory, and does not open the Tor client that reaches them. The node learns no stores or listings from the directory, does not announce its own store there, and pays no directory fee. A store followed by its address still works.
--xmr-daemon turns on Monero direct pay through monerod RPC: your own node (loopback or LAN, direct), or a remote node through --proxy (required for .onion and for any public daemon). There is no default node. Testnet4 pays on Monero stagenet.
--zec-indexer turns on Zcash direct pay from compact blocks: your own Zebra and Zaino (loopback or LAN, direct), or a remote Zaino or lightwalletd through --proxy (required for .onion and for any public server). http:// is plaintext gRPC (port 9067 if none is given), https:// is gRPC over TLS. There is no default server. The server must speak lightwallet-protocol 0.5.0 or later, which carries Ironwood; an older one is refused. Testnet4 pays on Zcash testnet.
--remote publishes the node's admin API as a Tor onion service that only paired devices can find (docs/home-node.md), and a separate public onion for the store. A buyer follows http://….onion; the catalog, order mail, reviews, and escrow mail go over Tor and a redirect off that onion is not followed. An order or escrow reply address is an onion for that one conversation, not the store onion. A follow of http://127.0.0.1 still works on this machine. --escrow-vendor and --moderator take that onion or a loopback URL. On testnet4, without --moderator, a dispute goes to the test network's moderator. `ob-node pair` prints a one-time pairing code and approves the device; `ob-node devices` lists and removes devices.
--local-shop serves the web shop export on the loopback listener for a browser on this machine. It does not mark that page as a remote home-node client, and it does not change who can call the onion service.
--lan adds the encrypted LAN channel on port 8790, mDNS discovery, and the first-run setup page at http://openbazaar.local:8790/.
--push-gateway sends content-free wake-ups to devices that turned push on, through that gateway over Tor. Off unless set.
--web-dir serves the web shop export to paired browsers over that onion.
--web-listen with --web-origin also serves it, and the admin API, to a browser on the open web at that one https address, for a node on a host with no screen. The node speaks plain HTTP on --web-listen, so run it only behind a proxy that ends TLS and sets X-Forwarded-Proto; that proxy sees what the admin onion hides. A browser signs in with a passkey, or with an access key: one a paired device made, or OB_WEB_ACCESS_KEY (obkey- and 52 letters). A new node, with no device, no passkey, and no wallet, belongs to the first browser that opens that address and creates a passkey there; set OB_WEB_ACCESS_KEY before the first start to close that, and to get back in if every passkey is lost. Pairing is not offered on that address.
--lightning runs an LDK Lightning node for direct pay (see docs/lightning.md). A public peer, Esplora chain source, or gossip server is reached through --proxy, so it does not see this machine's IP. Without --proxy those stay refused. Electrum and bitcoind stay direct. With --remote, inbound Lightning is an onion on that Tor client, forwarded to a loopback socket that is not announced. A public --ln-listen is refused. Mainnet Lightning requires --proxy and an LSP you name (or --no-lsp). There is no mainnet LSP default.
--vault passphrase reads OB_VAULT_PASSPHRASE, or asks on the terminal.
--vault file keeps the key in the data dir; use it only on headless machines and in tests.
--onion listens for one sealed letter on a Tor onion (virtual port 80) and serves the public shop routes on that same onion: GET /public/store, public media, and reviews. It also answers a seed-document query. The clearnet listener stays on 127.0.0.1. Wallet, escrow spend, and mail secrets stay closed. A clearnet sale URL is still refused.
send-letter dials --to. An onion uses Arti and fails if the dial fails or times out. A loopback http:// URL posts the letter to /escrow/mail. Any other URL is refused.
poll-mail lists id and sha256 for each letter in the sealed mailbox. With --expect, it exits 1 when that digest is absent.
sign-seed --key FILE --sequence N [--introductions ONION,ONION] --out FILE signs a directory head with the operator key. The key file is 64 hex characters and must match the public key in this binary. The head names the introduction list and the 20,000 sat fee. It does not name stores. A store joins by paying that fee. Copy the output file onto an introduction, or submit it to one that is already running; do not copy the key.
fetch-seed --data-dir DIR --from ONION [--from ONION...] asks those introductions and prints the sequence that verified.";
const PASSPHRASE_ENV: &str = "OB_VAULT_PASSPHRASE";
const WEB_KEY_ENV: &str = "OB_WEB_ACCESS_KEY";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("print") => print_catalog(),
        Some("serve") => serve(&args[1..]),
        Some("send-letter") => send_letter(&args[1..]),
        Some("poll-mail") => poll_mail(&args[1..]),
        Some("pair") => ob_node::pair_command(&args[1..]),
        Some("devices") => ob_node::devices_command(&args[1..]),
        Some("sign-seed") => sign_seed(&args[1..]),
        Some("fetch-seed") => fetch_seed(&args[1..]),
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
        eprintln!("ob-node: WARNING: a public Esplora server is not told which addresses this wallet follows, and it is not contacted without --proxy. Pass --proxy, or --peer with your own node.");
    }
    match &options.proxy {
        Some(proxy) => eprintln!("ob-node: rates and chain queries go through {proxy}"),
        None if options.uses_public_server() || options.live_rates => eprintln!(
            "ob-node: rates and public chain servers are not contacted without --proxy, so they do not see this machine's IP. Pass --proxy socks5h://127.0.0.1:9050 to use Tor{}.",
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
    let zec = options.zec_config();
    match &zec {
        Some(config) => eprintln!(
            "ob-node: Zcash {} direct pay via {} ({}). {}",
            config.network,
            config.servers.iter().map(|server| server.url.as_str()).collect::<Vec<_>>().join(", "),
            config.privacy(),
            config.reveals()
        ),
        None => eprintln!("ob-node: Zcash is off (no --zec-indexer)"),
    }
    let lightning = options.lightning.plan(network, options.proxy.as_deref()).expect("parse already checked the Lightning flags");
    match &lightning {
        Some(config) => eprintln!(
            "ob-node: Lightning on {} via {}, LSP {}",
            config.network,
            config.chain.describe(),
            config.lsp.as_ref().map_or("none (open channels by hand)".to_string(), |lsp| lsp.peer.to_string())
        ),
        None if network == NetworkMode::Mainnet => eprintln!("ob-node: Lightning is off on mainnet (it needs --proxy and an LSP you name; there is no default)"),
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
        introductions: options.introductions,
        local_shop: options.local_shop.clone(),
    };
    let node = match ob_node::Node::open_local(options.data_dir(), catalog, config, local) {
        Ok(node) => node,
        Err(err) => {
            eprintln!("ob-node: {err}");
            return ExitCode::from(1);
        }
    };
    if let Some((btc_usd, zec_usd)) = options.fixed_rates {
        node.set_fixed_rates(btc_usd, zec_usd);
    }
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
    if let Some(config) = zec {
        let chain = match openbazaar_zec::IndexerChain::new(config) {
            Ok(chain) => chain,
            Err(err) => {
                eprintln!("ob-node: Zcash client: {err}");
                return ExitCode::from(1);
            }
        };
        if let Err(err) = node.set_zec_chain(std::sync::Arc::new(chain)) {
            eprintln!("ob-node: {err}");
            return ExitCode::from(1);
        }
    }
    let web = remote.web.is_some();
    if let Err(err) = node.set_remote(remote) {
        eprintln!("ob-node: {err}");
        return ExitCode::from(1);
    }
    if let Some(key) = env::var(WEB_KEY_ENV).ok().filter(|_| web) {
        if let Err(err) = node.allow_web_key(&key) {
            eprintln!("ob-node: {WEB_KEY_ENV}: {err}");
            return ExitCode::from(1);
        }
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
    // First boot only: where the token file is. The token is not printed.
    if let Some(notice) = node.fresh_pairing_token_notice() {
        eprintln!("{notice}");
    }
    if options.onion {
        node.listen_onion(options.data_dir().join("tor"));
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

fn send_letter(args: &[String]) -> ExitCode {
    match ob_node::SendLetter::parse(args) {
        Ok(command) => match command.run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("ob-node: {err}");
                ExitCode::from(1)
            }
        },
        Err(err) => {
            eprintln!("ob-node: {err}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn poll_mail(args: &[String]) -> ExitCode {
    match ob_node::PollMail::parse(args) {
        Ok(command) => match command.run() {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::from(1),
            Err(err) => {
                eprintln!("ob-node: {err}");
                ExitCode::from(1)
            }
        },
        Err(err) => {
            eprintln!("ob-node: {err}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn fetch_seed(args: &[String]) -> ExitCode {
    let mut root = PathBuf::from("data");
    let mut onions = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let value = args.get(index + 1).map(String::as_str);
        match args[index].as_str() {
            "--data-dir" => root = PathBuf::from(value.unwrap_or("")),
            "--from" => onions.push(value.unwrap_or("").to_string()),
            other => {
                eprintln!("ob-node: unknown fetch-seed argument {other}");
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
        if value.is_none() {
            eprintln!("ob-node: {0} needs a value", args[index]);
            return ExitCode::from(2);
        }
        index += 2;
    }
    if onions.is_empty() || onions.iter().any(|onion| onion.is_empty()) {
        eprintln!("ob-node: fetch-seed needs --from ONION");
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let dir = NetworkMode::Testnet4.data_dir(&root);
    match ob_node::pull_seed(&dir, &onions) {
        Ok(sequence) => {
            println!("{sequence}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("ob-node: {err}");
            ExitCode::from(1)
        }
    }
}

fn sign_seed(args: &[String]) -> ExitCode {
    let mut key_path = None;
    let mut sequence = None;
    let mut introductions = Vec::new();
    let mut out = None;
    let mut index = 0;
    while index < args.len() {
        let value = args.get(index + 1).map(String::as_str);
        match args[index].as_str() {
            "--key" => key_path = value.map(str::to_string),
            "--sequence" => sequence = value.and_then(|text| text.parse::<u64>().ok()),
            "--introductions" => {
                introductions = value.unwrap_or("").split(',').map(str::trim).filter(|onion| !onion.is_empty()).map(str::to_string).collect();
            }
            "--stores" | "--inbox" => {
                eprintln!("ob-node: sign-seed signs a head. A store joins by paying the fee, not by being named here.");
                return ExitCode::from(2);
            }
            "--out" => out = value.map(str::to_string),
            other => {
                eprintln!("ob-node: unknown sign-seed argument {other}");
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
        index += 2;
    }
    let (Some(key_path), Some(sequence), Some(out)) = (key_path, sequence, out) else {
        eprintln!("ob-node: sign-seed needs --key, --sequence, and --out");
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let key_text = match fs::read_to_string(&key_path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("ob-node: {err}");
            return ExitCode::from(1);
        }
    };
    let secret = match unhex32(key_text.trim()) {
        Some(bytes) => bytes,
        None => {
            eprintln!("ob-node: the key file is 64 hex characters");
            return ExitCode::from(2);
        }
    };
    match ob_node::sign_document(sequence, introductions, secret) {
        Ok(bytes) => match fs::write(&out, bytes) {
            Ok(()) => {
                println!("wrote {out}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("ob-node: {err}");
                ExitCode::from(1)
            }
        },
        Err(err) => {
            eprintln!("ob-node: {err}");
            ExitCode::from(1)
        }
    }
}

fn unhex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
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
