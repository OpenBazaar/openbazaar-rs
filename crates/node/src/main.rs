#![forbid(unsafe_code)]

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let command = args.next();
    let catalog = match ob_node::load_catalog() {
        Ok(catalog) => catalog,
        Err(err) => {
            eprintln!("ob-node: fixture catalog failed to verify: {err}");
            return ExitCode::from(1);
        }
    };
    let json = match ob_node::catalog_json(&catalog) {
        Ok(json) => json,
        Err(err) => {
            eprintln!("ob-node: {err}");
            return ExitCode::from(1);
        }
    };

    match command.as_deref() {
        None | Some("print") => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Some("serve") => {
            let port = match args.next() {
                None => 8787,
                Some(text) => match text.parse::<u16>() {
                    Ok(port) => port,
                    Err(_) => {
                        eprintln!("ob-node: port must be a number");
                        return ExitCode::from(2);
                    }
                },
            };
            if let Err(err) = ob_node::serve(&json, port) {
                eprintln!("ob-node: {err}");
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Some("help") | Some("-h") | Some("--help") => {
            eprintln!("ob-node [print|serve [port]]");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("ob-node: unknown command {other}");
            eprintln!("ob-node [print|serve [port]]");
            ExitCode::from(2)
        }
    }
}
