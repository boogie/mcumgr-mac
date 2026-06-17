//! A persistent cache of devices we have successfully connected to.
//!
//! The cache lets the CLI prefer (and reconnect to) known devices before
//! falling back to a scan. Devices are ranked by how often we have connected
//! to them, with ties broken by recency.
//!
//! The pure ranking and bookkeeping logic is independent of the filesystem and
//! the clock (timestamps are passed in), which keeps it easy to test. The
//! binary is responsible for choosing the on-disk location and the current
//! time.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::{Error, Result};

/// A device we have connected to in the past.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedDevice {
    /// Platform peripheral identifier (a CoreBluetooth UUID on macOS).
    pub id: String,
    /// Last known advertised name, if any.
    pub name: Option<String>,
    /// How many times we have successfully connected to this device.
    pub connect_count: u64,
    /// When we last connected, used as a tiebreaker.
    #[serde(with = "time::serde::rfc3339")]
    pub last_connected: OffsetDateTime,
}

/// An in-memory view of the device cache.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceCache {
    devices: Vec<CachedDevice>,
}

impl DeviceCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a cache from existing records (primarily for tests).
    pub fn from_devices(devices: Vec<CachedDevice>) -> Self {
        Self { devices }
    }

    /// All cached devices in arbitrary (insertion) order.
    pub fn devices(&self) -> &[CachedDevice] {
        &self.devices
    }

    /// Candidate devices to try, highest priority first.
    ///
    /// When `name` is provided, only devices whose advertised name contains it
    /// (case-insensitive) are returned. Candidates are ordered by connection
    /// count (descending) and then by recency (most recent first).
    pub fn candidates(&self, name: Option<&str>) -> Vec<&CachedDevice> {
        let needle = name.map(|n| n.to_lowercase());
        let mut matched: Vec<&CachedDevice> = self
            .devices
            .iter()
            .filter(|d| match &needle {
                None => true,
                Some(needle) => d
                    .name
                    .as_deref()
                    .map(|n| n.to_lowercase().contains(needle))
                    .unwrap_or(false),
            })
            .collect();
        matched.sort_by_key(|d| (Reverse(d.connect_count), Reverse(d.last_connected)));
        matched
    }

    /// Record a successful connection: increment the device's count and refresh
    /// its name and timestamp, inserting it if previously unknown.
    pub fn record_success(&mut self, id: &str, name: Option<&str>, now: OffsetDateTime) {
        if let Some(dev) = self.devices.iter_mut().find(|d| d.id == id) {
            dev.connect_count += 1;
            dev.last_connected = now;
            if name.is_some() {
                dev.name = name.map(str::to_owned);
            }
        } else {
            self.devices.push(CachedDevice {
                id: id.to_owned(),
                name: name.map(str::to_owned),
                connect_count: 1,
                last_connected: now,
            });
        }
    }

    /// How many times we have connected to `id` (0 if unknown).
    pub fn connect_count(&self, id: &str) -> u64 {
        self.devices
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.connect_count)
            .unwrap_or(0)
    }

    /// Load a cache from `path`. A missing file yields an empty cache.
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| Error::Cache(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(e) => Err(Error::Cache(e.to_string())),
        }
    }

    /// Persist the cache to `path`, creating parent directories as needed.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(|e| Error::Cache(e.to_string()))?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// The default on-disk location for the cache in the platform data dir.
    pub fn default_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "mcumgr-mac")
            .ok_or_else(|| Error::Cache("could not determine data directory".into()))?;
        Ok(dirs.data_dir().join("devices.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn dev(id: &str, name: Option<&str>, count: u64, when: OffsetDateTime) -> CachedDevice {
        CachedDevice {
            id: id.to_owned(),
            name: name.map(str::to_owned),
            connect_count: count,
            last_connected: when,
        }
    }

    #[test]
    fn candidates_order_by_connect_count_descending() {
        let cache = DeviceCache::from_devices(vec![
            dev("a", Some("Alpha"), 2, datetime!(2024-01-01 0:00 UTC)),
            dev("b", Some("Bravo"), 9, datetime!(2024-01-01 0:00 UTC)),
            dev("c", Some("Charlie"), 5, datetime!(2024-01-01 0:00 UTC)),
        ]);
        let ids: Vec<&str> = cache
            .candidates(None)
            .iter()
            .map(|d| d.id.as_str())
            .collect();
        assert_eq!(ids, ["b", "c", "a"]);
    }

    #[test]
    fn candidates_break_ties_by_recency() {
        let cache = DeviceCache::from_devices(vec![
            dev("old", Some("X"), 3, datetime!(2024-01-01 0:00 UTC)),
            dev("new", Some("Y"), 3, datetime!(2024-06-01 0:00 UTC)),
        ]);
        let ids: Vec<&str> = cache
            .candidates(None)
            .iter()
            .map(|d| d.id.as_str())
            .collect();
        assert_eq!(ids, ["new", "old"]);
    }

    #[test]
    fn candidates_filter_by_name_case_insensitive_substring() {
        let cache = DeviceCache::from_devices(vec![
            dev("a", Some("My Sensor"), 10, datetime!(2024-01-01 0:00 UTC)),
            dev("b", Some("Other"), 20, datetime!(2024-01-01 0:00 UTC)),
            dev("c", None, 30, datetime!(2024-01-01 0:00 UTC)),
        ]);
        let ids: Vec<&str> = cache
            .candidates(Some("sensor"))
            .iter()
            .map(|d| d.id.as_str())
            .collect();
        assert_eq!(ids, ["a"]);
    }

    #[test]
    fn record_success_inserts_unknown_device() {
        let mut cache = DeviceCache::new();
        cache.record_success("x", Some("New"), datetime!(2024-01-01 0:00 UTC));
        assert_eq!(cache.connect_count("x"), 1);
        assert_eq!(cache.devices()[0].name.as_deref(), Some("New"));
    }

    #[test]
    fn record_success_increments_and_refreshes_existing() {
        let mut cache = DeviceCache::from_devices(vec![dev(
            "x",
            Some("Old"),
            4,
            datetime!(2024-01-01 0:00 UTC),
        )]);
        cache.record_success("x", Some("Renamed"), datetime!(2024-05-05 12:00 UTC));
        let d = &cache.devices()[0];
        assert_eq!(d.connect_count, 5);
        assert_eq!(d.name.as_deref(), Some("Renamed"));
        assert_eq!(d.last_connected, datetime!(2024-05-05 12:00 UTC));
    }

    #[test]
    fn connect_count_is_zero_for_unknown() {
        let cache = DeviceCache::new();
        assert_eq!(cache.connect_count("nope"), 0);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let path =
            std::env::temp_dir().join(format!("mcumgr-mac-cache-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut cache = DeviceCache::new();
        cache.record_success("id-1", Some("Dev One"), datetime!(2024-03-03 9:00 UTC));
        cache.save_to(&path).unwrap();

        let loaded = DeviceCache::load_from(&path).unwrap();
        assert_eq!(loaded.devices(), cache.devices());

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn load_missing_file_yields_empty_cache() {
        let path = std::env::temp_dir().join("mcumgr-mac-cache-does-not-exist.json");
        let _ = std::fs::remove_file(&path);
        let cache = DeviceCache::load_from(&path).unwrap();
        assert!(cache.devices().is_empty());
    }
}
