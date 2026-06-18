//! Command dispatch and the shared "resolve device and open a session" helper.

mod discover;
mod image;
mod os;

use std::time::Duration;

use anyhow::Result;
use console::style;
use time::OffsetDateTime;

use mcumgr_mac::cache::DeviceCache;

use crate::cli::{Cli, Command, GlobalOpts};
use crate::transport::{self, ResolveOptions, SmpSession};
use crate::ui;

/// Run the parsed CLI.
pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Discover => discover::run(&cli.global).await,
        Command::Echo { text } => os::echo(&cli.global, &text).await,
        Command::Reset => os::reset(&cli.global).await,
        Command::Image { command } => image::run(&cli.global, command).await,
    }
}

/// Resolve the target device, connect, and update the cache on success.
///
/// This is the single entry point used by every command that needs a live
/// connection.
pub async fn open_session(global: &GlobalOpts) -> Result<SmpSession> {
    let adapter = transport::adapter().await?;

    let cache_path = if global.no_cache {
        None
    } else {
        DeviceCache::default_path().ok()
    };
    let mut cache = match &cache_path {
        Some(path) => DeviceCache::load_from(path).unwrap_or_default(),
        None => DeviceCache::new(),
    };

    let opts = ResolveOptions {
        name: global.name.clone(),
        id: global.id.clone(),
        all_devices: !global.smp_devices,
        scan_secs: global.scan_secs,
        use_cache: !global.no_cache,
    };

    let target = match (&global.id, &global.name) {
        (Some(id), _) => format!("id {id}"),
        (_, Some(name)) => format!("\u{201c}{name}\u{201d}"),
        _ => "your usual device".to_string(),
    };
    let sp = ui::spinner(format!(
        "Scanning for {target} (up to {}s)\u{2026}",
        global.scan_secs
    ));
    let resolved = transport::resolve_peripheral(&adapter, &opts, &cache).await;
    sp.finish_and_clear();
    let peripheral = resolved?;

    let sp = ui::spinner("Connecting\u{2026}");
    let connected = SmpSession::connect(peripheral, Duration::from_secs(global.timeout)).await;
    sp.finish_and_clear();
    let session = connected?;

    let id = session.id();
    let name = session.name().await;

    if let Some(path) = &cache_path {
        cache.record_success(&id, name.as_deref(), OffsetDateTime::now_utc());
        if let Err(e) = cache.save_to(path) {
            ui::warn(format!("could not update device cache: {e}"));
        }
    }

    ui::success(format!(
        "Connected to {} {}",
        style(name.as_deref().unwrap_or("(unnamed)")).bold(),
        style(format!("[{id}]")).dim()
    ));
    Ok(session)
}
