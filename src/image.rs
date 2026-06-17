//! Parsing and validation of MCUboot firmware images.
//!
//! See the MCUboot image format reference:
//! <https://interrupt.memfault.com/blog/mcuboot-overview#mcuboot-image-binaries>

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// MCUboot image magic, stored little-endian at offset 0.
const IMAGE_MAGIC: u32 = 0x96f3_b83d;
/// Magic marking the start of the unprotected TLV area.
const TLV_INFO_MAGIC: u16 = 0x6907;
/// Magic marking the start of the protected TLV area.
const TLV_PROT_INFO_MAGIC: u16 = 0x6908;
/// TLV tag carrying the image's SHA-256 hash.
const TLV_SHA256: u16 = 0x10;
/// Minimum size of an MCUboot image header.
const MIN_HEADER_LEN: usize = 32;

/// Information extracted from an MCUboot image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    /// Semantic version string, e.g. `"1.2.3"`.
    pub version: String,
    /// Size of the image header in bytes.
    pub header_size: u16,
    /// Size of the application payload (excludes header and TLVs).
    pub image_size: u32,
    /// Size of the protected TLV area in bytes.
    pub protected_tlv_size: u16,
    /// Image flags field.
    pub flags: u32,
    /// SHA-256 computed over the header + image + protected TLV area. This is
    /// the hash MCUmgr uses to identify the image.
    pub hash: [u8; 32],
    /// Whether the SHA-256 TLV embedded in the image matches [`hash`](Self::hash).
    /// `None` if the image carries no SHA-256 TLV.
    pub hash_valid: Option<bool>,
}

impl ImageInfo {
    /// The computed image hash as a lowercase hex string.
    pub fn hash_hex(&self) -> String {
        hex::encode(self.hash)
    }
}

/// Parse and validate an MCUboot image.
///
/// Returns [`Error::InvalidImage`] if the magic is wrong or the declared sizes
/// do not fit within `data`.
pub fn parse(data: &[u8]) -> Result<ImageInfo> {
    if data.len() < MIN_HEADER_LEN {
        return Err(Error::InvalidImage(format!(
            "file too short: {} bytes (need at least {MIN_HEADER_LEN})",
            data.len()
        )));
    }

    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if magic != IMAGE_MAGIC {
        return Err(Error::InvalidImage(format!(
            "bad magic 0x{magic:08x} (expected 0x{IMAGE_MAGIC:08x})"
        )));
    }

    let header_size = u16::from_le_bytes([data[8], data[9]]);
    let protected_tlv_size = u16::from_le_bytes([data[10], data[11]]);
    let image_size = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);
    let flags = u32::from_le_bytes([data[16], data[17], data[18], data[19]]);
    let version = format!(
        "{}.{}.{}",
        data[20],
        data[21],
        u16::from_le_bytes([data[22], data[23]])
    );

    // The hash covers the header, image, and protected TLV area.
    let hashed_len = header_size as usize + image_size as usize + protected_tlv_size as usize;
    if hashed_len > data.len() {
        return Err(Error::InvalidImage(format!(
            "declared sizes ({hashed_len} bytes) exceed file ({} bytes)",
            data.len()
        )));
    }
    let hash: [u8; 32] = {
        let mut hasher = Sha256::new();
        hasher.update(&data[..hashed_len]);
        hasher.finalize().into()
    };

    // Walk the TLV area (best effort) looking for the embedded SHA-256 tag.
    let tlv_start = header_size as usize + image_size as usize;
    let hash_valid = find_sha256_tlv(&data[tlv_start..]).map(|embedded| embedded == hash);

    Ok(ImageInfo {
        version,
        header_size,
        image_size,
        protected_tlv_size,
        flags,
        hash,
        hash_valid,
    })
}

