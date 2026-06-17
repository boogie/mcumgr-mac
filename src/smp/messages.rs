//! Typed CBOR request payloads and response structures for the OS and Image
//! management groups.
//!
//! Byte-valued fields (`hash`, `data`, `sha`) are serialised as CBOR byte
//! strings via [`serde_bytes`], matching what MCUmgr devices expect.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Encode a value as a CBOR payload.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).map_err(|e| Error::CborEncode(e.to_string()))?;
    Ok(out)
}

/// Decode a CBOR payload into a typed value.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    ciborium::from_reader(bytes).map_err(|e| Error::CborDecode(e.to_string()))
}

/// Inspect a response payload's `rc` field and turn a non-zero code into an
/// [`Error::Mgmt`]. A missing or zero `rc` is treated as success (some nRF52
/// firmware omits `rc` on success).
pub fn check_rc(payload: &[u8]) -> Result<()> {
    let ReturnCode { rc } = decode(payload).unwrap_or_default();
    match rc {
        Some(code) if code != 0 => Err(Error::Mgmt(crate::smp::MgmtError::new(code))),
        _ => Ok(()),
    }
}

/// Minimal view of any response carrying a return code.
#[derive(Debug, Default, Deserialize)]
struct ReturnCode {
    #[serde(default)]
    rc: Option<u16>,
}

// ---------------------------------------------------------------------------
// OS group
// ---------------------------------------------------------------------------

/// `os echo` request: `{ "d": <text> }`.
#[derive(Debug, Serialize)]
pub struct EchoRequest<'a> {
    pub d: &'a str,
}

/// `os echo` response: `{ "r": <text> }`.
#[derive(Debug, Deserialize)]
pub struct EchoResponse {
    pub r: String,
}

// ---------------------------------------------------------------------------
// Image group
// ---------------------------------------------------------------------------

/// `image state` write request used for both test and confirm:
/// `{ "hash": <bytes>, "confirm": <bool> }`.
#[derive(Debug, Serialize)]
pub struct ImageStateWrite {
    #[serde(with = "serde_bytes")]
    pub hash: Vec<u8>,
    pub confirm: bool,
}

/// One image slot reported by `image state`.
#[derive(Debug, Default, Deserialize)]
pub struct ImageSlot {
    #[serde(default)]
    pub slot: u32,
    #[serde(default)]
    pub version: String,
    #[serde(with = "serde_bytes", default)]
    pub hash: Vec<u8>,
    #[serde(default)]
    pub bootable: bool,
    #[serde(default)]
    pub pending: bool,
    #[serde(default)]
    pub confirmed: bool,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub permanent: bool,
}

/// `image state` read response.
#[derive(Debug, Default, Deserialize)]
pub struct ImageStateResponse {
    #[serde(default)]
    pub images: Vec<ImageSlot>,
    #[serde(rename = "splitStatus", default)]
    pub split_status: Option<u32>,
}

/// A single `image upload` chunk request.
///
/// The first chunk (offset 0) additionally carries the total image length and
/// the SHA-256 of the whole image; later chunks omit them.
#[derive(Debug, Serialize)]
pub struct UploadChunk {
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
    pub off: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub len: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<serde_bytes::ByteBuf>,
}

impl UploadChunk {
    /// Build the first chunk, carrying total `len` and `sha`.
    pub fn first(data: Vec<u8>, total_len: u32, sha: Vec<u8>) -> Self {
        Self {
            data,
            off: 0,
            len: Some(total_len),
            sha: Some(serde_bytes::ByteBuf::from(sha)),
        }
    }

    /// Build a continuation chunk at `off`.
    pub fn next(data: Vec<u8>, off: u32) -> Self {
        Self {
            data,
            off,
            len: None,
            sha: None,
        }
    }
}

/// `image erase` request: `{ "slot": <n> }`. Older firmware ignores the extra
/// key and erases the secondary slot.
#[derive(Debug, Serialize)]
pub struct EraseRequest {
    pub slot: u8,
}

/// `image upload` response: the next expected offset.
#[derive(Debug, Default, Deserialize)]
pub struct UploadResponse {
    #[serde(default)]
    pub rc: Option<u16>,
    #[serde(default)]
    pub off: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ciborium::value::Value;

    /// Decode a CBOR payload into a generic [`Value`] for structural assertions.
    fn as_value(bytes: &[u8]) -> Value {
        ciborium::from_reader(bytes).expect("valid cbor")
    }

