//! The web gateway: one https address, access keys, no pairing.

use super::*;
use crate::remote::protocol::base32;
use crate::remote::RemoteConfig;

pub(super) const HOST: &str = "node.example.com";
pub(super) const HTTPS: &str = "X-Forwarded-Proto: https\r\n";

pub(super) fn flags(args: &[&str]) -> Result<RemoteConfig, String> {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    RemoteConfig::extract(&args).map(|(config, _)| config)
}

pub(super) fn web_home() -> (TempDir, Node) {
    let (dir, node) = home();
    let origin = format!("https://{HOST}");
    node.set_remote(flags(&["--web-listen", "0.0.0.0:8080", "--web-origin", &origin]).unwrap())
        .unwrap();
    (dir, node)
}

/// A browser holding the access key the node was started with.
pub(super) fn browser(node: &Node) -> Device {
    let secret = random_bytes::<32>();
    node.allow_web_key(&format!("obkey-{}", base32(&secret))).unwrap();
    Device {
        key: Key::Bip340(Signer::from_secret(&secret).unwrap()),
        watch: None,
        node_id: node.remote.node_id(),
    }
}

/// `raw` as the host's proxy hands it to the web gateway, with `headers` added.
pub(super) fn on_web(node: &Node, raw: &str, headers: &str) -> (String, Value) {
    let raw = raw.replacen(&format!("Host: {GATEWAY}\r\n"), &format!("Host: {HOST}\r\n{headers}"), 1);
    send(node, Entry::Web, &raw)
}

#[test]
fn the_web_flags_go_together_and_the_address_is_https() {
    assert!(flags(&["--web-listen", "0.0.0.0:8080"]).is_err());
    assert!(flags(&["--web-origin", "https://node.example.com"]).is_err());
    assert!(flags(&["--web-listen", "0.0.0.0:8080", "--web-origin", "http://node.example.com"]).is_err());
    assert!(flags(&["--web-listen", "0.0.0.0:8080", "--web-origin", "node.example.com"]).is_err());
    assert!(flags(&["--web-listen", "127.0.0.1:8080", "--web-origin", "http://127.0.0.1:8080"]).unwrap().web.is_some());
    assert!(flags(&[]).unwrap().web.is_none());
}

#[test]
fn an_access_key_signs_in_on_the_web_address() {
    let (_dir, node) = web_home();
    let browser = browser(&node);
    let signed = |method: &str, path: &str| browser.raw(method, path, "", now_secs());

    let (status, body) = on_web(&node, &signed("GET", "/remote/devices"), HTTPS);
    assert!(status.contains("200"), "{status} {body}");
    assert_eq!(body["devices"][0]["name"], "Web access key");
    let (_, body) = on_web(&node, &signed("GET", "/remote/devices"), HTTPS);
    assert_eq!(body["devices"][0]["lastVia"], "web");

    // Starting the node with the same key again changes nothing.
    let devices = node.remote.devices().list().len();
    assert!(node.allow_web_key("obkey-short").is_err());
    assert_eq!(node.remote.devices().list().len(), devices);

    let (status, _) = on_web(&node, &unsigned("GET", "/remote/hello", "", ""), HTTPS);
    assert!(status.contains("200"), "{status}");
    let stranger = Device {
        key: Key::bip340(),
        watch: None,
        node_id: node.remote.node_id(),
    };
    let (status, _) = on_web(&node, &stranger.raw("GET", "/remote/devices", "", now_secs()), HTTPS);
    assert!(status.contains("401"), "{status}");
}

#[test]
fn the_web_address_refuses_plain_http_other_hosts_other_pages_and_pairing() {
    let (_dir, node) = web_home();
    let browser = browser(&node);
    let signed = |method: &str, path: &str| browser.raw(method, path, "", now_secs());

    let plain = signed("GET", "/remote/devices?x=1").replacen(&format!("Host: {GATEWAY}"), &format!("Host: {HOST}"), 1);
    let answer = String::from_utf8(gateway::respond(&node, plain.as_bytes(), Entry::Web, MAIN_PORT)).unwrap();
    assert!(answer.starts_with("HTTP/1.1 308 "), "{answer}");
    assert!(answer.contains(&format!("Location: https://{HOST}/remote/devices?x=1\r\n")), "{answer}");
    let (status, _) = on_web(&node, &signed("POST", "/lock"), "");
    assert!(status.contains("400"), "{status}");

    let (status, _) = send(&node, Entry::Web, &signed("GET", "/remote/devices"));
    assert!(status.contains("421"), "{status}");
    let (status, _) = on_web(&node, &signed("GET", "/remote/devices"), &format!("{HTTPS}Origin: https://other.example.com\r\n"));
    assert!(status.contains("403"), "{status}");
    let (status, _) = on_web(&node, &signed("GET", "/remote/devices"), &format!("{HTTPS}Origin: https://{HOST}\r\n"));
    assert!(status.contains("200"), "{status}");

    // A pairing code is not tried here: the request is only an unsigned one.
    let code = node.remote.issue_code(None).unwrap();
    let ask = Ask::phone("Phone").body(&code.secret).to_string();
    let (status, body) = on_web(&node, &unsigned("POST", "/pair/request", "Content-Type: application/json\r\n", &ask), HTTPS);
    assert!(status.contains("401"), "{status} {body}");
    assert_eq!(body["code"], "unsigned");
}
