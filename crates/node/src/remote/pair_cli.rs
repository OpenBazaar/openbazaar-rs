//! `ob-node pair`: print a one-time pairing code from the running node.
//!
//! Run it on the node's machine (over SSH, or with `docker compose exec`).
//! It reads `pairing.token` from the data dir and asks the node on loopback,
//! so it needs the same user and the same `--data-dir`, `--network`, and
//! port as `serve`.

use std::process::ExitCode;

use qrcode::render::unicode::Dense1x2;
use qrcode::QrCode;
use serde_json::Value;

use crate::ServeOptions;

pub fn pair_command(args: &[String]) -> ExitCode {
    let options = match ServeOptions::parse(args) {
        Ok(options) => options,
        Err(err) => {
            eprintln!("ob-node: {err}");
            eprintln!("usage: ob-node pair [port] [--network testnet4|mainnet] [--data-dir DIR]");
            return ExitCode::from(2);
        }
    };
    let token = match std::fs::read_to_string(options.data_dir().join("pairing.token")) {
        Ok(token) => token.trim().to_string(),
        Err(err) => {
            eprintln!("ob-node: no pairing token in {}: {err}. Is this the node's data dir?", options.data_dir().display());
            return ExitCode::from(1);
        }
    };
    let url = format!("http://127.0.0.1:{}/remote/pairing", options.port);
    let answer = ureq::post(&url).set("X-Ob-Pair", &token).set("Content-Type", "application/json").send_string("{}");
    let body: Value = match answer {
        Ok(response) => response.into_string().ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null),
        Err(ureq::Error::Status(_, response)) => {
            let body: Value = response.into_string().ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null);
            eprintln!("ob-node: {}", body["error"].as_str().unwrap_or("the node refused"));
            return ExitCode::from(1);
        }
        Err(err) => {
            eprintln!("ob-node: the node at 127.0.0.1:{} could not be reached: {err}", options.port);
            return ExitCode::from(1);
        }
    };
    match body["code"].as_str() {
        Some(code) => {
            announce(code, "Pair a device with this code");
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("ob-node: the node sent no code");
            ExitCode::from(1)
        }
    }
}

/// Print a code and its QR on stderr (the service log).
pub(crate) fn announce(code: &str, lead: &str) {
    let qr = QrCode::new(code.as_bytes())
        .map(|qr| qr.render::<Dense1x2>().quiet_zone(true).build())
        .unwrap_or_default();
    eprintln!(
        "ob-node: {lead} (one use, valid 15 minutes):\n\n    {code}\n\n{qr}\n\
         ob-node: In the OpenBazaar app: Settings > Home node > Connect, and paste the code.\n\
         ob-node: In Tor Browser: open the onion address above and paste the code.\n\
         ob-node: Anyone with this code can pair once. Run `ob-node pair` for another."
    );
}
