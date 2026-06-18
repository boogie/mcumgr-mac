//! Image-group commands: list, upload, test, confirm, erase, and local info.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use comfy_table::{Attribute, Cell, Color};
use console::style;
use indicatif::ProgressBar;
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
        ImageCommand::Upload {
            file,
            slot,
            chunk,
            erase,
            window,
            fast,
        } => upload(global, &file, slot, chunk, erase, window, fast).await,
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

/// Default chunk sizes and window depth.
const DEFAULT_CHUNK: usize = 128;
const FAST_CHUNK: usize = 480;
const FAST_WINDOW: usize = 8;
const MIN_CHUNK: usize = 64;

/// A recoverable upload failure that the orchestrator can downgrade around.
#[derive(Debug)]
enum UploadIssue {
    /// The chunk did not fit the device's MTU (first chunk never acked).
    MtuTooLarge,
    /// The device cannot accept writes to the slot until it is erased.
    SlotNeedsErase,
}

impl std::fmt::Display for UploadIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UploadIssue::MtuTooLarge => write!(f, "chunk too large for the device's MTU"),
            UploadIssue::SlotNeedsErase => write!(f, "the target slot needs erasing"),
        }
    }
}

impl std::error::Error for UploadIssue {}

/// Upload a firmware image, optionally erasing the slot first and pipelining,
/// auto-downgrading any tuning the device cannot handle.
async fn upload(
    global: &GlobalOpts,
    file: &Path,
    slot: u8,
    chunk: Option<usize>,
    erase: bool,
    window: Option<usize>,
    fast: bool,
) -> Result<()> {
    let chunk = chunk.unwrap_or(if fast { FAST_CHUNK } else { DEFAULT_CHUNK });
    let window = window.unwrap_or(if fast { FAST_WINDOW } else { 1 });
    if chunk == 0 {
        bail!("--chunk must be greater than zero");
    }
    if window == 0 {
        bail!("--window must be at least 1");
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

    if erase {
        erase_slot(&mut session).await?;
        ui::success("Erased target slot");
    }

    let progress = ui::upload_bar(total as u64);
    upload_with_fallback(
        &mut session,
        &data,
        &sha,
        chunk,
        window,
        erase,
        Duration::from_secs(5),
        &progress,
    )
    .await?;

    progress.finish_and_clear();
    ui::success(format!("Uploaded {total} bytes"));
    ui::status("Run `image confirm` (and `reset`) to boot the new image.");
    session.disconnect().await;
    Ok(())
}

/// Erase the secondary image slot (the upload target).
async fn erase_slot(session: &mut SmpSession) -> Result<()> {
    let payload = messages::encode(&EraseRequest { slot: 1 })?;
    let response = session
        .request(Op::Write, group::IMAGE, image_cmd::ERASE, &payload)
        .await
        .context("erasing the slot")?;
    messages::check_rc(&response)?;
    Ok(())
}

/// Run the upload, downgrading individual tunings the device rejects until it
/// succeeds: pipelining → sequential, oversized chunk → smaller chunk, and an
/// un-erased slot → erase-then-retry. Each downgrade restarts from offset 0,
/// which the device accepts (the first chunk resets its upload state).
#[allow(clippy::too_many_arguments)]
async fn upload_with_fallback(
    session: &mut SmpSession,
    data: &[u8],
    sha: &[u8],
    mut chunk: usize,
    mut window: usize,
    mut erased: bool,
    chunk_timeout: Duration,
    progress: &ProgressBar,
) -> Result<()> {
    loop {
        if window > 1 {
            match upload_windowed(session, data, sha, chunk, window, chunk_timeout, progress).await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    progress.println(format!(
                        "  pipelined upload didn't hold ({e}); falling back to sequential"
                    ));
                    session.reset_buffer();
                    window = 1;
                    continue;
                }
            }
        }

        match upload_sequential(session, data, sha, chunk, chunk_timeout, progress).await {
            Ok(()) => return Ok(()),
            Err(e) => match e.downcast_ref::<UploadIssue>() {
                Some(UploadIssue::MtuTooLarge) if chunk > MIN_CHUNK => {
                    let smaller = (chunk / 2).max(MIN_CHUNK);
                    progress.println(format!(
                        "  chunk {chunk} too large for this device; retrying at {smaller}"
                    ));
                    session.reset_buffer();
                    chunk = smaller;
                    continue;
                }
                Some(UploadIssue::SlotNeedsErase) if !erased => {
                    progress.println("  slot needs erasing; erasing and retrying");
                    erase_slot(session).await?;
                    erased = true;
                    session.reset_buffer();
                    continue;
                }
                _ => return Err(e),
            },
        }
    }
}

/// Build the upload payload for the chunk starting at `offset`.
fn upload_chunk(data: &[u8], sha: &[u8], offset: usize, chunk: usize) -> Result<Vec<u8>> {
    let end = (offset + chunk).min(data.len());
    let piece = data[offset..end].to_vec();
    let payload = if offset == 0 {
        messages::encode(&UploadChunk::first(piece, data.len() as u32, sha.to_vec()))?
    } else {
        messages::encode(&UploadChunk::next(piece, offset as u32))?
    };
    Ok(payload)
}