    /// Look up a string key in a CBOR map value.
    fn get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
        value
            .as_map()?
            .iter()
            .find_map(|(k, v)| (k.as_text() == Some(key)).then_some(v))
    }

    #[test]
    fn echo_request_encodes_to_expected_cbor() {
        let bytes = encode(&EchoRequest { d: "hi" }).unwrap();
        // map(1) { "d": "hi" }
        assert_eq!(bytes, [0xA1, 0x61, b'd', 0x62, b'h', b'i']);
    }

    #[test]
    fn echo_response_decodes_text() {
        let bytes = encode(&EchoRequest { d: "pong" }).unwrap();
        // EchoRequest and EchoResponse differ only in key name; craft a real one.
        let mut buf = Vec::new();
        ciborium::into_writer(
            &Value::Map(vec![(Value::Text("r".into()), Value::Text("pong".into()))]),
            &mut buf,
        )
        .unwrap();
        let _ = bytes;
        let resp: EchoResponse = decode(&buf).unwrap();
        assert_eq!(resp.r, "pong");
    }

    #[test]
    fn image_state_write_encodes_hash_as_byte_string() {
        let bytes = encode(&ImageStateWrite {
            hash: vec![0xde, 0xad, 0xbe, 0xef],
            confirm: true,
        })
        .unwrap();
        let value = as_value(&bytes);
        assert!(
            matches!(get(&value, "hash"), Some(Value::Bytes(b)) if b == &[0xde, 0xad, 0xbe, 0xef]),
            "hash must be a CBOR byte string, got {:?}",
            get(&value, "hash")
        );
        assert_eq!(get(&value, "confirm"), Some(&Value::Bool(true)));
    }

    #[test]
    fn upload_first_chunk_carries_len_and_sha_as_bytes() {
        let bytes = encode(&UploadChunk::first(vec![1, 2, 3], 1024, vec![0xaa, 0xbb])).unwrap();
        let value = as_value(&bytes);
        assert!(matches!(get(&value, "data"), Some(Value::Bytes(_))));
        assert!(matches!(get(&value, "sha"), Some(Value::Bytes(b)) if b == &[0xaa, 0xbb]));
        assert_eq!(get(&value, "off"), Some(&Value::Integer(0.into())));
        assert_eq!(get(&value, "len"), Some(&Value::Integer(1024.into())));
    }

    #[test]
    fn upload_next_chunk_omits_len_and_sha() {
        let bytes = encode(&UploadChunk::next(vec![4, 5, 6], 512)).unwrap();
        let value = as_value(&bytes);
        assert!(get(&value, "len").is_none());
        assert!(get(&value, "sha").is_none());
        assert_eq!(get(&value, "off"), Some(&Value::Integer(512.into())));
    }

    #[test]
    fn image_state_response_decodes_slots() {
        let slot = Value::Map(vec![
            (Value::Text("slot".into()), Value::Integer(0.into())),
            (Value::Text("version".into()), Value::Text("1.2.3".into())),
            (Value::Text("hash".into()), Value::Bytes(vec![0x01, 0x02])),
            (Value::Text("active".into()), Value::Bool(true)),
            (Value::Text("confirmed".into()), Value::Bool(true)),
        ]);
        let map = Value::Map(vec![(
            Value::Text("images".into()),
            Value::Array(vec![slot]),
        )]);
        let mut buf = Vec::new();
        ciborium::into_writer(&map, &mut buf).unwrap();

        let resp: ImageStateResponse = decode(&buf).unwrap();
        assert_eq!(resp.images.len(), 1);
        let img = &resp.images[0];
        assert_eq!(img.version, "1.2.3");
        assert_eq!(img.hash, vec![0x01, 0x02]);
        assert!(img.active);
        assert!(img.confirmed);
        assert!(!img.pending);
    }

    #[test]
    fn check_rc_accepts_zero_and_missing() {
        let zero = encode(&Value::Map(vec![(
            Value::Text("rc".into()),
            Value::Integer(0.into()),
        )]))
        .unwrap();
        assert!(check_rc(&zero).is_ok());

        let empty = encode(&Value::Map(vec![])).unwrap();
        assert!(check_rc(&empty).is_ok());
    }

    #[test]
    fn check_rc_rejects_nonzero() {
        let err = encode(&Value::Map(vec![(
            Value::Text("rc".into()),
            Value::Integer(3.into()),
        )]))
        .unwrap();
        let result = check_rc(&err);
        assert!(matches!(result, Err(Error::Mgmt(e)) if e.code == 3));
    }
}
