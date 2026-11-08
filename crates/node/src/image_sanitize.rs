//! Decode an uploaded image and, when it carries identifying metadata, write
//! a new file without it.
//!
//! A camera JPEG can carry GPS coordinates, a body serial, and a comment in
//! EXIF, XMP, IPTC, or a GIF comment. Those bytes would be published under
//! the image hash. The node applies the EXIF orientation first, then encodes
//! a new image. An image that is already free of those fields is stored
//! unchanged, so a peer that copies it still sees the hash the store record
//! names.
//!
//! A file already on disk is checked again when it is served. If it still
//! has identifying metadata, the response is the cleaned image and the raw
//! file is not sent. A file that cannot be decoded inside the dimension and
//! memory limits is not stored and not served. A replica that requires the
//! original hash will not cache a cleaned legacy file under that hash; new
//! uploads are hashed after cleaning, so they replicate.

use std::io::Cursor;

use image::codecs::gif::{GifDecoder, GifEncoder};
use image::codecs::jpeg::JpegEncoder;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, Limits};

use crate::media::{image_type, MAX_MEDIA};

pub(crate) const MAX_DIMENSION: u32 = 8192;
pub(crate) const MAX_PIXELS: u64 = 16_777_216;
pub(crate) const MAX_DECODED: u64 = 64 * 1024 * 1024;
const MAX_GIF_FRAMES: usize = 300;

/// Bytes to store, and their content type. Metadata is removed before the
/// caller hashes them.
pub(crate) fn prepare_image(bytes: &[u8]) -> Result<(Vec<u8>, &'static str), String> {
    let kind = image_type(bytes).ok_or("Upload a PNG, JPEG, GIF, or WebP image.")?;
    if bytes.len() > MAX_MEDIA {
        return Err("That image is larger than 2 MiB.".into());
    }
    let clean = if identifying_metadata(bytes) {
        let (clean, cleaned) = rewrite(bytes)?;
        if identifying_metadata(&clean) {
            return Err("That image still had identifying metadata after it was rewritten.".into());
        }
        if cleaned != kind && image_type(&clean) != Some(cleaned) {
            return Err("That image could not be rewritten.".into());
        }
        clean
    } else {
        validate(bytes)?;
        bytes.to_vec()
    };
    if clean.len() > MAX_MEDIA {
        return Err("That image is larger than 2 MiB.".into());
    }
    let kind = image_type(&clean).ok_or("Upload a PNG, JPEG, GIF, or WebP image.")?;
    Ok((clean, kind))
}

/// What to send for bytes that already hash to the name asked for.
pub(crate) fn serve_stored(bytes: &[u8]) -> Option<(&'static str, Vec<u8>)> {
    if bytes.len() > MAX_MEDIA || image_type(bytes).is_none() {
        return None;
    }
    let (clean, kind) = prepare_image(bytes).ok()?;
    Some((kind, clean))
}

/// EXIF, XMP, IPTC, comments, and PNG text. A decoder that only checks the
/// magic would still publish these.
pub(crate) fn identifying_metadata(bytes: &[u8]) -> bool {
    jpeg_metadata(bytes) || png_metadata(bytes) || gif_metadata(bytes) || webp_metadata(bytes)
}

fn validate(bytes: &[u8]) -> Result<(), String> {
    if image_type(bytes) == Some("image/gif") {
        decode_gif(bytes).map(|_| ())
    } else {
        decode_still(bytes).map(|_| ())
    }
}

fn rewrite(bytes: &[u8]) -> Result<(Vec<u8>, &'static str), String> {
    if image_type(bytes) == Some("image/gif") {
        let frames = decode_gif(bytes)?;
        let mut out = Vec::new();
        GifEncoder::new(&mut out)
            .encode_frames(frames)
            .map_err(|err| err.to_string())?;
        return Ok((out, "image/gif"));
    }
    let orientation = jpeg_orientation(bytes).or_else(|| png_orientation(bytes));
    let mut image = decode_still(bytes)?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    check_dims(image.width(), image.height())?;
    encode_still(&image, image_type(bytes).unwrap_or("image/png"))
}

