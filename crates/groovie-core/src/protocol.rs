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
    /// Enter (`true`) or exit (`false`) low-latency LAN segment streaming (`cmd: "razer"`).
    RazerEnable(bool),
    /// Stream per-segment RGB values (`1..=255` slots) in `cmd: "razer"` mode.
    /// `gradient = true` sets the `0x01` stretch/interpolation flag used by Govee DreamView/Razer.
    RazerFrame {
        gradient: bool,
        colors: Vec<Rgb>,
    },
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
            Command::RazerEnable(on) => {
                let pt = if *on { "uwABsQEK" } else { "uwABsQAL" };
                ("razer", json!({ "pt": pt }))
            }
            Command::RazerFrame { gradient, colors } => {
                let raw = encode_razer_raw(*gradient, colors);
                ("razer", json!({ "pt": b64_encode(&raw) }))
            }
        };
        json!({ "msg": { "cmd": cmd, "data": data } })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.to_json()).expect("command serialises")
    }

    /// Fast-path encoder for 30 Hz UDP streaming: writes compact `cmd: "razer"`
    /// JSON directly into `out` without intermediate `serde_json::Value` allocations.
    pub fn write_razer_frame_bytes(gradient: bool, colors: &[Rgb], out: &mut Vec<u8>) {
        const PREFIX: &[u8] = br#"{"msg":{"cmd":"razer","data":{"pt":""#;
        const SUFFIX: &[u8] = br#""}}}"#;
        let raw = encode_razer_raw(gradient, colors);
        out.clear();
        out.reserve(PREFIX.len() + raw.len().div_ceil(3) * 4 + SUFFIX.len());
        out.extend_from_slice(PREFIX);
        b64_encode_into(&raw, out);
        out.extend_from_slice(SUFFIX);
    }
}

fn encode_razer_raw(gradient: bool, colors: &[Rgb]) -> Vec<u8> {
    let count = colors.len().min(255);
    let payload_len = (2 + 3 * count) as u16;
    let mut raw = Vec::with_capacity(7 + 3 * count);
    raw.push(0xBB);
    raw.push((payload_len >> 8) as u8);
    raw.push((payload_len & 0xFF) as u8);
    raw.push(0xB0);
    raw.push(u8::from(gradient));
    raw.push(count as u8);
    for c in colors.iter().take(count) {
        raw.push(c.r);
        raw.push(c.g);
        raw.push(c.b);
    }
    let xor = raw.iter().fold(0u8, |acc, &b| acc ^ b);
    raw.push(xor);
    raw
}

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(input: &[u8]) -> String {
    let mut out = Vec::with_capacity(input.len().div_ceil(3) * 4);
    b64_encode_into(input, &mut out);
    // Safety: `B64_TABLE` and `'='` are pure ASCII.
    String::from_utf8(out).expect("base64 is valid ASCII")
}

fn b64_encode_into(input: &[u8], out: &mut Vec<u8>) {
    let mut chunks = input.chunks_exact(3);
    for c in &mut chunks {
        let n = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32);
        out.push(B64_TABLE[((n >> 18) & 0x3F) as usize]);
        out.push(B64_TABLE[((n >> 12) & 0x3F) as usize]);
        out.push(B64_TABLE[((n >> 6) & 0x3F) as usize]);
        out.push(B64_TABLE[(n & 0x3F) as usize]);
    }
    let rem = chunks.remainder();
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(B64_TABLE[((n >> 18) & 0x3F) as usize]);
            out.push(B64_TABLE[((n >> 12) & 0x3F) as usize]);
            out.push(b'=');
            out.push(b'=');
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(B64_TABLE[((n >> 18) & 0x3F) as usize]);
            out.push(B64_TABLE[((n >> 12) & 0x3F) as usize]);
            out.push(B64_TABLE[((n >> 6) & 0x3F) as usize]);
            out.push(b'=');
        }
        _ => {}
    }
}

fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !bytes.len().is_multiple_of(4) {
        return Err("invalid base64 length".into());
    }
    let val = |b: u8| -> Result<u8, String> {
        match b {
            b'A'..=b'Z' => Ok(b - b'A'),
            b'a'..=b'z' => Ok(b - b'a' + 26),
            b'0'..=b'9' => Ok(b - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(format!("invalid base64 byte {b:#x}")),
        }
    };
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks_exact(4) {
        let pad = chunk.iter().rev().take_while(|&&b| b == b'=').count();
        if pad > 2 || chunk[..4 - pad].contains(&b'=') {
            return Err("invalid base64 padding".into());
        }
        let a = val(chunk[0])? as u32;
        let b = val(chunk[1])? as u32;
        let c = if pad >= 2 { 0 } else { val(chunk[2])? as u32 };
        let d = if pad >= 1 { 0 } else { val(chunk[3])? as u32 };
        let n = (a << 18) | (b << 12) | (c << 6) | d;
        out.push(((n >> 16) & 0xFF) as u8);
        if pad < 2 {
            out.push(((n >> 8) & 0xFF) as u8);
        }
        if pad < 1 {
            out.push((n & 0xFF) as u8);
        }
    }
    Ok(out)
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

/// Peer-to-peer LAN lease announcement between Groovie instances on UDP port `4002`
/// so multiple computers running Groovie detect active Screen Sync and hand off cleanly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPeerLease {
    pub instance_id: String,
    pub host_name: String,
    pub active: bool,
    #[serde(default)]
    pub device_ids: Vec<String>,
    #[serde(default)]
    pub lease_epoch_ms: u64,
}

