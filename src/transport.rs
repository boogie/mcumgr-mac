//! Bluetooth Low Energy transport for SMP.
//!
//! Responsibilities:
//!
//! - [`adapter`] — obtain the host's default BLE adapter.
//! - [`discover`] — scan and list nearby SMP-capable devices.
//! - [`resolve_peripheral`] — pick the device to talk to, preferring cached
//!   devices (by connection frequency) and falling back to a scan.
//! - [`SmpSession`] — a connected session that exchanges SMP request/response
//!   pairs over the SMP characteristic.
//!
//! Note on the cache "fast path": CoreBluetooth (macOS) cannot connect to a
//! peripheral by identifier without first discovering it via a scan
//! (`add_peripheral` is unsupported). So rather than a literal zero-scan
//! reconnect, [`resolve_peripheral`] runs a scan that is biased by the cache
//! and exits the instant the preferred device appears.

use std::collections::HashSet;
use std::pin::Pin;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, ValueNotification,
    WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral, PeripheralId};
use futures::{Stream, StreamExt};
use uuid::Uuid;

use mcumgr_mac::cache::DeviceCache;
use mcumgr_mac::smp::{self, Header, Op, HEADER_LEN};

/// The MCUmgr SMP GATT service UUID (`8d53dc1d-1db7-4cd3-868b-8a527460aa84`).
pub const SMP_SERVICE_UUID: Uuid = Uuid::from_u128(0x8d53dc1d_1db7_4cd3_868b_8a527460aa84);
/// The MCUmgr SMP GATT characteristic UUID (`da2e7828-fbce-4e01-ae9e-261174997c48`).
pub const SMP_CHAR_UUID: Uuid = Uuid::from_u128(0xda2e7828_fbce_4e01_ae9e_261174997c48);

/// A device seen during scanning.
#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    /// Platform peripheral identifier (CoreBluetooth UUID on macOS).
    pub id: String,
    /// Advertised local name, if any.
    pub name: Option<String>,
    /// Most recent RSSI, if reported.
    pub rssi: Option<i16>,
}

/// How to choose the device to connect to.
#[derive(Debug, Default, Clone)]
pub struct ResolveOptions {
    /// Match devices whose advertised name contains this (case-insensitive).
    pub name: Option<String>,
    /// Connect to exactly this peripheral id.
    pub id: Option<String>,
    /// Scan all BLE devices instead of only those advertising the SMP service.
    pub all_devices: bool,
    /// Maximum time to scan, in seconds.
    pub scan_secs: u64,
    /// Whether to consult the device cache for prioritisation.
    pub use_cache: bool,
}

/// Obtain the host's first BLE adapter.
pub async fn adapter() -> Result<Adapter> {
    let manager = Manager::new()
        .await
        .context("initialising Bluetooth manager")?;
    let adapters = manager
        .adapters()
        .await
        .context("listing Bluetooth adapters")?;
    adapters
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no Bluetooth adapter found"))
}

fn scan_filter(all_devices: bool) -> ScanFilter {
    if all_devices {
        ScanFilter::default()
    } else {
        ScanFilter {
            services: vec![SMP_SERVICE_UUID],
        }
    }
}

/// Scan for `duration` and return the devices seen, annotated with cache hits.
pub async fn discover(
    adapter: &Adapter,
    duration: Duration,
    all_devices: bool,
) -> Result<Vec<DiscoveredDevice>> {
    adapter
        .start_scan(scan_filter(all_devices))
        .await
        .context("starting scan")?;
    let mut events = adapter
        .events()
        .await
        .context("subscribing to scan events")?;

    // Collect ids during the scan; read properties only once the window closes,
    // since CoreBluetooth populates names/RSSI asynchronously after a device is
    // first discovered.
    let order = collect_ids(&mut events, duration).await;
    adapter.stop_scan().await.ok();

    let mut devices = Vec::with_capacity(order.len());
    for id in order {
        let (name, rssi) = device_properties(adapter, &id).await;
        devices.push(DiscoveredDevice {
            id: id.to_string(),
            name,
            rssi,
        });
    }
    Ok(devices)
}

/// Drain scan events for `duration`, returning unique peripheral ids in the
/// order they were first seen.
async fn collect_ids(
    events: &mut (impl Stream<Item = btleplug::api::CentralEvent> + Unpin),
    duration: Duration,
) -> Vec<PeripheralId> {
    let mut seen: HashSet<PeripheralId> = HashSet::new();
    let mut order: Vec<PeripheralId> = Vec::new();
    let deadline = Instant::now() + duration;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let event = match tokio::time::timeout(remaining, events.next()).await {
            Ok(Some(event)) => event,
            _ => break,
        };
        let id = match event {
            btleplug::api::CentralEvent::DeviceDiscovered(id)
            | btleplug::api::CentralEvent::DeviceUpdated(id) => id,
            _ => continue,
        };
        if seen.insert(id.clone()) {
            order.push(id);
        }
    }
    order
}