fn decode_still(bytes: &[u8]) -> Result<DynamicImage, String> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| err.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|err| err.to_string())?;
    decoder
        .set_limits(limits())
        .map_err(|_| "That image is larger than this node will decode.".to_string())?;
    let image = DynamicImage::from_decoder(decoder).map_err(|err| err.to_string())?;
    check_dims(image.width(), image.height())?;
    Ok(image)
}

fn decode_gif(bytes: &[u8]) -> Result<Vec<image::Frame>, String> {
    let mut decoder = GifDecoder::new(Cursor::new(bytes)).map_err(|err| err.to_string())?;
    decoder
        .set_limits(limits())
        .map_err(|_| "That image is larger than this node will decode.".to_string())?;
    let frames = decoder
        .into_frames()
        .collect_frames()
        .map_err(|err| err.to_string())?;
    if frames.is_empty() {
        return Err("That GIF has no frames.".into());
    }
    if frames.len() > MAX_GIF_FRAMES {
        return Err("That GIF has too many frames.".into());
    }
    let mut pixels = 0u64;
    for frame in &frames {
        let (width, height) = frame.buffer().dimensions();
        check_dims(width, height)?;
        pixels = pixels.saturating_add(u64::from(width) * u64::from(height));
        if pixels > MAX_PIXELS {
            return Err("That image is larger than this node will decode.".into());
        }
    }
    Ok(frames)
}

fn encode_still(image: &DynamicImage, kind: &str) -> Result<(Vec<u8>, &'static str), String> {
    let mut out = Vec::new();
    match kind {
        "image/jpeg" => {
            let mut encoder = JpegEncoder::new_with_quality(&mut out, 95);
            encoder.encode_image(image).map_err(|err| err.to_string())?;
            Ok((out, "image/jpeg"))
        }
        "image/webp" => {
            image
                .write_to(&mut Cursor::new(&mut out), ImageFormat::WebP)
                .map_err(|err| err.to_string())?;
            Ok((out, "image/webp"))
        }
        "image/gif" => {
            image
                .write_to(&mut Cursor::new(&mut out), ImageFormat::Gif)
                .map_err(|err| err.to_string())?;
            Ok((out, "image/gif"))
        }
        _ => {
            image
                .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
                .map_err(|err| err.to_string())?;
            Ok((out, "image/png"))
        }
    }
}

fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED);
    limits
}

fn check_dims(width: u32, height: u32) -> Result<(), String> {
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        Err("That image is larger than this node will decode.".into())
    } else {
        Ok(())
    }
}

fn jpeg_metadata(bytes: &[u8]) -> bool {
    if bytes.len() < 4 || bytes[0] != 0xff || bytes[1] != 0xd8 {
        return false;
    }
    let mut i = 2;
    while i + 1 < bytes.len() {
        if bytes[i] != 0xff {
            return true;
        }
        while i < bytes.len() && bytes[i] == 0xff {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let marker = bytes[i];
        i += 1;
        if marker == 0xd8 || marker == 0xd9 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            if marker == 0xd9 {
                break;
            }
            continue;
        }
        if marker == 0xda {
            break;
        }
        if i + 2 > bytes.len() {
            return true;
        }
        let len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
        if len < 2 || i + len > bytes.len() {
            return true;
        }
        if marker == 0xfe || marker == 0xe1 || marker == 0xed {
            return true;
        }
        i += len;
    }
    false
}

fn png_metadata(bytes: &[u8]) -> bool {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !bytes.starts_with(SIG) {
        return false;
    }
    let mut i = SIG.len();
    while i + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        let kind = &bytes[i + 4..i + 8];
        if kind == b"IEND" {
            return false;
        }
        if matches!(kind, b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" | b"tIME") {
            return true;
        }
        let next = i.checked_add(12).and_then(|at| at.checked_add(len));
        let Some(next) = next else { return true };
        if next > bytes.len() {
            return true;
        }
        i = next;
    }
    false
}