/// Strictly sequential upload: send a chunk, wait for its ack, repeat — with a
/// per-chunk retry. This is the most compatible mode. Signals [`UploadIssue`]s
/// the orchestrator can downgrade around.
async fn upload_sequential(
    session: &mut SmpSession,
    data: &[u8],
    sha: &[u8],
    chunk: usize,
    chunk_timeout: Duration,
    progress: &ProgressBar,
) -> Result<()> {
    let total = data.len();
    let mut offset: usize = 0;
    let mut stalls = 0u32;
    let max_stalls = 8u32;

    while offset < total {
        let payload = upload_chunk(data, sha, offset, chunk)?;

        // The first chunk doubles as an MTU probe: keep it short so the
        // orchestrator can downgrade the chunk size quickly if it never lands.
        let (timeout, max_attempts) = if offset == 0 {
            (chunk_timeout.min(Duration::from_secs(3)), 2u32)
        } else {
            (chunk_timeout, 6u32)
        };

        let mut attempt = 0u32;
        let response = loop {
            match session
                .request_with_timeout(Op::Write, group::IMAGE, image_cmd::UPLOAD, &payload, timeout)
                .await
            {
                Ok(resp) => break resp,
                Err(e) => {
                    attempt += 1;
                    if attempt >= max_attempts {
                        if offset == 0 {
                            // Nothing acknowledged yet — most likely the chunk
                            // exceeds the device's MTU.
                            return Err(UploadIssue::MtuTooLarge.into());
                        }
                        return Err(e).with_context(|| {
                            format!("upload stalled at offset {offset} after {attempt} attempts")
                        });
                    }
                    progress.println(format!(
                        "  retrying chunk at offset {offset} (attempt {}/{max_attempts}): {e}",
                        attempt + 1
                    ));
                    session.reset_buffer();
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        };

        match messages::check_rc(&response) {
            Ok(()) => {}
            // A busy / bad-state slot usually just needs erasing.
            Err(mcumgr_mac::Error::Mgmt(m)) if matches!(m.code, 2 | 6 | 10) => {
                return Err(UploadIssue::SlotNeedsErase.into());
            }
            Err(e) => return Err(e.into()),
        }

        let parsed: UploadResponse = messages::decode(&response)?;
        let next = parsed
            .off
            .ok_or_else(|| anyhow!("device did not report the next offset"))? as usize;
        if next > offset {
            stalls = 0;
        } else {
            stalls += 1;
            if stalls >= max_stalls {
                return Err(UploadIssue::SlotNeedsErase.into());
            }
        }
        offset = next;
        progress.set_position(offset.min(total) as u64);
    }
    Ok(())
}

/// Pipelined upload: keep `window` chunks in flight to hide round-trip latency.
/// Faster on devices that accept back-to-back writes; resyncs from the device's
/// acknowledged offset if it falls behind what we have sent.
async fn upload_windowed(
    session: &mut SmpSession,
    data: &[u8],
    sha: &[u8],
    chunk: usize,
    window: usize,
    chunk_timeout: Duration,
    progress: &ProgressBar,
) -> Result<()> {
    let total = data.len();
    let mut sent: usize = 0; // next offset to hand to the device
    let mut acked: usize = 0; // highest offset the device has confirmed
    let mut inflight: usize = 0;
    let mut stalls = 0u32;
    let max_stalls = window as u32 * 4 + 8;

    while acked < total {
        while inflight < window && sent < total {
            let payload = upload_chunk(data, sha, sent, chunk)?;
            session
                .send_request(Op::Write, group::IMAGE, image_cmd::UPLOAD, &payload)
                .await?;
            sent = (sent + chunk).min(total);
            inflight += 1;
        }

        let (_seq, payload) = session
            .recv_response(chunk_timeout)
            .await
            .with_context(|| format!("waiting for upload ack ({acked}/{total} bytes)"))?;
        messages::check_rc(&payload)?;
        inflight = inflight.saturating_sub(1);

        let parsed: UploadResponse = messages::decode(&payload)?;
        let off = parsed
            .off
            .ok_or_else(|| anyhow!("device did not report the next offset"))? as usize;

        if off > acked {
            acked = off;
            stalls = 0;
        } else {
            stalls += 1;
            if stalls >= max_stalls {
                bail!(
                    "windowed upload stuck at offset {acked} (device keeps requesting {off}). \
                     Retry with --window 1, or add --erase."
                );
            }
        }

        // The device can only advance to the first offset it is missing, so if
        // its expected offset is behind what we have sent and the window has
        // drained, resend from there.
        if off < sent && inflight == 0 {
            sent = off;
        }
        progress.set_position(acked.min(total) as u64);
    }
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