/// Read a discovered peripheral's advertised name and RSSI.
async fn device_properties(adapter: &Adapter, id: &PeripheralId) -> (Option<String>, Option<i16>) {
    match adapter.peripheral(id).await {
        Ok(p) => match p.properties().await {
            // BLE RSSI is always negative; CoreBluetooth reports 127 (or 0) when
            // it is unavailable, so treat any non-negative value as unknown.
            Ok(Some(props)) => (props.local_name, props.rssi.filter(|r| *r < 0)),
            _ => (None, None),
        },
        Err(_) => (None, None),
    }
}

/// Resolve the device to connect to according to `opts`, preferring cached
/// devices and falling back to whatever the scan turns up.
pub async fn resolve_peripheral(
    adapter: &Adapter,
    opts: &ResolveOptions,
    cache: &DeviceCache,
) -> Result<Peripheral> {
    let ranked: Vec<String> = if opts.use_cache {
        cache
            .candidates(opts.name.as_deref())
            .into_iter()
            .map(|d| d.id.clone())
            .collect()
    } else {
        Vec::new()
    };

    adapter
        .start_scan(scan_filter(opts.all_devices))
        .await
        .context("starting scan")?;
    let mut events = adapter
        .events()
        .await
        .context("subscribing to scan events")?;

    let mut seen: HashSet<PeripheralId> = HashSet::new();
    let mut order: Vec<PeripheralId> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(opts.scan_secs);

    // Connect on first sight of the preferred device. For a device that only
    // advertises in brief bursts, grabbing it the instant it appears is far more
    // reliable than waiting for the scan window to end. When searching by name
    // we re-read properties on every event for that device, because
    // CoreBluetooth fills advertised names in asynchronously.
    let early: Option<PeripheralId> = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break None;
        }
        let event = match tokio::time::timeout(remaining, events.next()).await {
            Ok(Some(event)) => event,
            _ => break None,
        };
        let id = match event {
            btleplug::api::CentralEvent::DeviceDiscovered(id)
            | btleplug::api::CentralEvent::DeviceUpdated(id) => id,
            _ => continue,
        };
        let pid = id.to_string();
        let is_new = seen.insert(id.clone());
        if is_new {
            order.push(id.clone());
        }

        if let Some(want) = &opts.id {
            if &pid == want {
                break Some(id);
            }
        } else if let Some(want) = &opts.name {
            let (name, _) = device_properties(adapter, &id).await;
            if is_new {
                tracing::debug!(id = %pid, name = ?name, "discovered device");
            }
            if name_matches(name.as_deref(), want) {
                tracing::info!(id = %pid, name = ?name, "matched name during scan");
                break Some(id);
            }
        } else if ranked.first().map(String::as_str) == Some(pid.as_str()) {
            // The most-frequently-connected cached device is here.
            break Some(id);
        }
    };
    tracing::debug!(count = order.len(), "scan window ended");

    let chosen = match early {
        Some(id) => id,
        None => {
            // Names and RSSI populate asynchronously, so read them now that the
            // window has closed and match against the full set.
            let mut discovered = Vec::with_capacity(order.len());
            for id in &order {
                let (name, _) = device_properties(adapter, id).await;
                discovered.push((id.clone(), name));
            }
            select_after_scan(opts, &ranked, &discovered)?
        }
    };

    adapter.stop_scan().await.ok();
    adapter
        .peripheral(&chosen)
        .await
        .context("retrieving selected peripheral")
}

/// True if `name` contains `needle`, case-insensitively.
fn name_matches(name: Option<&str>, needle: &str) -> bool {
    name.map(|n| n.to_lowercase().contains(&needle.to_lowercase()))
        .unwrap_or(false)
}

/// After the scan window elapses with no id-based early exit, pick the best
/// device: honour an explicit id or name (erroring if absent), else the
/// highest-ranked cached device seen, else the first device seen.
fn select_after_scan(
    opts: &ResolveOptions,
    ranked: &[String],
    discovered: &[(PeripheralId, Option<String>)],
) -> Result<PeripheralId> {
    if let Some(want) = &opts.id {
        return discovered
            .iter()
            .find(|(id, _)| &id.to_string() == want)
            .map(|(id, _)| id.clone())
            .ok_or_else(|| {
                anyhow!(
                    "no device found with id '{want}' (scanned {}s)",
                    opts.scan_secs
                )
            });
    }
    if let Some(want) = &opts.name {
        return discovered
            .iter()
            .find(|(_, name)| name_matches(name.as_deref(), want))
            .map(|(id, _)| id.clone())
            .ok_or_else(|| {
                anyhow!(
                    "no device found matching name '{want}' (scanned {}s)",
                    opts.scan_secs
                )
            });
    }
    let best_cached = discovered
        .iter()
        .filter_map(|(id, _)| {
            ranked
                .iter()
                .position(|r| r == &id.to_string())
                .map(|rank| (rank, id))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, id)| id.clone());
    if let Some(id) = best_cached {
        return Ok(id);
    }

    // No name, no id, and no cached device was seen. Falling back to "first
    // discovered" is only safe when the scan is filtered to the SMP service —
    // otherwise it would connect to an arbitrary nearby BLE device.
    if opts.all_devices {
        bail!(
            "no known device seen during {}s scan. Pass --name or --id to choose one of \
             the nearby BLE devices, or --smp-devices to only consider SMP devices.",
            opts.scan_secs
        );
    }
    discovered
        .first()
        .map(|(id, _)| id.clone())
        .ok_or_else(|| anyhow!("no SMP devices found during {}s scan", opts.scan_secs))
}