fn gif_metadata(bytes: &[u8]) -> bool {
    if bytes.len() < 13 || !(bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return false;
    }
    let mut i = 13;
    if bytes[10] & 0x80 != 0 {
        i += 3 * (1usize << ((bytes[10] & 7) + 1));
    }
    while i < bytes.len() {
        match bytes[i] {
            0x3b => return false,
            0x21 => {
                if i + 1 >= bytes.len() {
                    return true;
                }
                let label = bytes[i + 1];
                if label == 0xfe || label == 0xff {
                    return true;
                }
                i = gif_skip_subblocks(bytes, i + 2);
            }
            0x2c => {
                if i + 10 > bytes.len() {
                    return true;
                }
                let packed = bytes[i + 9];
                i += 10;
                if packed & 0x80 != 0 {
                    i += 3 * (1usize << ((packed & 7) + 1));
                }
                if i >= bytes.len() {
                    return true;
                }
                i = gif_skip_subblocks(bytes, i + 1);
            }
            _ => return true,
        }
        if i > bytes.len() {
            return true;
        }
    }
    false
}

fn gif_skip_subblocks(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() {
        let n = bytes[i] as usize;
        i += 1;
        if n == 0 {
            break;
        }
        i = i.saturating_add(n);
    }
    i
}

fn webp_metadata(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return false;
    }
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let kind = &bytes[i..i + 4];
        let len = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
        if kind == b"EXIF" || kind == b"XMP " {
            return true;
        }
        let Some(next) = i
            .checked_add(8)
            .and_then(|at| at.checked_add(len + (len & 1)))
        else {
            return true;
        };
        if next > bytes.len() {
            return true;
        }
        i = next;
    }
    false
}

/// Orientation 5–8 turns the image. The tag is read here and then dropped
/// with the rest of the EXIF segment.
fn jpeg_orientation(bytes: &[u8]) -> Option<image::metadata::Orientation> {
    let app1 = jpeg_app1(bytes)?;
    let tiff = app1.strip_prefix(b"Exif\0\0")?;
    orientation_tag(tiff)
}

fn png_orientation(bytes: &[u8]) -> Option<image::metadata::Orientation> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !bytes.starts_with(SIG) {
        return None;
    }
    let mut i = SIG.len();
    while i + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[i..i + 4].try_into().ok()?) as usize;
        let kind = &bytes[i + 4..i + 8];
        let data_at = i + 8;
        if kind == b"eXIf" && data_at + len <= bytes.len() {
            return orientation_tag(&bytes[data_at..data_at + len]);
        }
        if kind == b"IEND" {
            return None;
        }
        i = i.checked_add(12 + len)?;
    }
    None
}

fn jpeg_app1(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < 4 || bytes[0] != 0xff || bytes[1] != 0xd8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= bytes.len() && bytes[i] == 0xff {
        while i < bytes.len() && bytes[i] == 0xff {
            i += 1;
        }
        let marker = *bytes.get(i)?;
        i += 1;
        if marker == 0xda || marker == 0xd9 {
            return None;
        }
        let len = u16::from_be_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]) as usize;
        if len < 2 || i + len > bytes.len() {
            return None;
        }
        if marker == 0xe1 {
            return Some(&bytes[i + 2..i + len]);
        }
        i += len;
    }
    None
}

