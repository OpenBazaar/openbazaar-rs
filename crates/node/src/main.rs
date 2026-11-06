#![forbid(unsafe_code)]

use std::env;
use std::process::ExitCode;
use std::time::Duration;

use ob_node::{EsploraChain, LocalConfig, NetworkMode, NodeConfig, ServeOptions, VaultChoice, VaultMode};

const USAGE: &str = "ob-node [print | serve [port] [--network testnet4|mainnet] [--data-dir DIR] [--esplora URL[,URL...]] [--confirmations N] [--poll-secs S] [--escrow-vendor SELLER=URL] [--moderator URL] [--moderate] [--vault keystore|passphrase|file] [--shop-origin URL] [--no-pair-codes] [--proxy socks5h://HOST:PORT] [--no-rates]]

--vault passphrase reads OB_VAULT_PASSPHRASE, or asks on the terminal.
--vault file keeps the key in the data dir; use it only on headless machines and in tests.";
const PASSPHRASE_ENV: &str = "OB_VAULT_PASSPHRASE";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("print") => print_catalog(),
        Some("serve") => serve(&args[1..]),
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
    let options = match ServeOptions::parse(args) {
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
    eprintln!("ob-node: ==== network {banner} ====");
    eprintln!("ob-node: data dir {}", options.data_dir().display());
    eprintln!("ob-node: chain servers {}", options.esplora_urls.join(", "));
    eprintln!("ob-node: paid after {} confirmation(s)", options.confirmations);
    if network == NetworkMode::Mainnet && options.uses_public_server() {
        eprintln!("ob-node: WARNING: this public server sees every address the node watches; pass --esplora with your own server to keep them private.");
    }
    match &options.proxy {
        Some(proxy) => eprintln!("ob-node: rates and chain queries go through {proxy}"),
        None if options.uses_public_server() || options.live_rates => eprintln!(
            "ob-node: WARNING: rates and chain queries go over clearnet and show this machine's IP address; pass --proxy socks5h://127.0.0.1:9050 to use Tor{}.",
            if options.live_rates { ", or --no-rates to skip prices" } else { "" }
        ),
        None => {}
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
    let chain = match EsploraChain::with_egress(&options.esplora_urls, network.bitcoin(), &options.egress()) {
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
        chain: Box::new(chain),
        escrow: options.escrow_config(),
    };
    let local = LocalConfig {
        vault,
        shop_origin: options.shop_origin.clone(),
        pair_codes: options.pair_codes,
        egress: options.egress(),
    };
    let node = match ob_node::Node::open_local(options.data_dir(), catalog, config, local) {
        Ok(node) => node,
        Err(err) => {
            eprintln!("ob-node: {err}");
            return ExitCode::from(1);
        }
    };
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
