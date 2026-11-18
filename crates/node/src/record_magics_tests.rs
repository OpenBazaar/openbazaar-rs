//! Every record, snapshot, and frame in the workspace starts with its own
//! 4-byte magic. The magics are private constants in several crates, so this
//! reads the sources: a `const` whose name holds `MAGIC`, or an escrow
//! `Writer::new(b"....", _)`. A `LEGACY_` magic is read-only and is skipped,
//! as are `tests` directories and `*_tests.rs` files.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        if path.is_dir() {
            if name != "tests" {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") {
            out.push(path);
        }
    }
}

/// The 4-byte literal that starts `text`, after an optional `*b` or `b`.
fn literal(text: &str) -> Option<String> {
    let text = text.trim_start().trim_start_matches('*').trim_start_matches('b').strip_prefix('"')?;
    let magic = &text[..text.find('"')?];
    (magic.len() == 4).then(|| magic.to_string())
}

fn magic_on(line: &str) -> Option<String> {
    let line = line.trim();
    if line.starts_with("//") {
        return None;
    }
    if let Some((_, rest)) = line.split_once("Writer::new(") {
        return literal(rest);
    }
    let (name, value) = line.split_once("const ")?.1.split_once('=')?;
    let name = name.split(':').next()?.trim();
    (name.contains("MAGIC") && !name.starts_with("LEGACY_")).then(|| literal(value)).flatten()
}

fn defined_magics() -> BTreeMap<String, Vec<String>> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for entry in fs::read_dir(crates).unwrap() {
        let src = entry.unwrap().path().join("src");
        if src.is_dir() {
            sources(&src, &mut files);
        }
    }
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for (index, line) in text.lines().enumerate() {
            if let Some(magic) = magic_on(line) {
                let at = format!("{}:{}", file.strip_prefix(crates).unwrap().display(), index + 1);
                found.entry(magic).or_default().push(at);
            }
        }
    }
    found
}

#[test]
fn every_record_magic_is_defined_once() {
    let found = defined_magics();
    for magic in ["OBST", "OBSD", "OBAD", "OBBD", "OBBN", "OBSB", "OBBS", "OBML", "OBRM", "OBK1"] {
        assert!(found.contains_key(magic), "the scan misses {magic}: {found:?}");
    }
    let repeated: Vec<_> = found.iter().filter(|(_, at)| at.len() > 1).collect();
    assert!(repeated.is_empty(), "two formats share a magic: {repeated:?}");
}

#[test]
fn the_scan_reads_consts_and_escrow_writers_but_not_legacy_or_comments() {
    assert_eq!(magic_on(r#"const MAGIC: [u8; 4] = *b"OBST";"#).as_deref(), Some("OBST"));
    assert_eq!(magic_on(r#"    const MAGIC: [u8; 4] = *b"OBBN";"#).as_deref(), Some("OBBN"));
    assert_eq!(magic_on(r#"pub(crate) const SUBMIT_MAGIC: &[u8; 4] = b"OBSB";"#).as_deref(), Some("OBSB"));
    assert_eq!(magic_on(r#"const KEYSTORE_MAGIC: &str = "OBK1";"#).as_deref(), Some("OBK1"));
    assert_eq!(magic_on(r#"        let mut w = Writer::new(b"OBRM", 2);"#).as_deref(), Some("OBRM"));
    assert_eq!(magic_on(r#"const LEGACY_SNAPSHOT_MAGIC: &[u8; 4] = b"OBSB";"#), None);
    assert_eq!(magic_on(r#"// const MAGIC: [u8; 4] = *b"OBXX";"#), None);
    assert_eq!(magic_on(r#"const LAN_LABEL: &[u8] = b"OBLAN1";"#), None);
}
