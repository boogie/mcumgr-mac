//! Terminal presentation helpers: colours, status symbols, spinners, progress
//! bars, and table builders for a tidy, modern CLI look.
//!
//! Colours are emitted only when the stream is a terminal (handled by
//! [`console`]) and tables degrade gracefully when piped.

use std::time::Duration;

use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

/// Print a green success line to stderr.
pub fn success(msg: impl std::fmt::Display) {
    eprintln!("{} {}", style("✓").green().bold(), msg);
}

/// Print a cyan status line to stderr.
pub fn status(msg: impl std::fmt::Display) {
    eprintln!("{} {}", style("›").cyan().bold(), msg);
}

/// Print a yellow warning line to stderr.
pub fn warn(msg: impl std::fmt::Display) {
    eprintln!("{} {}", style("!").yellow().bold(), style(msg).yellow());
}

/// Format an error for the top-level handler (red, with a cross).
pub fn error_prefix() -> String {
    style("✗ error:").red().bold().to_string()
}

/// A fresh table with rounded borders and dynamic column sizing.
pub fn table() -> Table {
    let mut table = Table::new();
    table
        .load_preset(comfy_table::presets::UTF8_FULL)
        .apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table
}

/// Build bold, cyan header cells.
pub fn header<const N: usize>(labels: [&str; N]) -> Vec<Cell> {
    labels
        .into_iter()
        .map(|l| Cell::new(l).add_attribute(Attribute::Bold).fg(Color::Cyan))
        .collect()
}

fn signal_level(rssi: i16) -> u8 {
    match rssi {
        r if r >= -55 => 4,
        r if r >= -67 => 3,
        r if r >= -78 => 2,
        r if r >= -90 => 1,
        _ => 0,
    }
}

/// A coloured 4-segment signal meter cell from an RSSI value.
pub fn signal_cell(rssi: Option<i16>) -> Cell {
    let Some(rssi) = rssi else {
        return Cell::new("▱▱▱▱   n/a").fg(Color::DarkGrey);
    };
    let level = signal_level(rssi);
    let bars: String = (0..4u8)
        .map(|i| if i < level { '▰' } else { '▱' })
        .collect();
    let color = match level {
        3 | 4 => Color::Green,
        2 => Color::Yellow,
        _ => Color::Red,
    };
    Cell::new(format!("{bars} {rssi:>4} dBm")).fg(color)
}

/// A spinner for indeterminate work (scanning, connecting). Writes to stderr.
pub fn spinner(msg: impl Into<String>) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✓"]),
    );
    pb.enable_steady_tick(Duration::from_millis(80));
    pb.set_message(msg.into());
    pb
}

/// A styled byte-progress bar for firmware upload.
pub fn upload_bar(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} {prefix:.bold.cyan} {wide_bar:.cyan/blue} \
             {bytes:>9}/{total_bytes} {percent:>3}% • {binary_bytes_per_sec} • ETA {eta}",
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏ ")
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );
    pb.set_prefix("uploading");
    pb
}
