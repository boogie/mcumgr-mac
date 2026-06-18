//! Image-group commands: list, upload, test, confirm, erase, and local info.

use std::path::Path;
use std::time::{Duration, Instant};

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

/// Default chunk size and the `--fast` presets.
const DEFAULT_CHUNK: usize = 128;
/// `--fast` uses a large chunk (more bytes per ack on a DLE link) plus a modest
/// pipeline; the MTU auto-downgrade trims the chunk on smaller-MTU devices.
const FAST_CHUNK: usize = 432;
const FAST_WINDOW: usize = 4;
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
    let new_version = match image_file::parse(&data) {
        Ok(info) => {
            ui::status(format!(
                "Image v{} \u{2022} {} bytes \u{2022} {}",
                style(&info.version).bold(),
                info.image_size,
                match info.hash_valid {
                    Some(true) => style("embedded hash OK").green().to_string(),
                    Some(false) => style("embedded hash MISMATCH").red().to_string(),
                    None => style("no embedded hash").dim().to_string(),
                }
            ));
            Some(info.version)
        }
        Err(e) => {
            ui::warn(format!("{e}; uploading raw bytes anyway"));
            None
        }
    };

    if slot != 0 {
        ui::warn(format!(
            "target slot {slot} requested (most firmware uploads to slot 1 regardless)"
        ));
    }

    let sha = Sha256::digest(&data).to_vec();
    let total = data.len();

    let mut session = open_session(global).await?;

    // Report the version transition, reading the currently running image.
    if let Some(new_ver) = &new_version {
        match read_state(&mut session).await {
            Ok(state) => match state.images.iter().find(|s| s.active) {
                Some(active) if !active.version.is_empty() => {
                    ui::status(format!("Updating from v{} to v{new_ver}", active.version))
                }
                _ => ui::status(format!("Installing v{new_ver}")),
            },
            Err(_) => ui::status(format!("Installing v{new_ver}")),
        }
    }

    if erase {
        session = robust_erase(global, session, 1).await?;
    }

    let progress = ui::upload_bar(total as u64);
    let stats = upload_with_fallback(
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

    let secs = stats.elapsed_secs();
    let avg = if secs > 0.0 {
        (total as f64 / 1024.0) / secs
    } else {
        0.0
    };
    let peak = stats.max_kbps;
    let low = if stats.min_kbps.is_finite() {
        stats.min_kbps
    } else {
        avg
    };
    let uploaded = match &new_version {
        Some(v) => format!("v{v} ({total} bytes)"),
        None => format!("{total} bytes"),
    };
    ui::success(format!(
        "Uploaded {uploaded} in {secs:.1}s \u{2022} avg {avg:.2} KB/s"
    ));
    ui::status(format!(
        "throughput: avg {avg:.2} \u{2022} peak {peak:.2} \u{2022} min {low:.2} KB/s"
    ));
    ui::status("Run `image confirm` (and `reset`) to boot the new image.");
    session.disconnect().await;
    Ok(())
}

/// Send an erase and wait (with a generous timeout) for the ack, without the
/// reconnect handling — used mid-upload by the auto-downgrade path.
async fn erase_slot_simple(session: &mut SmpSession, slot: u8) -> Result<()> {
    let payload = messages::encode(&EraseRequest { slot })?;
    let response = session
        .request_with_timeout(
            Op::Write,
            group::IMAGE,
            image_cmd::ERASE,
            &payload,
            Duration::from_secs(60),
        )
        .await?;
    messages::check_rc(&response)?;
    Ok(())
}

