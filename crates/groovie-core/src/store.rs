//! Tiny JSON persistence for user config (groups, scenes, nicknames) and the
//! last-known device list, so the app works instantly on startup and the CLI
//! can address devices without waiting for discovery.

use crate::model::{Group, Scene};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownDevice {
    pub id: String,
    pub ip: Ipv4Addr,
    pub sku: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreData {
    pub version: u32,
    pub nicknames: BTreeMap<String, String>,
    pub groups: Vec<Group>,
    pub scenes: Vec<Scene>,
    pub known_devices: Vec<KnownDevice>,
}

#[derive(Debug)]
pub struct Store {
    path: Option<PathBuf>,
    pub data: StoreData,
}

impl Store {
    pub fn in_memory() -> Self {
        Self { path: None, data: StoreData { version: 1, ..Default::default() } }
    }

    /// Load from `path`, or start empty if it does not exist yet.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => StoreData { version: 1, ..Default::default() },
            Err(e) => return Err(e),
        };
        Ok(Self { path: Some(path), data })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Atomically write to disk (write temp file, then rename).
    pub fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(&self.data).map_err(io::Error::other)?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    }

    pub fn upsert_known(&mut self, dev: KnownDevice) -> bool {
        match self.data.known_devices.iter_mut().find(|d| d.id == dev.id) {
            Some(existing) if *existing == dev => false,
            Some(existing) => {
                *existing = dev;
                true
            }
            None => {
                self.data.known_devices.push(dev);
                true
            }
        }
    }
}

/// Default config file location, e.g. `~/.config/groovie/groovie.json` on
/// Linux, `~/Library/Application Support/groovie/groovie.json` on macOS and
/// `%APPDATA%\groovie\groovie.json` on Windows.
pub fn default_store_path() -> Option<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("groovie").join("groovie.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/groovie.json");
        let mut s = Store::open(&path).unwrap();
        s.data.nicknames.insert("AA".into(), "Lamp".into());
        assert!(s.upsert_known(KnownDevice { id: "AA".into(), ip: Ipv4Addr::LOCALHOST, sku: "H6076".into() }));
        assert!(!s.upsert_known(KnownDevice { id: "AA".into(), ip: Ipv4Addr::LOCALHOST, sku: "H6076".into() }));
        s.save().unwrap();
        let s2 = Store::open(&path).unwrap();
        assert_eq!(s2.data, s.data);
    }
}
