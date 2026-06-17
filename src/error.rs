//! Crate-wide error type.

use thiserror::Error;

/// Errors produced by the mcumgr-mac core library.
#[derive(Debug, Error)]
pub enum Error {
    /// The SMP response frame was malformed or too short to parse.
    #[error("malformed SMP frame: {0}")]
    MalformedFrame(String),

    /// The device replied with a non-zero SMP management return code.
    #[error("device returned error: {0}")]
    Mgmt(#[from] crate::smp::MgmtError),

    /// CBOR payload could not be encoded.
    #[error("failed to encode CBOR payload: {0}")]
    CborEncode(String),

    /// CBOR payload could not be decoded.
    #[error("failed to decode CBOR payload: {0}")]
    CborDecode(String),

    /// The supplied firmware file is not a valid MCUboot image.
    #[error("invalid MCUboot image: {0}")]
    InvalidImage(String),

    /// The device cache file could not be read, parsed, or written.
    #[error("device cache error: {0}")]
    Cache(String),

    /// An underlying I/O operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Convenience alias for results in the core library.
pub type Result<T> = std::result::Result<T, Error>;