/// A connected SMP session over a peripheral's SMP characteristic.
pub struct SmpSession {
    peripheral: Peripheral,
    characteristic: Characteristic,
    notifications: Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
    assembler: smp::FrameAssembler,
    seq: u8,
    timeout: Duration,
}

impl SmpSession {
    /// Connect to `peripheral`, discover services, locate the SMP
    /// characteristic, and subscribe to notifications.
    pub async fn connect(peripheral: Peripheral, timeout: Duration) -> Result<Self> {
        peripheral.connect().await.context("connecting to device")?;
        peripheral
            .discover_services()
            .await
            .context("discovering GATT services")?;

        let characteristic = peripheral
            .characteristics()
            .into_iter()
            .find(|c| c.uuid == SMP_CHAR_UUID)
            .ok_or_else(|| {
                anyhow!("SMP characteristic not found — is this an MCUmgr/SMP device?")
            })?;

        peripheral
            .subscribe(&characteristic)
            .await
            .context("subscribing to SMP notifications")?;
        let notifications = peripheral
            .notifications()
            .await
            .context("opening notification stream")?;

        Ok(Self {
            peripheral,
            characteristic,
            notifications,
            assembler: smp::FrameAssembler::new(),
            seq: 0,
            timeout,
        })
    }

    /// The connected peripheral's identifier.
    pub fn id(&self) -> String {
        self.peripheral.id().to_string()
    }

    /// The connected peripheral's advertised name, if known.
    pub async fn name(&self) -> Option<String> {
        self.peripheral
            .properties()
            .await
            .ok()
            .flatten()
            .and_then(|p| p.local_name)
    }

    fn next_seq(&mut self) -> u8 {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        seq
    }

    /// Write a request frame without waiting for a response. Useful for
    /// operations (such as reset) after which the device immediately
    /// disconnects.
    pub async fn send(&mut self, op: Op, group: u16, id: u8, payload: &[u8]) -> Result<()> {
        let seq = self.next_seq();
        let frame = smp::encode_frame(op, group, seq, id, payload);
        self.peripheral
            .write(&self.characteristic, &frame, WriteType::WithoutResponse)
            .await
            .context("writing SMP request")?;
        Ok(())
    }

    /// Send a request and wait (up to the session timeout) for the matching
    /// response, returning its CBOR payload.
    pub async fn request(&mut self, op: Op, group: u16, id: u8, payload: &[u8]) -> Result<Vec<u8>> {
        let timeout = self.timeout;
        self.request_with_timeout(op, group, id, payload, timeout)
            .await
    }

    /// Like [`request`](Self::request), but with an explicit per-call timeout.
    /// Used by the upload loop, which prefers a short timeout plus retries over
    /// one long wait.
    pub async fn request_with_timeout(
        &mut self,
        op: Op,
        group: u16,
        id: u8,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let seq = self.next_seq();
        let frame = smp::encode_frame(op, group, seq, id, payload);
        self.peripheral
            .write(&self.characteristic, &frame, WriteType::WithoutResponse)
            .await
            .context("writing SMP request")?;

        tokio::time::timeout(timeout, self.read_response(seq))
            .await
            .map_err(|_| anyhow!("timed out waiting for SMP response (seq {seq})"))?
    }

    /// Discard any buffered (possibly partial) notification bytes. Called after
    /// a timeout so a half-received frame cannot desync the next response.
    pub fn reset_buffer(&mut self) {
        self.assembler = smp::FrameAssembler::new();
    }

    /// Read notifications until a complete frame matching `want_seq` arrives.
    async fn read_response(&mut self, want_seq: u8) -> Result<Vec<u8>> {
        loop {
            while let Some(frame) = self.assembler.next_frame() {
                let header = Header::decode(&frame)?;
                if header.seq == want_seq {
                    return Ok(frame[HEADER_LEN..].to_vec());
                }
                // A frame with an unexpected sequence number is stale; skip it.
            }
            match self.notifications.next().await {
                Some(n) if n.uuid == self.characteristic.uuid => self.assembler.push(&n.value),
                Some(_) => {}
                None => bail!("notification stream ended before a response arrived"),
            }
        }
    }

    /// Disconnect from the device, ignoring errors.
    pub async fn disconnect(&self) {
        let _ = self.peripheral.disconnect().await;
    }
}