/// Scan a TLV region (protected and/or unprotected) for the SHA-256 tag,
/// returning its value if present. Returns `None` on any structural problem,
/// since the TLV area is advisory for our purposes.
fn find_sha256_tlv(mut region: &[u8]) -> Option<[u8; 32]> {
    // The region may begin with a protected TLV block followed by an
    // unprotected one. Handle either by consuming successive info blocks.
    while region.len() >= 4 {
        let magic = u16::from_le_bytes([region[0], region[1]]);
        if magic != TLV_INFO_MAGIC && magic != TLV_PROT_INFO_MAGIC {
            return None;
        }
        let total = u16::from_le_bytes([region[2], region[3]]) as usize;
        if total < 4 || total > region.len() {
            return None;
        }
        let mut entries = &region[4..total];
        while entries.len() >= 4 {
            let tag = u16::from_le_bytes([entries[0], entries[1]]);
            let len = u16::from_le_bytes([entries[2], entries[3]]) as usize;
            let value_start = 4;
            let value_end = value_start + len;
            if value_end > entries.len() {
                return None;
            }
            if tag == TLV_SHA256 && len == 32 {
                return entries[value_start..value_end].try_into().ok();
            }
            entries = &entries[value_end..];
        }
        region = &region[total..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha256(data: &[u8]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finalize().into()
    }

    /// Build a minimal valid MCUboot image with a 32-byte header and a single
    /// unprotected SHA-256 TLV. If `tlv_hash` is `Some`, that value is embedded
    /// as the SHA-256 TLV; if `None`, no TLV area is appended.
    fn build_image(
        major: u8,
        minor: u8,
        revision: u16,
        app: &[u8],
        tlv_hash: Option<[u8; 32]>,
    ) -> Vec<u8> {
        let mut img = Vec::new();
        img.extend_from_slice(&IMAGE_MAGIC.to_le_bytes()); // 0: magic
        img.extend_from_slice(&0u32.to_le_bytes()); // 4: load addr
        img.extend_from_slice(&32u16.to_le_bytes()); // 8: header size
        img.extend_from_slice(&0u16.to_le_bytes()); // 10: protected tlv size
        img.extend_from_slice(&(app.len() as u32).to_le_bytes()); // 12: image size
        img.extend_from_slice(&0u32.to_le_bytes()); // 16: flags
        img.push(major); // 20
        img.push(minor); // 21
        img.extend_from_slice(&revision.to_le_bytes()); // 22: revision
        img.extend_from_slice(&0u32.to_le_bytes()); // 24: build number
        img.extend_from_slice(&[0u8; 4]); // 28: padding to header size 32
        assert_eq!(img.len(), 32);
        img.extend_from_slice(app);

        if let Some(hash) = tlv_hash {
            // Unprotected TLV info: magic, total length (incl. 4-byte header).
            let entry_len = 4 + hash.len(); // tag + len + value
            let total = (4 + entry_len) as u16;
            img.extend_from_slice(&TLV_INFO_MAGIC.to_le_bytes());
            img.extend_from_slice(&total.to_le_bytes());
            img.extend_from_slice(&TLV_SHA256.to_le_bytes());
            img.extend_from_slice(&(hash.len() as u16).to_le_bytes());
            img.extend_from_slice(&hash);
        }
        img
    }

    #[test]
    fn parses_version_and_sizes() {
        let app = [0xAAu8; 16];
        let img = build_image(1, 2, 3, &app, None);
        let info = parse(&img).unwrap();
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.image_size, 16);
        assert_eq!(info.header_size, 32);
    }

    #[test]
    fn computes_hash_over_header_and_image() {
        let app = [0x11u8; 8];
        let img = build_image(0, 1, 0, &app, None);
        let expected = sha256(&img[..32 + app.len()]);
        let info = parse(&img).unwrap();
        assert_eq!(info.hash, expected);
        assert_eq!(info.hash_hex(), hex::encode(expected));
    }

    #[test]
    fn validates_matching_embedded_hash() {
        let app = [0x22u8; 8];
        // First build without TLV to learn the correct hash.
        let bare = build_image(2, 0, 0, &app, None);
        let correct = sha256(&bare[..32 + app.len()]);
        let img = build_image(2, 0, 0, &app, Some(correct));
        let info = parse(&img).unwrap();
        assert_eq!(info.hash_valid, Some(true));
    }

    #[test]
    fn flags_mismatched_embedded_hash() {
        let app = [0x33u8; 8];
        let img = build_image(2, 0, 0, &app, Some([0xFF; 32]));
        let info = parse(&img).unwrap();
        assert_eq!(info.hash_valid, Some(false));
    }

    #[test]
    fn hash_valid_is_none_without_tlv() {
        let app = [0x44u8; 8];
        let img = build_image(1, 0, 0, &app, None);
        let info = parse(&img).unwrap();
        assert_eq!(info.hash_valid, None);
    }

    #[test]
    fn rejects_too_short_input() {
        assert!(matches!(parse(&[0u8; 10]), Err(Error::InvalidImage(_))));
    }

    #[test]
    fn rejects_wrong_magic() {
        let mut img = build_image(1, 0, 0, &[0u8; 8], None);
        img[0] ^= 0xFF;
        assert!(matches!(parse(&img), Err(Error::InvalidImage(_))));
    }

    #[test]
    fn rejects_image_size_larger_than_file() {
        let mut img = build_image(1, 0, 0, &[0u8; 8], None);
        // Overwrite image size (offset 12) with an absurd value.
        img[12..16].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert!(matches!(parse(&img), Err(Error::InvalidImage(_))));
    }
}