impl SyncPeerLease {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&json!({ "msg": { "cmd": "groovieSync", "data": self } }))
            .expect("peer lease serialises")
    }
}

/// Lightweight startup/rescan probe asking any peer Groovie instance currently
/// streaming Screen Sync on the LAN to announce its active lease immediately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPeerQuery {
    pub instance_id: String,
}

impl SyncPeerQuery {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&json!({ "msg": { "cmd": "groovieSyncQuery", "data": self } }))
            .expect("peer query serialises")
    }
}

/// Any message a device (or peer Groovie instance) may send to us.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Scan(ScanInfo),
    Status(StatusInfo),
    PeerSync(SyncPeerLease),
    PeerSyncQuery(SyncPeerQuery),
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
        "groovieSync" => Incoming::PeerSync(serde_json::from_value(env.msg.data)?),
        "groovieSyncQuery" => Incoming::PeerSyncQuery(serde_json::from_value(env.msg.data)?),
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
        "razer" => {
            let pt = d.get("pt").and_then(Value::as_str).ok_or("missing razer pt")?;
            parse_razer_payload(pt)?
        }
        other => return Err(format!("unknown cmd {other}")),
    })
}

fn parse_razer_payload(pt: &str) -> Result<Command, String> {
    let raw = b64_decode(pt)?;
    if raw.len() < 6 || raw[0] != 0xBB {
        return Err("invalid razer header".into());
    }
    if raw.iter().fold(0u8, |acc, &b| acc ^ b) != 0 {
        return Err("razer xor checksum mismatch".into());
    }
    let payload_len = u16::from_be_bytes([raw[1], raw[2]]) as usize;
    if raw.len() != 5 + payload_len {
        return Err(format!("razer length mismatch: expected {}, got {}", 5 + payload_len, raw.len()));
    }
    match raw[3] {
        0xB1 => {
            if payload_len != 1 {
                return Err("invalid razer enable length".into());
            }
            Ok(Command::RazerEnable(raw[4] != 0))
        }
        0xB0 => {
            if payload_len < 2 {
                return Err("invalid razer frame length".into());
            }
            let gradient = raw[4] != 0;
            let count = raw[5] as usize;
            if payload_len != 2 + 3 * count {
                return Err("razer segment count mismatch".into());
            }
            let mut colors = Vec::with_capacity(count);
            for rgb in raw[6..6 + 3 * count].chunks_exact(3) {
                colors.push(Rgb::new(rgb[0], rgb[1], rgb[2]));
            }
            Ok(Command::RazerFrame { gradient, colors })
        }
        sub => Err(format!("unknown razer subcommand {sub:#x}")),
    }
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
        assert_eq!(
            Command::RazerEnable(true).to_json(),
            json!({"msg":{"cmd":"razer","data":{"pt":"uwABsQEK"}}})
        );
        assert_eq!(
            Command::RazerEnable(false).to_json(),
            json!({"msg":{"cmd":"razer","data":{"pt":"uwABsQAL"}}})
        );
    }

    #[test]
    fn razer_golden_vector_and_fast_path_match() {
        // Golden 10-segment Razer frame vector:
        // bb 00 20 b0 01 0a (10 RGB triples) d5 -> "uwAgsAEKABH/JWzySsflbyLYlH3Ludi+3jOxA46kKOmXTUSK1Q=="
        let colors = vec![
            Rgb::new(0x00, 0x11, 0xFF),
            Rgb::new(0x25, 0x6C, 0xF2),
            Rgb::new(0x4A, 0xC7, 0xE5),
            Rgb::new(0x6F, 0x22, 0xD8),
            Rgb::new(0x94, 0x7D, 0xCB),
            Rgb::new(0xB9, 0xD8, 0xBE),
            Rgb::new(0xDE, 0x33, 0xB1),
            Rgb::new(0x03, 0x8E, 0xA4),
            Rgb::new(0x28, 0xE9, 0x97),
            Rgb::new(0x4D, 0x44, 0x8A),
        ];
        let cmd = Command::RazerFrame { gradient: true, colors: colors.clone() };
        assert_eq!(
            cmd.to_json(),
            json!({"msg":{"cmd":"razer","data":{"pt":"uwAgsAEKABH/JWzySsflbyLYlH3Ludi+3jOxA46kKOmXTUSK1Q=="}}})
        );
        let mut fast = Vec::new();
        Command::write_razer_frame_bytes(true, &colors, &mut fast);
        assert_eq!(parse_command(&fast).unwrap(), cmd);
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
            Command::RazerEnable(true),
            Command::RazerEnable(false),
            Command::RazerFrame {
                gradient: true,
                colors: vec![Rgb::new(255, 0, 128), Rgb::new(12, 34, 56)],
            },
            Command::RazerFrame {
                gradient: false,
                colors: vec![Rgb::new(1, 2, 3)],
            },
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

    #[test]
    fn parses_peer_sync_lease_and_query() {
        let lease = SyncPeerLease {
            instance_id: "inst-1".into(),
            host_name: "MacBook-Pro".into(),
            active: true,
            device_ids: vec!["AA:BB".into()],
            lease_epoch_ms: 1_700_000_000_123,
        };
        assert_eq!(parse_incoming(&lease.to_bytes()).unwrap(), Incoming::PeerSync(lease));

        let q = SyncPeerQuery { instance_id: "inst-2".into() };
        assert_eq!(parse_incoming(&q.to_bytes()).unwrap(), Incoming::PeerSyncQuery(q));
    }
}
