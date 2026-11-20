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

/// Visual icon category derived from a Govee SKU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DeviceIcon {
    Lamp,
    Bars,
    Strip,
    #[default]
    Bulb,
}

/// Supported white colour temperature range in Kelvin for a device SKU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KelvinRange {
    pub min: u16,
    pub max: u16,
}

impl Default for KelvinRange {
    fn default() -> Self {
        Self {
            min: crate::protocol::KELVIN_MIN,
            max: crate::protocol::KELVIN_MAX,
        }
    }
}

impl KelvinRange {
    pub const fn new(min: u16, max: u16) -> Self {
        Self { min, max }
    }

    pub fn clamp(self, k: u16) -> u16 {
        k.clamp(self.min, self.max)
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
    /// Icon category derived from `sku`.
    #[serde(default)]
    pub icon: DeviceIcon,
    /// Hardware-supported white colour temperature bounds derived from `sku`.
    #[serde(default, rename = "kelvinRange")]
    pub kelvin_range: KelvinRange,
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

impl Device {
    pub fn new(id: String, ip: Ipv4Addr, sku: String, name: String) -> Self {
        let meta = sku_metadata(&sku);
        Self {
            id,
            ip,
            sku,
            name,
            online: false,
            state: None,
            icon: meta.icon,
            kelvin_range: meta.kelvin_range,
            firmware: None,
            last_color: None,
            last_kelvin: None,
        }
    }
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
    /// Validated pinned desktop release version (`"0.5.3"`), when configured in `groovie.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_version: Option<String>,
    /// Validation message when `groovie.json` specifies an invalid or pre-`v0.5.3` `pinned_version`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_version_error: Option<String>,
    /// Earliest Groovie release supporting version pinning (`"0.5.3"`).
    pub min_pinnable_version: String,
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

/// Hardware defaults derived from a Govee SKU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkuMetadata {
    pub default_name: Option<&'static str>,
    pub icon: DeviceIcon,
    pub kelvin_range: KelvinRange,
    pub sync_shape: crate::sync::SkuSyncShape,
}

/// Central lookup table for known Govee LAN-capable SKUs.
pub fn sku_metadata(sku: &str) -> SkuMetadata {
    use crate::sync::{Orientation, SkuSyncShape};
    let full_k = KelvinRange::new(crate::protocol::KELVIN_MIN, crate::protocol::KELVIN_MAX);
    let lamp_k = KelvinRange::new(2200, 6500);
    let bulb_k = KelvinRange::new(2700, 6500);

    match sku.trim().to_ascii_uppercase().as_str() {
        // Flow Plus / Flow Pro 2-bar kits: 6 segments per bar (slots 0..6 and 6..12).
        "H6056" | "H6043" => SkuMetadata {
            default_name: Some("Light Bars"),
            icon: DeviceIcon::Bars,
            kelvin_range: full_k,
            sync_shape: SkuSyncShape::DualBars { segments_per_bar: 6 },
        },
        // Gaming / TV Light Bars 2-bar kits: 10 segments per bar (slots 0..10 and 10..20).
        "H6046" | "H6047" | "H6048" => SkuMetadata {
            default_name: Some("Light Bars"),
            icon: DeviceIcon::Bars,
            kelvin_range: full_k,
            sync_shape: SkuSyncShape::DualBars { segments_per_bar: 10 },
        },
        // Razer-capable RGBIC Floor Lamps (Lyra / Floor Lamp Pro).
        "H6072" | "H6078" | "H6079" | "H6672" => SkuMetadata {
            default_name: Some("Floor Lamp"),
            icon: DeviceIcon::Lamp,
            kelvin_range: lamp_k,
            sync_shape: SkuSyncShape::LinearRazer {
                segments: 15,
                orientation: Orientation::Vertical,
                corner_lamp: true,
            },
        },
        "H607C" => SkuMetadata {
            default_name: Some("Floor Lamp 2"),
            icon: DeviceIcon::Lamp,
            kelvin_range: lamp_k,
            sync_shape: SkuSyncShape::LinearRazer {
                segments: 15,
                orientation: Orientation::Vertical,
                corner_lamp: true,
            },
        },
        // Non-Razer Floor Lamps (Floor Lamp Basic).
        "H6076" | "H6073" => SkuMetadata {
            default_name: Some("Floor Lamp"),
            icon: DeviceIcon::Lamp,
            kelvin_range: lamp_k,
            sync_shape: SkuSyncShape::SingleSolid { corner_lamp: true },
        },
        // Table Lamps (Aura / Table Lamp 2).
        "H6051" | "H6052" => SkuMetadata {
            default_name: Some("Table Lamp"),
            icon: DeviceIcon::Lamp,
            kelvin_range: lamp_k,
            sync_shape: SkuSyncShape::SingleSolid { corner_lamp: false },
        },
        // Glide / Neon Wall Bars.
        "H6061" | "H6062" | "H6065" | "H6066" | "H6067" => SkuMetadata {
            default_name: Some("Glide Wall Light"),
            icon: DeviceIcon::Bars,
            kelvin_range: full_k,
            sync_shape: SkuSyncShape::LinearRazer {
                segments: 10,
                orientation: Orientation::Horizontal,
                corner_lamp: false,
            },
        },
        "H610A" | "H610B" => SkuMetadata {
            default_name: Some("Wall Light"),
            icon: DeviceIcon::Bars,
            kelvin_range: full_k,
            sync_shape: SkuSyncShape::LinearRazer {
                segments: 10,
                orientation: Orientation::Horizontal,
                corner_lamp: false,
            },
        },
        // RGBIC LED strips & COB strips.
        "H618A" | "H618C" | "H619A" | "H619B" | "H619C" | "H619D" | "H619E" | "H619Z"
        | "H61A0" | "H61A1" | "H61A2" | "H61A3" | "H61BA" | "H61BC" | "H61BE" | "H612F"
        | "H61E1" | "H61E5" => SkuMetadata {
            default_name: Some("LED Strip"),
            icon: DeviceIcon::Strip,
            kelvin_range: full_k,
            sync_shape: SkuSyncShape::LinearRazer {
                segments: 15,
                orientation: Orientation::Horizontal,
                corner_lamp: false,
            },
        },
        // Solid RGB LED strips.
        "H6159" | "H615A" | "H6141" => SkuMetadata {
            default_name: Some("LED Strip"),
            icon: DeviceIcon::Strip,
            kelvin_range: full_k,
            sync_shape: SkuSyncShape::SingleSolid { corner_lamp: false },
        },
        // Smart Bulbs.
        "H6008" | "H6009" | "H6010" => SkuMetadata {
            default_name: Some("Bulb"),
            icon: DeviceIcon::Bulb,
            kelvin_range: bulb_k,
            sync_shape: SkuSyncShape::SingleSolid { corner_lamp: false },
        },
        _ => SkuMetadata {
            default_name: None,
            icon: DeviceIcon::Bulb,
            kelvin_range: full_k,
            sync_shape: SkuSyncShape::SingleSolid { corner_lamp: false },
        },
    }
}

/// Friendly default name for a SKU. Unknown SKUs fall back to `Govee <SKU>`.
pub fn friendly_sku_name(sku: &str) -> String {
    match sku_metadata(sku).default_name {
        Some(known) => known.to_string(),
        None => format!("Govee {}", sku.trim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sku_metadata_unifies_icons_names_and_kelvin_bounds() {
        let lamp = sku_metadata("h6072");
        assert_eq!(lamp.icon, DeviceIcon::Lamp);
        assert_eq!(lamp.kelvin_range, KelvinRange::new(2200, 6500));
        assert_eq!(friendly_sku_name("H6072"), "Floor Lamp");

        let strip = sku_metadata("H619A");
        assert_eq!(strip.icon, DeviceIcon::Strip);
        assert_eq!(strip.kelvin_range, KelvinRange::new(2000, 9000));
        assert_eq!(friendly_sku_name("H619A"), "LED Strip");

        let bulb = sku_metadata("H6008");
        assert_eq!(bulb.icon, DeviceIcon::Bulb);
        assert_eq!(bulb.kelvin_range, KelvinRange::new(2700, 6500));
        assert_eq!(bulb.kelvin_range.clamp(2000), 2700);
        assert_eq!(bulb.kelvin_range.clamp(9000), 6500);

        let unknown = sku_metadata("H9999");
        assert_eq!(unknown.icon, DeviceIcon::Bulb);
        assert_eq!(unknown.kelvin_range, KelvinRange::new(2000, 9000));
        assert_eq!(friendly_sku_name("H9999"), "Govee H9999");
    }
}
