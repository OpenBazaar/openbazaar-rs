//! `ob-node pair` and `ob-node devices`: remote access from the node's console.
//!
//! Run them on the node's machine (over SSH, or with `docker compose exec`).
//! They read `pairing.token` from the data dir and ask the running node on
//! loopback, so they need the same user and the same `--data-dir`,
//! `--network`, and port as `serve`.

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use qrcode::render::svg;
use qrcode::render::unicode::Dense1x2;
use qrcode::QrCode;
use serde_json::{json, Value};

use super::pairing::CODE_LIFETIME;
use super::protocol::words;
use super::Remote;
use crate::ServeOptions;

const PAIR_USAGE: &str =
    "usage: ob-node pair [port] [--network testnet4|mainnet] [--data-dir DIR] [--lan-page]";
const DEVICES_USAGE: &str = "usage: ob-node devices [port] [--network …] [--data-dir DIR] [--remove ID [--rotate] | --remove-all --rotate]";

pub fn pair_command(args: &[String]) -> ExitCode {
    let (flags, rest) = take_flags(args, &["--lan-page"]);
    let node = match LocalNode::open(&rest, PAIR_USAGE) {
        Ok(node) => node,
        Err(code) => return code,
    };
    if flags.contains(&"--lan-page") {
        if let Err(message) = node.call("POST", "/remote/lan-page", json!({})) {
            eprintln!("ob-node: {message}");
            return ExitCode::from(1);
        }
    }
    let offer = match node.call("POST", "/remote/pairing", json!({})) {
        Ok(offer) => offer,
        Err(message) => {
            eprintln!("ob-node: {message}");
            return ExitCode::from(1);
        }
    };
    let (Some(link), Some(code_words)) = (offer["link"].as_str(), offer["words"].as_array()) else {
        eprintln!("ob-node: the node sent no code");
        return ExitCode::from(1);
    };
    let code_words: Vec<&str> = code_words.iter().filter_map(Value::as_str).collect();
    print_code(link, &code_words.join(" "), "Pair a device with this code");
    if !std::io::stdin().is_terminal() {
        eprintln!("ob-node: approve the device in the app on a paired device, or run `ob-node pair` in a terminal to approve it here.");
        return ExitCode::SUCCESS;
    }
    approve_here(&node)
}

