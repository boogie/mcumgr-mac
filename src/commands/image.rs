//! Image-group commands: list, upload, test, confirm, erase, and local info.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use comfy_table::{Attribute, Cell, Color};
use console::style;
use sha2::{Digest, Sha256};

use mcumgr_mac::image as image_file;
use mcumgr_mac::smp::groups::{group, image as image_cmd};
use mcumgr_mac::smp::messages::{
    self, EraseRequest, ImageSlot, ImageStateResponse, ImageStateWrite, UploadChunk, UploadResponse,
};
use mcumgr_mac::smp::Op;

use crate::cli::{GlobalOpts, ImageCommand};
use crate::commands::open_session;
use crate::transport::SmpSession;
use crate::ui;

/// Dispatch an `image` subcommand.
pub async fn run(global: &GlobalOpts, command: ImageCommand) -> Result<()> {
    match command {
        ImageCommand::List => list(global).await,
        ImageCommand::Upload { file, slot, chunk } => upload(global, &file, slot, chunk).await,
        ImageCommand::Test { hash } => set_state(global, hash, false).await,
        ImageCommand::Confirm { hash } => set_state(global, hash, true).await,
        ImageCommand::Erase { slot } => erase(global, slot).await,
        ImageCommand::Info { file } => info(&file),
    }
}

/// Read and print the image state of all slots.
async fn list(global: &GlobalOpts) -> Result<()> {
    let mut session = open_session(global).await?;
    let state = read_state(&mut session).await?;
    session.disconnect().await;
    print_state(&state);
    Ok(())
}