fn orientation_tag(tiff: &[u8]) -> Option<image::metadata::Orientation> {
    if tiff.len() < 8 {
        return None;
    }
    let le = match &tiff[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let read_u16 = |at: usize| {
        let bytes: [u8; 2] = tiff.get(at..at + 2)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    };
    let read_u32 = |at: usize| {
        let bytes: [u8; 4] = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    };
    if read_u16(2)? != 42 {
        return None;
    }
    let mut ifd = read_u32(4)? as usize;
    for _ in 0..8 {
        let count = read_u16(ifd)? as usize;
        let entries = ifd.checked_add(2)?;
        for n in 0..count {
            let at = entries.checked_add(n * 12)?;
            if read_u16(at)? == 0x0112 {
                let value = read_u16(at + 8)?;
                return orientation_from(value);
            }
        }
        ifd = read_u32(entries + count * 12)? as usize;
        if ifd == 0 {
            break;
        }
    }
    None
}

fn orientation_from(value: u16) -> Option<image::metadata::Orientation> {
    u8::try_from(value)
        .ok()
        .and_then(image::metadata::Orientation::from_exif)
}

/// A tiny JPEG whose APP1 segment carries `secret` as if it were a GPS note.
#[cfg(test)]
pub(crate) fn jpeg_with_note(width: u32, height: u32, secret: &str, orientation: u16) -> Vec<u8> {
    let mut pixels = image::RgbImage::new(width, height);
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        *pixel = image::Rgb([(x as u8).wrapping_add(orientation as u8), y as u8, 40]);
    }
    let mut clean = Vec::new();
    JpegEncoder::new_with_quality(&mut clean, 95)
        .encode_image(&DynamicImage::ImageRgb8(pixels))
        .unwrap();
    let mut exif = b"Exif\0\0II*\0\x08\0\0\0".to_vec();
    exif.extend_from_slice(&1u16.to_le_bytes());
    exif.extend_from_slice(&0x0112u16.to_le_bytes());
    exif.extend_from_slice(&3u16.to_le_bytes());
    exif.extend_from_slice(&1u32.to_le_bytes());
    exif.extend_from_slice(&orientation.to_le_bytes());
    exif.extend_from_slice(&0u16.to_le_bytes());
    exif.extend_from_slice(&0u32.to_le_bytes());
    exif.extend_from_slice(secret.as_bytes());
    let mut segment = Vec::new();
    segment.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    segment.extend_from_slice(&exif);
    let mut out = vec![0xff, 0xd8, 0xff, 0xe1];
    out.extend(segment);
    let mut comment = secret.as_bytes().to_vec();
    let mut com = vec![0xff, 0xfe];
    com.extend_from_slice(&((comment.len() + 2) as u16).to_be_bytes());
    com.append(&mut comment);
    out.extend(com);
    out.extend_from_slice(&clean[2..]);
    out
}

#[cfg(test)]
mod tests {
    use bitcoin::hashes::Hash;

    use super::*;
    use crate::media::Media;
    use crate::network::NetworkMode;

    fn still(kind: &str) -> Vec<u8> {
        let image =
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30])));
        encode_still(&image, kind).unwrap().0
    }

    #[test]
    fn every_format_round_trips_without_metadata() {
        for kind in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
            let bytes = still(kind);
            assert!(
                !identifying_metadata(&bytes),
                "{kind} encoder output looked dirty"
            );
            let (clean, cleaned) = prepare_image(&bytes).unwrap();
            assert_eq!(cleaned, kind);
            assert_eq!(
                clean, bytes,
                "{kind} changed even though it had no metadata"
            );
        }
    }

    #[test]
    fn a_gps_note_and_comment_are_removed_and_orientation_is_applied() {
        let secret = "GPS 37.7749,-122.4194 Canon-SECRET";
        let dirty = jpeg_with_note(2, 1, secret, 6);
        assert!(identifying_metadata(&dirty));
        assert!(dirty
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
        let (clean, kind) = prepare_image(&dirty).unwrap();
        assert_eq!(kind, "image/jpeg");
        assert!(!identifying_metadata(&clean));
        assert!(!clean
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
        let turned = decode_still(&clean).unwrap();
        assert_eq!(
            (turned.width(), turned.height()),
            (1, 2),
            "orientation 6 turns 2x1 into 1x2"
        );
    }

    #[test]
    fn an_image_over_the_dimension_limit_is_refused() {
        let image = DynamicImage::ImageRgb8(image::RgbImage::new(MAX_DIMENSION + 1, 1));
        let bytes = encode_still(&image, "image/png").unwrap().0;
        assert!(prepare_image(&bytes).is_err());
    }

    #[test]
    fn a_previously_stored_image_is_not_served_with_its_metadata() {
        let secret = "GPS 37.7749,-122.4194 Canon-SECRET";
        let dirty = jpeg_with_note(2, 1, secret, 1);
        let hash = bitcoin::hashes::sha256::Hash::hash(&dirty).to_byte_array();
        let dir = std::env::temp_dir().join(format!("ob-image-{}-{}", std::process::id(), line!()));
        let media = Media::open(dir.clone(), NetworkMode::Mainnet).unwrap();
        let name: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
        std::fs::write(dir.join(name), &dirty).unwrap();
        let (kind, served) = media.get(&hash).unwrap();
        assert_eq!(kind, "image/jpeg");
        assert_ne!(served, dirty);
        assert!(!served
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
