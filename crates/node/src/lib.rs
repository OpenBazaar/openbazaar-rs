//! Loads the signed fixture catalog and projects the listing headers the UI searches.

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpListener;

use openbazaar_protocol::{fixture_catalog, Catalog, Error};
use serde_json::{json, Value};

pub fn load_catalog() -> Result<Catalog, Error> {
    let catalog = fixture_catalog();
    catalog.verify()?;
    Ok(catalog)
}

pub fn catalog_value(catalog: &Catalog) -> Result<Value, Error> {
    let mut listings = Vec::with_capacity(catalog.listings.len());
    for listing in &catalog.listings {
        listings.push(json!({
            "hash": listing.content_hash()?.to_hex(),
            "storeId": listing.store_id.to_hex(),
            "title": listing.title,
            "category": listing.category,
            "priceSats": listing.price_sats,
            "shipFrom": listing.ship_from,
            "kind": listing.kind.as_str(),
        }));
    }
    Ok(json!({
        "store": {
            "id": catalog.store.id.to_hex(),
            "name": catalog.store.name,
            "region": catalog.store.region,
        },
        "listings": listings,
    }))
}

pub fn catalog_json(catalog: &Catalog) -> Result<String, Error> {
    let value = catalog_value(catalog)?;
    Ok(serde_json::to_string_pretty(&value).expect("catalog json is finite"))
}

pub fn http_response(status: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        body.len()
    );
    let mut response = header.into_bytes();
    response.extend_from_slice(body);
    response
}

/// Serves the fixture catalog on the loopback interface. This is not a relay.
pub fn serve(json: &str, port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    eprintln!("ob-node catalog at http://127.0.0.1:{port}/catalog");
    for connection in listener.incoming() {
        let mut stream = connection?;
        let mut buf = [0u8; 2048];
        let read = stream.read(&mut buf).unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..read]);
        let line = request.lines().next().unwrap_or("");
        let found = line.starts_with("GET /catalog ") || line.starts_with("GET / ");
        let (status, content_type, body): (&str, &str, &[u8]) = if found {
            ("200 OK", "application/json", json.as_bytes())
        } else {
            ("404 Not Found", "text/plain", b"not found")
        };
        stream.write_all(&http_response(status, content_type, body))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn fixture_catalog_verifies() {
        let catalog = load_catalog().unwrap();
        assert_eq!(catalog.store.name, "Marlow Goods");
        assert!(catalog
            .listings
            .iter()
            .any(|listing| listing.kind.as_str() == "physical"));
        assert!(catalog
            .listings
            .iter()
            .any(|listing| listing.kind.as_str() == "digital"));
    }

    #[test]
    fn shared_ui_catalog_matches_signed_records() {
        let catalog = load_catalog().unwrap();
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/mobile/src/catalog.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let committed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(committed, catalog_value(&catalog).unwrap());
    }

    #[test]
    fn catalog_response_carries_the_json_body() {
        let json = catalog_json(&load_catalog().unwrap()).unwrap();
        let response = http_response("200 OK", "application/json", json.as_bytes());
        let text = String::from_utf8(response).unwrap();
        let (header, body) = text.split_once("\r\n\r\n").unwrap();
        assert!(header.contains("200 OK"));
        assert!(header.contains(&format!("Content-Length: {}", json.len())));
        assert_eq!(body, json);
        assert!(body.contains("Marlow Goods"));
        assert!(body.contains("Paperback novel"));
    }
}
