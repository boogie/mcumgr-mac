//! Core library for `mcumgr-mac`: a CLI that manages MCUmgr / SMP devices
//! over Bluetooth Low Energy.
//!
//! This crate contains the transport-agnostic, unit-testable core:
//!
//! - [`smp`] — the Simple Management Protocol: framing, groups, return codes,
//!   and typed request/response payloads.
//! - [`image`] — parsing and validation of MCUboot firmware images.
//! - [`cache`] — a persistent cache of previously connected devices used to
//!   speed up reconnection.
//!
//! The Bluetooth transport and the command-line surface live in the binary
//! crate, which depends on these building blocks.

pub mod cache;
pub mod error;
pub mod image;
pub mod smp;

pub use error::{Error, Result};
