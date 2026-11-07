//! `GET /media/{hash}`: image bytes named by their SHA-256 in a store record.
//!
//! The node serves a file only if its bytes hash to the name asked for, are
//! at most [`MAX_MEDIA`] bytes, and start like a PNG, JPEG, GIF, or WebP
//! image; the content type comes from those bytes, not from a file name.
//! Files live in `media/<hex>` in the data directory. On testnet4 the demo
//! stores' images are built in. Hosting and replicating images is not done
//! here.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;

use bitcoin::hashes::{sha256, Hash};
use openbazaar_protocol::fixture_media;

use crate::http::{http_response_cors, json_error, local_guard, Client};
use crate::network::NetworkMode;
use crate::{Error, Node};

/// Largest image the node serves.
pub const MAX_MEDIA: usize = 2 * 1024 * 1024;

pub(crate) struct Media {
    dir: PathBuf,
    builtin: HashMap<[u8; 32], &'static [u8]>,
}

/// The content type of an image, from its first bytes.
pub(crate) fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

impl Media {
    pub(crate) fn open(dir: PathBuf, network: NetworkMode) -> Result<Self, Error> {
        fs::create_dir_all(&dir).map_err(|err| Error::Detail(err.to_string()))?;
        let builtin = match network {
            NetworkMode::Testnet4 => fixture_media()
                .into_iter()
                .map(|bytes| (sha256::Hash::hash(bytes).to_byte_array(), bytes))
                .collect(),
            NetworkMode::Mainnet => HashMap::new(),
        };
        Ok(Self { dir, builtin })
    }

    /// Keep an image under its SHA-256. Only the image types [`image_type`]
    /// knows, up to [`MAX_MEDIA`] bytes.
    pub(crate) fn put(&self, bytes: &[u8]) -> Result<([u8; 32], &'static str), String> {
        if bytes.len() > MAX_MEDIA {
            return Err("That image is larger than 2 MiB.".into());
        }
        let kind = image_type(bytes).ok_or("Upload a PNG, JPEG, GIF, or WebP image.")?;
        let hash = sha256::Hash::hash(bytes).to_byte_array();
        if self.get(&hash).is_none() {
            let name: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
            let tmp = self.dir.join(format!("{name}.tmp"));
            fs::write(&tmp, bytes)
                .and_then(|_| fs::rename(&tmp, self.dir.join(&name)))
                .map_err(|err| err.to_string())?;
        }
        Ok((hash, kind))
    }

    /// The bytes and content type for `hash`, only if everything checks.
    pub(crate) fn get(&self, hash: &[u8; 32]) -> Option<(&'static str, Vec<u8>)> {
        let bytes = match self.builtin.get(hash) {
            Some(bytes) => bytes.to_vec(),
            None => {
                let name: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
                let file = fs::File::open(self.dir.join(name)).ok()?;
                let mut bytes = Vec::new();
                file.take(MAX_MEDIA as u64 + 1).read_to_end(&mut bytes).ok()?;
                bytes
            }
        };
        if bytes.len() > MAX_MEDIA || sha256::Hash::hash(&bytes).to_byte_array() != *hash {
            return None;
        }
        Some((image_type(&bytes)?, bytes))
    }
}

/// `GET /media/{hash}`: paired loopback callers only.
pub(crate) fn response(node: &Node, client: &Client, head: &str, hash: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let hash = hash.trim_end_matches('/');
    let parsed = (hash.len() == 64)
        .then(|| bitcoin::hashes::hex::FromHex::from_hex(hash).ok())
        .flatten()
        .and_then(|bytes: Vec<u8>| <[u8; 32]>::try_from(bytes).ok());
    match parsed.and_then(|hash| node.media.get(&hash)) {
        Some((content_type, bytes)) => http_response_cors("200 OK", content_type, &bytes, origin),
        None => json_error("404 Not Found", "That image is not on this node.", origin),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_served_only_when_the_hash_and_type_check() {
        let dir = std::env::temp_dir().join(format!("ob-media-{}-{}", std::process::id(), line!()));
        let media = Media::open(dir.clone(), NetworkMode::Testnet4).unwrap();
        let demo = fixture_media()[0];
        let hash = sha256::Hash::hash(demo).to_byte_array();
        assert_eq!(media.get(&hash).unwrap(), ("image/png", demo.to_vec()));

        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 1, 2, 3];
        let jpeg_hash = sha256::Hash::hash(&jpeg).to_byte_array();
        let name = |hash: &[u8; 32]| hash.iter().map(|b| format!("{b:02x}")).collect::<String>();
        fs::write(dir.join(name(&jpeg_hash)), jpeg).unwrap();
        assert_eq!(media.get(&jpeg_hash).unwrap().0, "image/jpeg");

        let swapped = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 9];
        fs::write(dir.join(name(&jpeg_hash)), swapped).unwrap();
        assert!(media.get(&jpeg_hash).is_none(), "bytes that do not hash to the name are refused");

        let script = b"<svg onload=alert(1)>";
        let script_hash = sha256::Hash::hash(script).to_byte_array();
        fs::write(dir.join(name(&script_hash)), script).unwrap();
        assert!(media.get(&script_hash).is_none(), "only known image types are served");

        let big = [b"GIF89a".as_slice(), &vec![0u8; MAX_MEDIA]].concat();
        let big_hash = sha256::Hash::hash(&big).to_byte_array();
        fs::write(dir.join(name(&big_hash)), &big).unwrap();
        assert!(media.get(&big_hash).is_none(), "images above the cap are refused");

        let mainnet = Media::open(dir.clone(), NetworkMode::Mainnet).unwrap();
        assert!(mainnet.get(&hash).is_none(), "demo images are testnet4 only");

        let (put, kind) = mainnet.put(&jpeg).unwrap();
        assert_eq!((put, kind), (jpeg_hash, "image/jpeg"));
        assert_eq!(mainnet.get(&jpeg_hash).unwrap().1, jpeg.to_vec());
        assert!(mainnet.put(script).is_err(), "only images are kept");
        assert!(mainnet.put(&big).is_err(), "nothing above the cap is kept");
        let _ = fs::remove_dir_all(dir);
    }
}