/// Erase `slot`, tolerating the BLE link drop that a long flash erase can cause:
/// erasing an occupied slot can block the device's radio past the supervision
/// timeout, dropping the connection even though the erase completes. When that
/// happens we reconnect (the device re-advertises once done) and confirm the
/// slot is actually clear. Returns a live session.
async fn robust_erase(
    global: &GlobalOpts,
    mut session: SmpSession,
    slot: u8,
) -> Result<SmpSession> {
    let payload = messages::encode(&EraseRequest { slot })?;
    match session
        .request_watching_link(
            Op::Write,
            group::IMAGE,
            image_cmd::ERASE,
            &payload,
            Duration::from_secs(60),
        )
        .await
    {
        Ok(response) => {
            messages::check_rc(&response)?;
            ui::success(format!("Erased slot {slot}"));
            Ok(session)
        }
        // A transport error here is almost always the erase blocking the link
        // (and, with reset-on-disconnect firmware, rebooting the device).
        Err(_) => {
            ui::warn(
                "erase blocked the BLE link (the device is busy and will reboot); \
                 reconnecting\u{2026}",
            );
            // Fully release the old peripheral and CoreBluetooth manager. An
            // in-process re-scan only re-discovers the rebooted device with a
            // *fresh* manager (CoreBluetooth won't re-report a device we still
            // hold), and the old handle cannot reconnect — so we rebuild from
            // scratch, exactly like a new invocation would.
            drop(session);
            let mut session = reconnect_fresh(global).await?;
            let state = read_state(&mut session).await?;
            if state.images.iter().any(|s| s.slot == slot as u32) {
                bail!("slot {slot} still holds an image after the erase");
            }
            ui::success(format!("Erased slot {slot} (after reconnect)"));
            Ok(session)
        }
    }
}