/// Wait for pairing requests and ask about each one on the terminal.
fn approve_here(node: &LocalNode) -> ExitCode {
    eprintln!("ob-node: waiting for the device… (Ctrl-C to stop)");
    let until = Instant::now() + CODE_LIFETIME;
    let mut asked: Vec<String> = Vec::new();
    while Instant::now() < until {
        let pending = node
            .call("GET", "/remote/pending", Value::Null)
            .unwrap_or(Value::Null);
        for request in pending["pending"].as_array().into_iter().flatten() {
            let Some(id) = request["requestId"]
                .as_str()
                .filter(|id| !asked.iter().any(|asked| asked == id))
            else {
                continue;
            };
            asked.push(id.to_string());
            let sas = request["sas"]
                .as_array()
                .map(|sas| {
                    sas.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            let names = request["sasNames"]
                .as_array()
                .map(|names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            eprint!(
                "\n{} ({}) wants to pair.\nIts screen should show:  {sas}   ({names})\nApprove only if it matches. Approve? [y/N] ",
                request["name"].as_str().unwrap_or("A device"),
                request["platform"].as_str().unwrap_or("?"),
            );
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            let _ = std::io::stdin().lock().read_line(&mut line);
            let approve = matches!(line.trim(), "y" | "Y" | "yes");
            match node.call(
                "POST",
                &format!("/remote/pending/{id}"),
                json!({ "approve": approve }),
            ) {
                Ok(_) if approve => {
                    eprintln!("ob-node: paired.");
                    return ExitCode::SUCCESS;
                }
                Ok(_) => eprintln!("ob-node: rejected."),
                Err(message) => eprintln!("ob-node: {message}"),
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    eprintln!("ob-node: the code expired. Run `ob-node pair` for another.");
    ExitCode::from(1)
}

pub fn devices_command(args: &[String]) -> ExitCode {
    let (flags, rest) = take_flags(args, &["--remove-all", "--rotate"]);
    let (remove, rest) = take_value(&rest, "--remove");
    let node = match LocalNode::open(&rest, DEVICES_USAGE) {
        Ok(node) => node,
        Err(code) => return code,
    };
    let rotate = flags.contains(&"--rotate");
    let result = if flags.contains(&"--remove-all") {
        if !rotate {
            eprintln!("ob-node: --remove-all also needs --rotate, so a lost device cannot find the node again.");
            return ExitCode::from(2);
        }
        remove_all(&node)
    } else if let Some(id) = remove {
        node.call(
            "DELETE",
            &format!("/remote/devices/{id}"),
            json!({ "rotate": rotate }),
        )
        .map(|answer| {
            eprintln!(
                "ob-node: removed {}",
                answer["removed"]
                    .as_array()
                    .map(|ids| ids
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            );
            answer
        })
    } else {
        node.call("GET", "/remote/devices", Value::Null)
    };
    match result {
        Ok(answer) => {
            print_devices(&answer);
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("ob-node: {message}");
            ExitCode::from(1)
        }
    }
}

fn remove_all(node: &LocalNode) -> Result<Value, String> {
    let list = node.call("GET", "/remote/devices", Value::Null)?;
    for device in list["devices"].as_array().into_iter().flatten() {
        if let Some(id) = device["id"].as_str() {
            let _ = node.call("DELETE", &format!("/remote/devices/{id}"), json!({}));
        }
    }
    let moved = node.call("POST", "/remote/rotate", json!({}))?;
    eprintln!(
        "ob-node: every device is removed and the node is moving to {}",
        moved["nextOnion"].as_str().unwrap_or("a new address")
    );
    eprintln!("ob-node: run `ob-node pair` to pair your devices again.");
    node.call("GET", "/remote/devices", Value::Null)
}

fn print_devices(answer: &Value) {
    let list = answer["devices"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        println!("No devices are paired. Run `ob-node pair`.");
        return;
    }
    println!(
        "{:<18} {:<24} {:<9} {:<6} last seen",
        "id", "name", "platform", "via"
    );
    for device in list {
        let seen = device["lastSeen"]
            .as_u64()
            .filter(|seen| *seen > 0)
            .map_or("never".to_string(), |seen| {
                format!("{}s ago", super::now_secs().saturating_sub(seen))
            });
        println!(
            "{:<18} {:<24} {:<9} {:<6} {seen}",
            device["id"].as_str().unwrap_or("?"),
            device["name"].as_str().unwrap_or("?"),
            device["platform"].as_str().unwrap_or("?"),
            device["lastVia"].as_str().unwrap_or("-"),
        );
    }
}

/// The running node, reached on loopback with its pairing token.
struct LocalNode {
    port: u16,
    token: String,
}

impl LocalNode {
    fn open(args: &[String], usage: &str) -> Result<Self, ExitCode> {
        let options = ServeOptions::parse(args).map_err(|err| {
            eprintln!("ob-node: {err}\n{usage}");
            ExitCode::from(2)
        })?;
        let token =
            std::fs::read_to_string(options.data_dir().join("pairing.token")).map_err(|err| {
                eprintln!(
                    "ob-node: no pairing token in {}: {err}. Is this the node's data dir?",
                    options.data_dir().display()
                );
                ExitCode::from(1)
            })?;
        Ok(Self {
            port: options.port,
            token: token.trim().to_string(),
        })
    }

    fn call(&self, method: &str, path: &str, body: Value) -> Result<Value, String> {
        let url = format!("http://127.0.0.1:{}{path}", self.port);
        let request = ureq::request(method, &url).set("X-Ob-Pair", &self.token);
        let answer = if body.is_null() {
            request.call()
        } else {
            request
                .set("Content-Type", "application/json")
                .send_string(&body.to_string())
        };
        let read = |response: ureq::Response| {
            response
                .into_string()
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .unwrap_or(Value::Null)
        };
        match answer {
            Ok(response) => Ok(read(response)),
            Err(ureq::Error::Status(_, response)) => Err(read(response)["error"]
                .as_str()
                .unwrap_or("the node refused")
                .to_string()),
            Err(err) => Err(format!(
                "the node at 127.0.0.1:{} could not be reached: {err}",
                self.port
            )),
        }
    }
}

fn take_flags<'a>(args: &[String], names: &[&'a str]) -> (Vec<&'a str>, Vec<String>) {
    let found = names
        .iter()
        .copied()
        .filter(|name| args.iter().any(|arg| arg == name))
        .collect();
    let rest = args
        .iter()
        .filter(|arg| !names.contains(&arg.as_str()))
        .cloned()
        .collect();
    (found, rest)
}

fn take_value(args: &[String], name: &str) -> (Option<String>, Vec<String>) {
    match args.iter().position(|arg| arg == name) {
        Some(at) if at + 1 < args.len() => {
            let mut rest = args.to_vec();
            let value = rest.remove(at + 1);
            rest.remove(at);
            (Some(value), rest)
        }
        _ => (None, args.to_vec()),
    }
}

/// The pairing link as an SVG QR code, for the setup page and the app.
pub(crate) fn qr_svg(link: &str) -> String {
    QrCode::new(link.as_bytes())
        .map(|qr| {
            qr.render::<svg::Color<'_>>()
                .min_dimensions(240, 240)
                .quiet_zone(true)
                .build()
        })
        .unwrap_or_default()
}

const FIRST_RUN_LEAD: &str = "No device is paired yet. Pair your first device with this code";
const FIRST_RUN_NO_TERMINAL: &str = "ob-node: No device is paired yet. A pairing code is a secret, so it is shown only on a terminal and is not written to this log. Run `ob-node pair` in a terminal on the node's machine, with the same port, --network, and --data-dir as `serve`: it shows a code and asks you to approve the device.";

/// What a node with no paired device says on stderr when it starts.
///
/// On a terminal it makes a code and shows the words, the link, and its QR.
/// Stderr that is not a terminal is a log that others may read and keep (a
/// service manager's, a hosting platform's): no code is made, and the log
/// says how to get one. `None` when a code could not be made.
pub(crate) fn first_run_notice(remote: &Remote, terminal: bool) -> Option<String> {
    if !terminal {
        return Some(FIRST_RUN_NO_TERMINAL.to_string());
    }
    let code = remote.issue_code(None)?;
    Some(code_text(
        &remote.link_for(&code),
        &words(&code.secret).join(" "),
        FIRST_RUN_LEAD,
    ))
}

fn print_code(link: &str, code_words: &str, lead: &str) {
    eprintln!("{}", code_text(link, code_words, lead));
}

fn code_text(link: &str, code_words: &str, lead: &str) -> String {
    let qr = QrCode::new(link.as_bytes())
        .map(|qr| qr.render::<Dense1x2>().quiet_zone(true).build())
        .unwrap_or_default();
    format!(
        "ob-node: {lead} (one use, valid {} minutes):\n\n    {code_words}\n\n{qr}\n    {link}\n\n\
         ob-node: In the OpenBazaar app choose Connect to my home node, then scan the QR or type the words.\n\
         ob-node: Approve the device here, in the app on a paired device, or on the setup page on your home network.",
        CODE_LIFETIME.as_secs() / 60
    )
}

pub(crate) fn announce_request(name: &str, platform: &str, sas: &[&str], names: &[&str]) {
    eprintln!(
        "ob-node: {name:?} ({platform}) asks to pair. Its screen shows {} ({}). Approve it with `ob-node pair` or in the app.",
        sas.join(" "),
        names.join(", ")
    );
}
