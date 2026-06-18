//! The `discover` command: scan and list nearby SMP devices.

use std::time::Duration;

use anyhow::Result;
use comfy_table::{Attribute, Cell, Color};

use mcumgr_mac::cache::DeviceCache;

use crate::cli::GlobalOpts;
use crate::transport;
use crate::ui;

/// Scan for devices and print them, annotated with cached connection counts.
pub async fn run(global: &GlobalOpts) -> Result<()> {
    let adapter = transport::adapter().await?;
    let cache = if global.no_cache {
        DeviceCache::new()
    } else {
        DeviceCache::default_path()
            .ok()
            .and_then(|p| DeviceCache::load_from(&p).ok())
            .unwrap_or_default()
    };

    let all_devices = !global.smp_devices;
    let kind = if all_devices {
        "all BLE devices"
    } else {
        "SMP devices"
    };
    let sp = ui::spinner(format!(
        "Scanning for {kind} (up to {}s)\u{2026}",
        global.scan_secs
    ));
    let mut devices =
        transport::discover(&adapter, Duration::from_secs(global.scan_secs), all_devices).await?;
    sp.finish_and_clear();

    if devices.is_empty() {
        ui::warn("No devices found.");
        return Ok(());
    }

    // Strongest signal first; devices with unknown RSSI sink to the bottom.
    devices.sort_by(|a, b| b.rssi.unwrap_or(i16::MIN).cmp(&a.rssi.unwrap_or(i16::MIN)));

    ui::success(format!("Found {} device(s)", devices.len()));

    let mut table = ui::table();
    table.set_header(ui::header(["", "Name", "Signal", "Identifier", "Seen"]));
    for d in &devices {
        let count = cache.connect_count(&d.id);
        let dot_color = signal_color(d.rssi);
        let seen = if count > 0 {
            Cell::new(format!("\u{2605} {count}")).fg(Color::Yellow)
        } else {
            Cell::new("\u{2014}").fg(Color::DarkGrey)
        };
        table.add_row(vec![
            Cell::new("\u{25cf}").fg(dot_color),
            Cell::new(d.name.as_deref().unwrap_or("(unnamed)")).add_attribute(Attribute::Bold),
            ui::signal_cell(d.rssi),
            Cell::new(&d.id).fg(Color::DarkGrey),
            seen,
        ]);
    }
    println!("{table}");
    Ok(())
}

fn signal_color(rssi: Option<i16>) -> Color {
    match rssi {
        Some(r) if r >= -67 => Color::Green,
        Some(r) if r >= -78 => Color::Yellow,
        Some(_) => Color::Red,
        None => Color::DarkGrey,
    }
}