/// Rebuild a connection after the device dropped us and rebooted: retry a fresh
/// scan + connect across a generous budget while it comes back up. Each attempt
/// is time-bounded so a stuck connect can't hang the whole flow.
async fn reconnect_fresh(global: &GlobalOpts) -> Result<SmpSession> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut last_err = anyhow!("device did not come back after the erase");
    loop {
        match tokio::time::timeout(Duration::from_secs(45), open_session(global)).await {
            Ok(Ok(session)) => return Ok(session),
            Ok(Err(e)) => last_err = e,
            Err(_) => last_err = anyhow!("reconnect attempt timed out"),
        }
        if Instant::now() >= deadline {
            return Err(last_err).context("reconnecting after the erase dropped the link");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
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
) -> Result<UploadStats> {
    loop {
        if window > 1 {
            match upload_windowed(session, data, sha, chunk, window, chunk_timeout, progress).await
            {
                Ok(stats) => return Ok(stats),
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
            Ok(stats) => return Ok(stats),
            Err(e) => match e.downcast_ref::<UploadIssue>() {
                Some(UploadIssue::MtuTooLarge) if chunk > MIN_CHUNK => {
                    // Step down gently (×¾) so we land near the MTU limit rather
                    // than overshooting by halving.
                    let smaller = (chunk * 3 / 4).max(MIN_CHUNK);
                    progress.println(format!(
                        "  chunk {chunk} too large for this device; retrying at {smaller}"
                    ));
                    session.reset_buffer();
                    chunk = smaller;
                    continue;
                }
                Some(UploadIssue::SlotNeedsErase) if !erased => {
                    progress.println("  slot needs erasing; erasing and retrying");
                    erase_slot_simple(session, 1).await?;
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

/// Tracks upload timing and throughput, and (when the progress bar is hidden,
/// e.g. redirected output) prints a milestone line every 5% showing the
/// *instantaneous* rate over that interval. Also records min/peak instantaneous
/// rate and total elapsed time for the closing summary.
struct UploadStats {
    start: std::time::Instant,
    hidden: bool,
    last_step: usize,
    last_done: usize,
    last_secs: f64,
    min_kbps: f64,
    max_kbps: f64,
}

impl UploadStats {
    fn new(progress: &ProgressBar) -> Self {
        Self {
            start: std::time::Instant::now(),
            hidden: progress.is_hidden(),
            last_step: 0,
            last_done: 0,
            last_secs: 0.0,
            min_kbps: f64::INFINITY,
            max_kbps: 0.0,
        }
    }

    /// Record progress; emit a milestone line on each new 5% step.
    fn tick(&mut self, done: usize, total: usize) {
        let step = (done * 20).checked_div(total).unwrap_or(20); // 5% steps
        if step <= self.last_step {
            return;
        }
        self.last_step = step;
        let secs = self.start.elapsed().as_secs_f64();
        let dbytes = done.saturating_sub(self.last_done) as f64;
        let dsecs = (secs - self.last_secs).max(1e-6);
        let inst = (dbytes / 1024.0) / dsecs; // instantaneous KB/s over this interval
        self.min_kbps = self.min_kbps.min(inst);
        self.max_kbps = self.max_kbps.max(inst);
        if self.hidden {
            eprintln!(
                "  {:>3}%  {done}/{total} bytes  {secs:.0}s  {inst:.2} KB/s",
                step * 5
            );
        }
        self.last_done = done;
        self.last_secs = secs;
    }

    fn elapsed_secs(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
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
) -> Result<UploadStats> {
    let total = data.len();
    let mut offset: usize = 0;
    let mut stalls = 0u32;
    let max_stalls = 8u32;
    let mut stats = UploadStats::new(progress);

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
                .request_with_timeout(
                    Op::Write,
                    group::IMAGE,
                    image_cmd::UPLOAD,
                    &payload,
                    timeout,
                )
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
            .ok_or_else(|| anyhow!("device did not report the next offset"))?
            as usize;
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
        stats.tick(offset.min(total), total);
    }
    Ok(stats)
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
) -> Result<UploadStats> {
    let total = data.len();
    let mut sent: usize = 0; // next offset to hand to the device
    let mut acked: usize = 0; // highest offset the device has confirmed
    let mut stats = UploadStats::new(progress);

    // If the window is too deep the device drops chunks and keeps re-reporting
    // the first offset it is missing. Detect that and resync from there rather
    // than flooding past the gap (which collapses throughput).
    let mut no_progress = 0u32;
    let resync_after = window as u32 + 4;
    let mut total_stalls = 0u32;
    let max_total_stalls = window as u32 * 16 + 32;

    while acked < total {
        // Keep at most `window` chunks of *unacknowledged* data in flight,
        // measured from `acked` so a stall cannot run `sent` away from the gap.
        while sent < total && sent - acked < window * chunk {
            let payload = upload_chunk(data, sha, sent, chunk)?;
            session
                .send_request(Op::Write, group::IMAGE, image_cmd::UPLOAD, &payload)
                .await?;
            sent = (sent + chunk).min(total);
        }

        let (_seq, payload) = session
            .recv_response(chunk_timeout)
            .await
            .with_context(|| format!("waiting for upload ack ({acked}/{total} bytes)"))?;
        messages::check_rc(&payload)?;
        let parsed: UploadResponse = messages::decode(&payload)?;
        let off = parsed
            .off
            .ok_or_else(|| anyhow!("device did not report the next offset"))?
            as usize;

        if off > acked {
            acked = off;
            no_progress = 0;
        } else {
            no_progress += 1;
            total_stalls += 1;
            if total_stalls >= max_total_stalls {
                bail!("windowed upload stuck at offset {acked}");
            }
            // Once the in-flight acks are drained and the device still wants
            // `off`, rewind and resend from there.
            if no_progress >= resync_after {
                session.reset_buffer();
                sent = off;
                no_progress = 0;
            }
        }
        progress.set_position(acked.min(total) as u64);
        stats.tick(acked.min(total), total);
    }
    Ok(stats)
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
    let session = open_session(global).await?;
    let session = robust_erase(global, session, slot).await?;
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
        Cell::new(format!("v{}", info.version)).add_attribute(Attribute::Bold),
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
            Cell::new(format!("v{}", img.version)),
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
