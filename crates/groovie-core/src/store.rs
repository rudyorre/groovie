//! Tiny JSON persistence for user config (groups, scenes, nicknames) and the
//! last-known device list, so the app works instantly on startup and the CLI
//! can address devices without waiting for discovery.

use crate::model::{Group, Scene};
use crate::sync::SyncConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
    #[serde(default, skip_serializing_if = "SyncConfig::is_default")]
    pub sync: SyncConfig,
}

#[derive(Debug)]
pub struct Store {
    path: Option<PathBuf>,
    mtime: Option<SystemTime>,
    pub data: StoreData,
}

impl Store {
    pub fn in_memory() -> Self {
        Self { path: None, mtime: None, data: StoreData { version: 1, ..Default::default() } }
    }

    /// Load from `path`, or start empty if it does not exist yet.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let (data, mtime) = read_store_file(&path)?;
        Ok(Self { path: Some(path), mtime, data })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// If `self.path` is a symlink, returns its resolved target path.
    pub fn symlink_target(&self) -> Option<PathBuf> {
        let path = self.path.as_deref()?;
        let meta = std::fs::symlink_metadata(path).ok()?;
        if !meta.file_type().is_symlink() {
            return None;
        }
        resolve_write_target(path).ok().filter(|resolved| resolved != path)
    }

    pub fn pretty_json(&self) -> String {
        serde_json::to_string_pretty(&self.data).unwrap_or_else(|_| "{}".into())
    }

    /// Atomically write to disk while preserving symlinks (`stow`, `ln -s`, `chezmoi`).
    pub fn save(&mut self) -> io::Result<()> {
        let Some(path) = self.path.clone() else { return Ok(()) };
        self.data.known_devices.sort_by(|a, b| a.id.cmp(&b.id));
        let target = resolve_write_target(&path)?;
        if let Some(dir) = target.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let tmp_name = target
            .file_name()
            .map(|n| format!("{}.tmp", n.to_string_lossy()))
            .unwrap_or_else(|| "groovie.json.tmp".into());
        let tmp = target.with_file_name(tmp_name);
        let mut bytes = serde_json::to_vec_pretty(&self.data).map_err(io::Error::other)?;
        bytes.push(b'\n');
        std::fs::write(&tmp, &bytes)?;
        if let Err(rename_err) = std::fs::rename(&tmp, &target) {
            // Fallback if a file-sync client on Windows briefly holds a share lock.
            if std::fs::write(&target, &bytes).is_err() {
                let _ = std::fs::remove_file(&tmp);
                return Err(rename_err);
            }
            let _ = std::fs::remove_file(&tmp);
        }
        self.mtime = file_mtime(&path);
        Ok(())
    }

    /// Ensure the backing file exists on disk (handy before opening its folder).
    pub fn ensure_saved(&mut self) -> io::Result<()> {
        if let Some(path) = &self.path {
            if std::fs::metadata(path).is_err() {
                return self.save();
            }
        }
        Ok(())
    }

    /// Unconditionally re-read `self.path` from disk. Returns `true` if `data` changed.
    pub fn reload(&mut self) -> io::Result<bool> {
        let Some(path) = self.path.clone() else { return Ok(false) };
        let (next, mtime) = read_store_file(&path)?;
        let changed = self.data != next;
        self.data = next;
        self.mtime = mtime;
        Ok(changed)
    }

    /// Re-read `self.path` only if its modification timestamp changed externally.
    pub fn reload_if_changed(&mut self) -> io::Result<bool> {
        let Some(path) = &self.path else { return Ok(false) };
        if file_mtime(path) == self.mtime {
            return Ok(false);
        }
        self.reload()
    }

    /// Switch to `path`: loads existing data if `path` already exists on disk, or
    /// seeds `path` with the current in-memory data if it does not exist yet.
    pub fn set_path(&mut self, path: PathBuf) -> io::Result<()> {
        if std::fs::metadata(&path).is_ok() {
            let (next, mtime) = read_store_file(&path)?;
            self.path = Some(path);
            self.data = next;
            self.mtime = mtime;
            Ok(())
        } else {
            self.path = Some(path);
            self.save()
        }
    }

    /// Parse and validate `raw_json` as [`StoreData`], preserve any currently-online
    /// `live_known` devices (keeping their live verified IPs), and atomically save.
    pub fn replace_json(&mut self, raw_json: &str, live_known: &[KnownDevice]) -> io::Result<()> {
        let mut next: StoreData = serde_json::from_str(raw_json).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("invalid JSON: {e}"))
        })?;
        if next.version == 0 {
            next.version = 1;
        }
        self.data = next;
        for dev in live_known {
            self.upsert_known(dev.clone());
        }
        self.data.known_devices.sort_by(|a, b| a.id.cmp(&b.id));
        self.save()
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
                self.data.known_devices.sort_by(|a, b| a.id.cmp(&b.id));
                true
            }
        }
    }
}

