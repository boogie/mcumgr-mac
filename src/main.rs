//! `mcumgr-mac`: a command-line tool for managing MCUmgr / SMP devices over
//! Bluetooth Low Energy.

mod cli;
mod commands;
mod help;
mod transport;
mod ui;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use cli::Cli;

#[tokio::main]
async fn main() {
    // Show one consistent, grouped help for any -h/--help (at any level) and for
    // no arguments, instead of clap's per-subcommand help.
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().is_none() || std::env::args().any(|a| a == "-h" || a == "--help") {
        help::print();
        return;
    }

    let cli = Cli::parse();
    init_tracing(cli.global.verbose);

    if let Err(err) = commands::run(cli).await {
        eprintln!("{} {err:#}", ui::error_prefix());
        std::process::exit(1);
    }
}

/// Initialise diagnostic logging. Verbose mode lowers the level and honours
/// `RUST_LOG`; otherwise only warnings and errors are shown.
fn init_tracing(verbose: bool) {
    // btleplug logs a benign "SendError { Disconnected }" when we disconnect;
    // keep it (and other dependency noise) out of normal output.
    let default = if verbose {
        "debug,btleplug=warn"
    } else {
        "warn,btleplug=off"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();
}
