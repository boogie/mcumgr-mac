//! Command-line interface definition.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Manage MCUmgr / SMP devices over Bluetooth Low Energy.
#[derive(Debug, Parser)]
#[command(name = "mcumgr-mac", version, about, long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalOpts,

    #[command(subcommand)]
    pub command: Command,
}

/// Options shared by every subcommand.
#[derive(Debug, Args)]
pub struct GlobalOpts {
    /// Connect to a device whose advertised name contains NAME (case-insensitive).
    #[arg(short = 'n', long, global = true)]
    pub name: Option<String>,

    /// Connect to a specific peripheral id (skips name/cache matching).
    #[arg(long, global = true)]
    pub id: Option<String>,

    /// Seconds to scan when resolving a device.
    #[arg(long, default_value_t = 5, global = true)]
    pub scan_secs: u64,

    /// Per-operation response timeout, in seconds.
    #[arg(long, default_value_t = 30, global = true)]
    pub timeout: u64,

    /// Scan all BLE devices. This is the default; the flag is accepted for clarity.
    #[arg(long, global = true)]
    pub all_devices: bool,

    /// Only consider devices advertising the SMP service.
    #[arg(long, global = true, conflicts_with = "all_devices")]
    pub smp_devices: bool,

    /// Do not read from or write to the device cache for this run.
    #[arg(long, global = true)]
    pub no_cache: bool,

    /// Enable verbose diagnostic logging.
    #[arg(short = 'v', long, global = true)]
    pub verbose: bool,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scan for and list nearby SMP-capable devices.
    Discover,

    /// Echo a string off the device (a connectivity smoke test).
    Echo {
        /// Text to send.
        text: String,
    },

    /// Reboot the device.
    Reset,

    /// Firmware image management.
    Image {
        #[command(subcommand)]
        command: ImageCommand,
    },
}

/// `image` subcommands.
#[derive(Debug, Subcommand)]
pub enum ImageCommand {
    /// List firmware image slots and their state.
    List,

    /// Upload a firmware image to the device.
    Upload {
        /// Path to the MCUboot image file.
        file: PathBuf,

        /// Target slot number.
        #[arg(long, default_value_t = 0)]
        slot: u8,

        /// Data bytes per upload chunk.
        #[arg(long, default_value_t = 128)]
        chunk: usize,
    },

    /// Mark an image for test on the next boot (defaults to the non-active image).
    Test {
        /// Image hash as hex. If omitted, the non-active image is used.
        hash: Option<String>,
    },

    /// Confirm an image permanently (defaults to the unconfirmed image).
    Confirm {
        /// Image hash as hex. If omitted, the unconfirmed image is used.
        hash: Option<String>,
    },

    /// Erase an image slot.
    Erase {
        /// Slot to erase (the secondary slot by default).
        #[arg(long, default_value_t = 1)]
        slot: u8,
    },

    /// Parse and validate a local MCUboot image file (no Bluetooth).
    Info {
        /// Path to the MCUboot image file.
        file: PathBuf,
    },
}
