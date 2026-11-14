//! Wire format for the Govee LAN API.
//!
//! Reference: <https://app-h5.govee.com/user-manual/wlan-guide>
//!
//! * Discovery: a `scan` request is multicast to `239.255.255.250:4001`;
//!   devices answer (unicast) to the sender on port `4002`.
//! * Control: JSON commands are sent to `<device-ip>:4003`.
//! * Status: a `devStatus` request to `:4003` is answered on `:4002`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::Ipv4Addr;

pub const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
pub const SCAN_PORT: u16 = 4001;
pub const LISTEN_PORT: u16 = 4002;
pub const COMMAND_PORT: u16 = 4003;

/// Colour temperature range supported by most Govee lights.
pub const KELVIN_MIN: u16 = 2000;
pub const KELVIN_MAX: u16 = 9000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse `#rrggbb` / `rrggbb` / `#rgb`.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        let expanded: String = match s.len() {
            3 => s.chars().flat_map(|c| [c, c]).collect(),
            6 => s.to_string(),
            _ => return None,
        };
        let v = u32::from_str_radix(&expanded, 16).ok()?;
        Some(Self::new((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// A control command understood by the device.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Scan,
    Turn(bool),
    /// 1..=100
    Brightness(u8),
    Color(Rgb),
    /// Colour temperature in Kelvin.
    ColorTemp(u16),
    DevStatus,
}

impl Command {
    pub fn to_json(&self) -> Value {
        let (cmd, data) = match self {
            Command::Scan => ("scan", json!({ "account_topic": "reserve" })),
            Command::Turn(on) => ("turn", json!({ "value": u8::from(*on) })),
            Command::Brightness(b) => ("brightness", json!({ "value": (*b).clamp(1, 100) })),
            Command::Color(c) => (
                "colorwc",
                json!({ "color": { "r": c.r, "g": c.g, "b": c.b }, "colorTemInKelvin": 0 }),
            ),
            Command::ColorTemp(k) => (
                "colorwc",
                json!({
                    "color": { "r": 0, "g": 0, "b": 0 },
                    "colorTemInKelvin": (*k).clamp(KELVIN_MIN, KELVIN_MAX)
                }),
            ),
            Command::DevStatus => ("devStatus", json!({})),
        };
        json!({ "msg": { "cmd": cmd, "data": data } })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.to_json()).expect("command serialises")
    }
}

/// Reply to a `scan` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanInfo {
    pub ip: String,
    pub device: String,
    pub sku: String,
    #[serde(default)]
    pub ble_version_hard: Option<String>,
    #[serde(default)]
    pub ble_version_soft: Option<String>,
    #[serde(default)]
    pub wifi_version_hard: Option<String>,
    #[serde(default)]
    pub wifi_version_soft: Option<String>,
}

/// Reply to a `devStatus` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusInfo {
    pub on_off: u8,
    pub brightness: u8,
    #[serde(default)]
    pub color: Rgb,
    #[serde(default)]
    pub color_tem_in_kelvin: u16,
}

/// Any message a device may send to us.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Scan(ScanInfo),
    Status(StatusInfo),
    Other { cmd: String, data: Value },
}

#[derive(Deserialize)]
struct Envelope {
    msg: Inner,
}

#[derive(Deserialize)]
struct Inner {
    cmd: String,
    #[serde(default)]
    data: Value,
}

pub fn parse_incoming(bytes: &[u8]) -> Result<Incoming, serde_json::Error> {
    let env: Envelope = serde_json::from_slice(bytes)?;
    Ok(match env.msg.cmd.as_str() {
        "scan" => Incoming::Scan(serde_json::from_value(env.msg.data)?),
        "devStatus" => Incoming::Status(serde_json::from_value(env.msg.data)?),
        _ => Incoming::Other { cmd: env.msg.cmd, data: env.msg.data },
    })
}

/// Parse a command as a *device* would see it (used by the simulator).
pub fn parse_command(bytes: &[u8]) -> Result<Command, String> {
    let env: Envelope = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let d = &env.msg.data;
    let num = |key: &str| d.get(key).and_then(Value::as_u64);
    Ok(match env.msg.cmd.as_str() {
        "scan" => Command::Scan,
        "devStatus" => Command::DevStatus,
        "turn" => Command::Turn(num("value").ok_or("missing value")? != 0),
        "brightness" => Command::Brightness(num("value").ok_or("missing value")? as u8),
        "colorwc" => {
            let k = num("colorTemInKelvin").unwrap_or(0) as u16;
            if k > 0 {
                Command::ColorTemp(k)
            } else {
                let c: Rgb = serde_json::from_value(d.get("color").cloned().unwrap_or_default())
                    .map_err(|e| e.to_string())?;
                Command::Color(c)
            }
        }
        other => return Err(format!("unknown cmd {other}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        assert_eq!(Rgb::from_hex("#ff8000"), Some(Rgb::new(255, 128, 0)));
        assert_eq!(Rgb::from_hex("f80"), Some(Rgb::new(255, 136, 0)));
        assert_eq!(Rgb::new(1, 2, 3).to_hex(), "#010203");
        assert_eq!(Rgb::from_hex("nope"), None);
    }

    #[test]
    fn encodes_commands_like_govee_docs() {
        assert_eq!(
            Command::Turn(true).to_json(),
            json!({"msg":{"cmd":"turn","data":{"value":1}}})
        );
        assert_eq!(
            Command::Brightness(0).to_json(),
            json!({"msg":{"cmd":"brightness","data":{"value":1}}})
        );
        assert_eq!(
            Command::Color(Rgb::new(1, 2, 3)).to_json(),
            json!({"msg":{"cmd":"colorwc","data":{"color":{"r":1,"g":2,"b":3},"colorTemInKelvin":0}}})
        );
    }

    #[test]
    fn commands_roundtrip_through_device_parser() {
        for c in [
            Command::Scan,
            Command::DevStatus,
            Command::Turn(false),
            Command::Brightness(42),
            Command::Color(Rgb::new(9, 8, 7)),
            Command::ColorTemp(4000),
        ] {
            assert_eq!(parse_command(&c.to_bytes()).unwrap(), c);
        }
    }

    #[test]
    fn parses_scan_reply() {
        let raw = br#"{"msg":{"cmd":"scan","data":{"ip":"192.168.1.23","device":"1F:80:C5:32:32:36:72:4E","sku":"H6076","bleVersionHard":"3.01.01","bleVersionSoft":"1.03.01","wifiVersionHard":"1.00.10","wifiVersionSoft":"1.02.03"}}}"#;
        match parse_incoming(raw).unwrap() {
            Incoming::Scan(s) => {
                assert_eq!(s.sku, "H6076");
                assert_eq!(s.ip, "192.168.1.23");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_status_reply() {
        let raw = br#"{"msg":{"cmd":"devStatus","data":{"onOff":1,"brightness":100,"color":{"r":255,"g":0,"b":0},"colorTemInKelvin":7200}}}"#;
        match parse_incoming(raw).unwrap() {
            Incoming::Status(s) => {
                assert_eq!(s.on_off, 1);
                assert_eq!(s.color, Rgb::new(255, 0, 0));
                assert_eq!(s.color_tem_in_kelvin, 7200);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
