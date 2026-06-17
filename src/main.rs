//! `mcumgr-mac`: a command-line tool for managing MCUmgr / SMP devices over
//! Bluetooth Low Energy.

mod cli;
mod commands;
mod transport;
mod ui;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use cli::Cli;

#[tokio::main]
async fn main() {
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
    let default = if verbose { "debug" } else { "warn" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();
}
