#![forbid(unsafe_code)]

use std::env;
use std::process::ExitCode;
use std::time::Duration;

use ob_node::{EsploraChain, NetworkMode, NodeConfig, ServeOptions};

const USAGE: &str = "ob-node [print | serve [port] [--network testnet4|mainnet] [--data-dir DIR] [--esplora URL[,URL...]] [--confirmations N] [--poll-secs S]]";

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
        eprintln!("ob-node: WARNING: this public server sees every address the node watches, and the node is not on Tor; pass --esplora with your own server to keep them private.");
    }
    let catalog = match ob_node::load_catalog(network) {
        Ok(catalog) => catalog,
        Err(err) => {
            eprintln!("ob-node: fixture catalog failed to verify: {err}");
            return ExitCode::from(1);
        }
    };
    let chain = match EsploraChain::new(&options.esplora_urls, network.bitcoin()) {
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
    };
    let node = match ob_node::Node::open_with(options.data_dir(), catalog, config) {
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
    if let Err(err) = node.serve(options.port, Duration::from_secs(options.poll_secs)) {
        eprintln!("ob-node: {err}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
