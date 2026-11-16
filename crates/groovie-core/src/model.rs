//! Public data model shared by the engine, CLI, desktop app and (later) server.

use crate::protocol::Rgb;
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

/// Last known state of a light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceState {
    pub on: bool,
    /// 1..=100
    pub brightness: u8,
    pub color: Rgb,
    /// Colour temperature in Kelvin, or 0 when the light is in RGB mode.
    pub kelvin: u16,
}

impl Default for DeviceState {
    fn default() -> Self {
        Self { on: false, brightness: 100, color: Rgb::new(255, 255, 255), kelvin: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    /// Govee device id (MAC-like, e.g. `1F:80:C5:32:32:36:72:4E`).
    pub id: String,
    pub ip: Ipv4Addr,
    pub sku: String,
    /// User nickname, or a friendly default derived from the SKU.
    pub name: String,
    pub online: bool,
    /// `None` until the device has answered a status request.
    pub state: Option<DeviceState>,
    /// Firmware versions reported in `scan` replies, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    /// Most recent RGB colour observed or commanded on this light, preserved when switching to White mode.
    #[serde(default, rename = "lastColor", skip_serializing_if = "Option::is_none")]
    pub last_color: Option<Rgb>,
    /// Most recent Kelvin temperature observed or commanded on this light, preserved when switching to Colour mode.
    #[serde(default, rename = "lastKelvin", skip_serializing_if = "Option::is_none")]
    pub last_kelvin: Option<u16>,
}

/// One line in the in-memory diagnostic log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// Unix epoch milliseconds.
    pub ts_ms: u64,
    /// `"action"`, `"state"`, `"tx"`, or `"rx"`.
    pub dir: String,
    /// Remote socket address (`10.0.0.64:4003`), or empty for high-level actions.
    pub addr: String,
    /// Matched device label (`"Light Bars (H6056)"`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Human-readable summary or raw UDP JSON payload.
    pub message: String,
    /// True for routine `devStatus` traffic so the UI can hide background polls.
    #[serde(default)]
    pub poll: bool,
}

/// Active configuration file metadata and serialized contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigInfo {
    /// Configured `groovie.json` path (`None` when running in-memory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Resolved target path when `path` is a symlink (`None` if not symlinked).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
    /// Platform default path Groovie would use without a custom override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_path: Option<String>,
    /// True when the active path differs from the automatic default path.
    pub is_custom: bool,
    /// Pretty-printed JSON representation of the current config.
    pub json: String,
}

/// Per-machine desktop startup preferences (stored outside dotfiles `groovie.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPrefs {
    #[serde(default)]
    pub launch_at_login: bool,
    #[serde(default)]
    pub start_minimized: bool,
}

/// What to do.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum Action {
    Power(bool),
    /// For several devices: if any is on, turn all off; otherwise turn all on.
    Toggle,
    Brightness(u8),
    Color(Rgb),
    Kelvin(u16),
}

/// Who to do it to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "camelCase")]
pub enum Target {
    Device(String),
    Group(String),
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    /// Device ids.
    pub members: Vec<String>,
}

/// A saved look: per-device power/brightness/colour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub id: String,
    pub name: String,
    pub entries: Vec<SceneEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneEntry {
    pub device: String,
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kelvin: Option<u16>,
}

/// Pushed to subscribers whenever something changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    DeviceUpdated { device: Device },
    /// Groups, scenes or nicknames changed.
    ConfigChanged,
    Logged { entry: LogEntry },
    SyncChanged { status: crate::sync::SyncStatus },
    SyncPreview { preview: crate::sync::SyncPreview },
}

/// Friendly default name for a SKU. Unknown SKUs fall back to `Govee <SKU>`.
pub fn friendly_sku_name(sku: &str) -> String {
    let known = match sku.to_ascii_uppercase().as_str() {
        "H6076" | "H6072" | "H6079" => "Floor Lamp",
        "H607C" => "Floor Lamp 2",
        "H6046" | "H6047" | "H6056" | "H6043" => "Light Bars",
        "H6008" | "H6009" | "H6010" => "Bulb",
        "H6052" | "H6051" => "Table Lamp",
        _ => return format!("Govee {sku}"),
    };
    known.to_string()
}