fn read_store_file(path: &Path) -> io::Result<(StoreData, Option<SystemTime>)> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut data: StoreData = serde_json::from_slice(&bytes).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, format!("invalid JSON in {}: {e}", path.display()))
            })?;
            if data.version == 0 {
                data.version = 1;
            }
            Ok((data, file_mtime(path)))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Ok((StoreData { version: 1, ..Default::default() }, None))
        }
        Err(e) => Err(e),
    }
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Follow up to 16 symlink hops (including dangling symlinks) so atomic saves
/// update the real dotfiles target rather than replacing the symlink itself.
pub fn resolve_write_target(path: &Path) -> io::Result<PathBuf> {
    let mut cur = path.to_path_buf();
    for _ in 0..16 {
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = std::fs::read_link(&cur)?;
                cur = if target.is_absolute() {
                    target
                } else {
                    cur.parent().unwrap_or_else(|| Path::new(".")).join(target)
                };
            }
            _ => break,
        }
    }
    Ok(cur)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Expand leading `~/` or `~\` to the user's home directory, and append
/// `groovie.json` if the path points to an existing directory.
pub fn normalize_user_store_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    let expanded = if let Some(rest) = trimmed.strip_prefix("~/").or_else(|| trimmed.strip_prefix("~\\")) {
        match home_dir() {
            Some(h) => h.join(rest),
            None => PathBuf::from(trimmed),
        }
    } else {
        PathBuf::from(trimmed)
    };
    if expanded.is_dir() {
        expanded.join("groovie.json")
    } else {
        expanded
    }
}

fn platform_config_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(any(target_os = "macos", target_os = "ios")) {
        home_dir().map(|h| h.join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".config")))
    }?;
    Some(base.join("groovie"))
}

fn xdg_store_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".config")))?;
    Some(base.join("groovie").join("groovie.json"))
}

fn custom_path_pointer_file() -> Option<PathBuf> {
    platform_config_dir().map(|d| d.join("config-path"))
}

/// Read the optional custom config path pointer saved by the desktop app.
pub fn read_custom_store_path() -> Option<PathBuf> {
    let ptr = custom_path_pointer_file()?;
    let raw = std::fs::read_to_string(ptr).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// Save or clear the custom config path pointer, returning the active path.
pub fn write_custom_store_path(custom: Option<&Path>) -> io::Result<Option<PathBuf>> {
    let Some(ptr) = custom_path_pointer_file() else {
        return Ok(custom.map(Path::to_path_buf));
    };
    match custom {
        Some(p) => {
            if let Some(dir) = ptr.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&ptr, format!("{}\n", p.to_string_lossy()))?;
            Ok(Some(p.to_path_buf()))
        }
        None => {
            match std::fs::remove_file(&ptr) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            Ok(default_auto_store_path())
        }
    }
}

/// Automatic config file location (ignoring custom pointer overrides):
/// on iOS uses `~/Documents/groovie.json` so it is directly accessible in the
/// Files app (*On My iPhone → Groovie*); on desktop prefers an existing
/// `~/.config/groovie/groovie.json` for cross-OS dotfiles repos, otherwise falls
/// back to `%APPDATA%\groovie\groovie.json`,
/// `~/Library/Application Support/groovie/groovie.json`, or
/// `~/.config/groovie/groovie.json`.
pub fn default_auto_store_path() -> Option<PathBuf> {
    if cfg!(target_os = "ios") {
        return home_dir().map(|h| h.join("Documents").join("groovie.json"));
    }
    if let Some(xdg) = xdg_store_path() {
        if std::fs::symlink_metadata(&xdg).is_ok() {
            return Some(xdg);
        }
    }
    platform_config_dir().map(|d| d.join("groovie.json"))
}

/// Active default config file location (honoring any custom path pointer).
pub fn default_store_path() -> Option<PathBuf> {
    read_custom_store_path().or_else(default_auto_store_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_and_sorts_known_devices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/groovie.json");
        let mut s = Store::open(&path).unwrap();
        s.data.nicknames.insert("AA".into(), "Lamp".into());
        assert!(s.upsert_known(KnownDevice { id: "BB".into(), ip: Ipv4Addr::LOCALHOST, sku: "H6056".into() }));
        assert!(s.upsert_known(KnownDevice { id: "AA".into(), ip: Ipv4Addr::LOCALHOST, sku: "H6076".into() }));
        assert!(!s.upsert_known(KnownDevice { id: "AA".into(), ip: Ipv4Addr::LOCALHOST, sku: "H6076".into() }));
        assert_eq!(s.data.known_devices[0].id, "AA");
        assert_eq!(s.data.known_devices[1].id, "BB");
        s.save().unwrap();
        let s2 = Store::open(&path).unwrap();
        assert_eq!(s2.data, s.data);
    }

    #[cfg(unix)]
    #[test]
    fn save_preserves_symlink_including_dangling_target() {
        let dir = tempfile::tempdir().unwrap();
        let dotfiles = dir.path().join("dotfiles/groovie.json");
        let link_dir = dir.path().join("config");
        std::fs::create_dir_all(&link_dir).unwrap();
        let link = link_dir.join("groovie.json");
        // Relative symlink whose target directory doesn't even exist yet.
        std::os::unix::fs::symlink("../dotfiles/groovie.json", &link).unwrap();

        let mut s = Store::open(&link).unwrap();
        s.data.nicknames.insert("11".into(), "Desk".into());
        s.save().unwrap();

        // Symlink itself must remain a symlink, and the real file in `dotfiles/` must exist.
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(dotfiles.is_file());
        assert!(s.symlink_target().is_some());
        let s2 = Store::open(&dotfiles).unwrap();
        assert_eq!(s2.data.nicknames.get("11").map(String::as_str), Some("Desk"));
    }

    #[test]
    fn detects_external_edits_and_switches_paths() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.json");
        let mut s = Store::open(&first).unwrap();
        s.data.nicknames.insert("AA".into(), "Lamp".into());
        s.save().unwrap();
        assert!(!s.reload_if_changed().unwrap());

        // Seed a brand-new path with current data.
        let seeded = dir.path().join("dotfiles/groovie.json");
        s.set_path(seeded.clone()).unwrap();
        assert!(seeded.is_file());

        // Simulate external edit (bump mtime so sub-second filesystems still trigger).
        std::thread::sleep(std::time::Duration::from_millis(15));
        let mut edited = s.data.clone();
        edited.nicknames.insert("AA".into(), "Floor Lamp".into());
        std::fs::write(&seeded, serde_json::to_vec_pretty(&edited).unwrap()).unwrap();

        assert!(s.reload_if_changed().unwrap());
        assert_eq!(s.data.nicknames.get("AA").map(String::as_str), Some("Floor Lamp"));
    }

    #[test]
    fn replace_json_validates_and_preserves_live_devices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("groovie.json");
        let mut s = Store::open(&path).unwrap();
        s.data.nicknames.insert("OLD".into(), "Before".into());
        s.save().unwrap();

        let live = vec![KnownDevice {
            id: "LIVE1".into(),
            ip: Ipv4Addr::new(192, 168, 1, 42),
            sku: "H6072".into(),
        }];

        // Invalid JSON is rejected without mutating state or disk.
        let err = s.replace_json("{ not valid json", &live).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(s.data.nicknames.get("OLD").map(String::as_str), Some("Before"));

        // Valid JSON replaces config while keeping live online devices & updating stale live IPs.
        let raw = r#"{
          "nicknames": { "LIVE1": "Corner Lamp", "OFF1": "Bedroom" },
          "groups": [{ "id": "g1", "name": "Main", "members": ["LIVE1", "OFF1"] }],
          "known_devices": [
            { "id": "LIVE1", "ip": "192.168.1.9", "sku": "H6072" },
            { "id": "OFF1", "ip": "192.168.1.77", "sku": "H6056" }
          ]
        }"#;
        s.replace_json(raw, &live).unwrap();
        assert_eq!(s.data.version, 1);
        assert!(!s.data.nicknames.contains_key("OLD"));
        assert_eq!(s.data.nicknames.get("LIVE1").map(String::as_str), Some("Corner Lamp"));
        assert_eq!(s.data.groups.len(), 1);
        let live_saved = s.data.known_devices.iter().find(|d| d.id == "LIVE1").unwrap();
        assert_eq!(live_saved.ip, Ipv4Addr::new(192, 168, 1, 42));
        let off_saved = s.data.known_devices.iter().find(|d| d.id == "OFF1").unwrap();
        assert_eq!(off_saved.ip, Ipv4Addr::new(192, 168, 1, 77));
    }
}