/// Upload a firmware image, chunking it under the configured size.
async fn upload(global: &GlobalOpts, file: &Path, slot: u8, chunk: usize) -> Result<()> {
    if chunk == 0 {
        bail!("--chunk must be greater than zero");
    }
    let data =
        std::fs::read(file).with_context(|| format!("reading firmware file {}", file.display()))?;

    // Validate locally and show what we are about to flash.
    match image_file::parse(&data) {
        Ok(info) => {
            ui::status(format!(
                "Image {} \u{2022} {} bytes \u{2022} {}",
                style(&info.version).bold(),
                info.image_size,
                match info.hash_valid {
                    Some(true) => style("embedded hash OK").green().to_string(),
                    Some(false) => style("embedded hash MISMATCH").red().to_string(),
                    None => style("no embedded hash").dim().to_string(),
                }
            ));
        }
        Err(e) => ui::warn(format!("{e}; uploading raw bytes anyway")),
    }

    if slot != 0 {
        ui::warn(format!(
            "target slot {slot} requested (most firmware uploads to slot 1 regardless)"
        ));
    }

    let sha = Sha256::digest(&data).to_vec();
    let total = data.len();

    let mut session = open_session(global).await?;

    let progress = ui::upload_bar(total as u64);

    // A single dropped BLE notification must not abort a multi-minute upload,
    // so each chunk is retried with a short timeout before giving up.
    let chunk_timeout = Duration::from_secs(5);
    let max_attempts = 6u32;

    let mut offset: usize = 0;
    while offset < total {
        let end = (offset + chunk).min(total);
        let piece = data[offset..end].to_vec();
        let payload = if offset == 0 {
            messages::encode(&UploadChunk::first(piece, total as u32, sha.clone()))?
        } else {
            messages::encode(&UploadChunk::next(piece, offset as u32))?
        };

        let mut attempt = 0u32;
        let response = loop {
            match session
                .request_with_timeout(
                    Op::Write,
                    group::IMAGE,
                    image_cmd::UPLOAD,
                    &payload,
                    chunk_timeout,
                )
                .await
            {
                Ok(resp) => break resp,
                Err(e) => {
                    attempt += 1;
                    if attempt >= max_attempts {
                        return Err(e).with_context(|| {
                            format!("upload stalled at offset {offset} after {attempt} attempts")
                        });
                    }
                    progress.println(format!(
                        "  retrying chunk at offset {offset} (attempt {}/{max_attempts}): {e}",
                        attempt + 1
                    ));
                    // Drop any half-received frame and let the device settle.
                    session.reset_buffer();
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        };
        messages::check_rc(&response)?;

        let parsed: UploadResponse = messages::decode(&response)?;
        offset = parsed
            .off
            .ok_or_else(|| anyhow!("device did not report the next offset"))?
            as usize;
        progress.set_position(offset.min(total) as u64);
    }

    progress.finish_and_clear();
    ui::success(format!("Uploaded {total} bytes"));
    ui::status("Run `image confirm` (and `reset`) to boot the new image.");
    session.disconnect().await;
    Ok(())
}

/// Mark an image for test (`confirm = false`) or confirm it (`confirm = true`).
async fn set_state(global: &GlobalOpts, hash: Option<String>, confirm: bool) -> Result<()> {
    let mut session = open_session(global).await?;

    let hash = match hash {
        Some(hex) => hex::decode(hex.trim()).context("parsing hash argument as hex")?,
        None => {
            let state = read_state(&mut session).await?;
            select_default_target(&state, confirm)?
        }
    };

    let payload = messages::encode(&ImageStateWrite {
        hash: hash.clone(),
        confirm,
    })?;
    let response = session
        .request(Op::Write, group::IMAGE, image_cmd::STATE, &payload)
        .await?;
    messages::check_rc(&response)?;

    let verb = if confirm {
        "confirmed"
    } else {
        "marked for test"
    };
    ui::success(format!("Image {} {verb}", style(hex::encode(&hash)).dim()));

    // The response echoes the new state; show it for confirmation.
    if let Ok(state) = messages::decode::<ImageStateResponse>(&response) {
        if !state.images.is_empty() {
            print_state(&state);
        }
    }
    session.disconnect().await;
    Ok(())
}

/// Erase an image slot.
async fn erase(global: &GlobalOpts, slot: u8) -> Result<()> {
    let mut session = open_session(global).await?;
    let payload = messages::encode(&EraseRequest { slot })?;
    let response = session
        .request(Op::Write, group::IMAGE, image_cmd::ERASE, &payload)
        .await?;
    messages::check_rc(&response)?;
    ui::success(format!("Slot {slot} erased"));
    session.disconnect().await;
    Ok(())
}

/// Parse and print info about a local MCUboot image file (no Bluetooth).
fn info(file: &Path) -> Result<()> {
    let data =
        std::fs::read(file).with_context(|| format!("reading image file {}", file.display()))?;
    let info = image_file::parse(&data)?;
    ui::success("Valid MCUboot image");

    let (hash_text, hash_color) = match info.hash_valid {
        Some(true) => ("present, valid", Color::Green),
        Some(false) => ("present, MISMATCH", Color::Red),
        None => ("not present", Color::DarkGrey),
    };

    let field = |name: &str| Cell::new(name).fg(Color::Cyan);
    let mut table = ui::table();
    table.set_header(ui::header(["Field", "Value"]));
    table.add_row(vec![field("File"), Cell::new(file.display())]);
    table.add_row(vec![
        field("Version"),
        Cell::new(&info.version).add_attribute(Attribute::Bold),
    ]);
    table.add_row(vec![
        field("Image size"),
        Cell::new(format!("{} bytes", info.image_size)),
    ]);
    table.add_row(vec![
        field("Header size"),
        Cell::new(format!("{} bytes", info.header_size)),
    ]);
    table.add_row(vec![
        field("Flags"),
        Cell::new(format!("0x{:08x}", info.flags)),
    ]);
    table.add_row(vec![field("SHA-256"), Cell::new(info.hash_hex())]);
    table.add_row(vec![
        field("Embedded hash"),
        Cell::new(hash_text).fg(hash_color),
    ]);
    println!("{table}");
    Ok(())
}

/// Read the image state from a connected device.
async fn read_state(session: &mut SmpSession) -> Result<ImageStateResponse> {
    let response = session
        .request(Op::Read, group::IMAGE, image_cmd::STATE, &[])
        .await?;
    messages::check_rc(&response)?;
    messages::decode(&response).context("decoding image state")
}

/// Choose a default target image when the user did not give a hash.
///
/// For `confirm`, target the unconfirmed image; for `test`, the non-active one.
fn select_default_target(state: &ImageStateResponse, confirm: bool) -> Result<Vec<u8>> {
    let (label, matches): (&str, Vec<&ImageSlot>) = if confirm {
        (
            "unconfirmed",
            state.images.iter().filter(|s| !s.confirmed).collect(),
        )
    } else {
        (
            "non-active",
            state.images.iter().filter(|s| !s.active).collect(),
        )
    };

    match matches.as_slice() {
        [] => bail!("no {label} image found; pass an explicit hash"),
        [only] => Ok(only.hash.clone()),
        many => {
            ui::warn(format!(
                "{} {label} images found; targeting slot {}",
                many.len(),
                many[0].slot
            ));
            Ok(many[0].hash.clone())
        }
    }
}

/// Pretty-print image state as a table.
fn print_state(state: &ImageStateResponse) {
    if state.images.is_empty() {
        ui::warn("No images reported.");
        return;
    }
    let mut table = ui::table();
    table.set_header(ui::header(["Slot", "Version", "Flags", "Hash"]));
    for img in &state.images {
        table.add_row(vec![
            Cell::new(img.slot).add_attribute(Attribute::Bold),
            Cell::new(&img.version),
            flags_cell(img),
            Cell::new(hex::encode(&img.hash)).fg(Color::DarkGrey),
        ]);
    }
    println!("{table}");
}

/// Render an image slot's flags as a single coloured cell.
fn flags_cell(img: &ImageSlot) -> Cell {
    let mut flags = Vec::new();
    if img.active {
        flags.push("active");
    }
    if img.confirmed {
        flags.push("confirmed");
    }
    if img.pending {
        flags.push("pending");
    }
    if img.permanent {
        flags.push("permanent");
    }
    if img.bootable {
        flags.push("bootable");
    }
    if flags.is_empty() {
        Cell::new("\u{2014}").fg(Color::DarkGrey)
    } else {
        let color = if img.active {
            Color::Green
        } else {
            Color::Yellow
        };
        Cell::new(flags.join(", ")).fg(color)
    }
}
