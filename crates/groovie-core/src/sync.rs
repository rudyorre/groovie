//! Screen Sync spatial sampling, SKU layout defaults, auto-letterbox cropping,
//! and perceptual Oklab temporal smoothing.
//!
//! OS-specific screen capture lives in the desktop shell (`apps/desktop`) and
//! feeds downsampled [`SyncFrame`] grids (`64×36` by default) into this module
//! so `groovie-core` stays free of native GUI/OS dependencies.

use crate::protocol::Rgb;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const SYNC_GRID_W: usize = 64;
pub const SYNC_GRID_H: usize = 36;

const STAGE_MIN: f32 = -0.45;
const STAGE_MAX: f32 = 1.45;

/// Downsampled RGB frame captured from a display.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncFrame {
    pub width: usize,
    pub height: usize,
    /// Display aspect ratio (`width / height`, e.g. `16.0 / 9.0` or `21.0 / 9.0`).
    pub aspect: f32,
    /// Row-major RGB pixels (`width * height`).
    pub pixels: Vec<Rgb>,
}

impl SyncFrame {
    pub fn new(width: usize, height: usize, aspect: f32, pixels: Vec<Rgb>) -> Option<Self> {
        if width == 0 || height == 0 || pixels.len() != width.checked_mul(height)? {
            return None;
        }
        let aspect = if aspect.is_finite() && aspect > 0.2 {
            aspect
        } else {
            width as f32 / height as f32
        };
        Some(Self { width, height, aspect, pixels })
    }

    pub fn solid(width: usize, height: usize, color: Rgb) -> Self {
        Self {
            width,
            height,
            aspect: width as f32 / height.max(1) as f32,
            pixels: vec![color; width * height],
        }
    }
}

/// Connected monitor metadata reported by the OS capture backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayInfo {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SyncPreset {
    #[default]
    Game,
    Movie,
    Custom,
}

impl SyncPreset {
    /// Built-in `(smoothing, vibrance, min_glow)` values for `Game` and `Movie`.
    pub const fn tuning(self) -> Option<(u8, u8, u8)> {
        match self {
            SyncPreset::Game => Some((32, 65, 0)),
            SyncPreset::Movie => Some((68, 45, 15)),
            SyncPreset::Custom => None,
        }
    }

    pub const fn from_tuning(smoothing: u8, vibrance: u8, min_glow: u8) -> Self {
        match (smoothing, vibrance, min_glow) {
            (32, 65, 0) => SyncPreset::Game,
            (68, 45, 15) => SyncPreset::Movie,
            _ => SyncPreset::Custom,
        }
    }
}

/// Sub-unit within a Govee controller (`BarA` / `BarB` for 2-bar kits sharing one IP).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PlacementPart {
    #[default]
    Single,
    BarA,
    BarB,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Orientation {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PlacementMode {
    #[default]
    Segments,
    Solid,
}

/// 2D spatial placement of a light (or one bar of a 2-bar kit) around the monitor.
///
/// Monitor bounds are `[0.0..1.0] × [0.0..1.0]`; surrounding desk space extends
/// out to `[-0.45..1.45] × [-0.45..1.45]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPlacement {
    pub id: String,
    pub device_id: String,
    #[serde(default)]
    pub part: PlacementPart,
    #[serde(default)]
    pub excluded: bool,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub orientation: Orientation,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub mode: PlacementMode,
    pub segments: u8,
}

impl SyncPlacement {
    pub fn sanitize(&mut self) {
        self.x = if self.x.is_finite() { self.x.clamp(STAGE_MIN, STAGE_MAX) } else { 0.5 };
        self.y = if self.y.is_finite() { self.y.clamp(STAGE_MIN, STAGE_MAX) } else { 0.5 };
        self.segments = self.segments.clamp(1, 32);
    }
}

fn default_smoothing() -> u8 {
    32
}
fn default_vibrance() -> u8 {
    65
}
fn default_min_glow() -> u8 {
    0
}

/// Persistent user configuration for Screen Sync stored in `groovie.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_id: Option<String>,
    #[serde(default)]
    pub preset: SyncPreset,
    #[serde(default = "default_smoothing")]
    pub smoothing: u8,
    #[serde(default = "default_vibrance")]
    pub vibrance: u8,
    #[serde(default = "default_min_glow")]
    pub min_glow: u8,
    #[serde(default)]
    pub placements: Vec<SyncPlacement>,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            display_id: None,
            preset: SyncPreset::Game,
            smoothing: default_smoothing(),
            vibrance: default_vibrance(),
            min_glow: default_min_glow(),
            placements: Vec::new(),
        }
    }
}

impl SyncConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn sanitized(mut self) -> Self {
        self.display_id = self.display_id.take().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        self.smoothing = self.smoothing.min(100);
        self.vibrance = self.vibrance.min(100);
        self.min_glow = self.min_glow.min(100);
        self.preset = SyncPreset::from_tuning(self.smoothing, self.vibrance, self.min_glow);

        let mut seen = HashSet::new();
        self.placements.retain_mut(|p| {
            p.sanitize();
            seen.insert(p.id.clone())
        });
        self
    }
}

/// Normalized `[0..1]` rectangle inside the monitor frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleRect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Default for SampleRect {
    fn default() -> Self {
        Self { x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0 }
    }
}

/// Runtime + config snapshot of Screen Sync exposed to UI and tray.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub active: bool,
    pub active_devices: Vec<String>,
    pub config: SyncConfig,
}

/// Live preview of a single placement's sample region and smoothed segment colors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementPreview {
    pub id: String,
    pub sample: SampleRect,
    /// Ordered top-to-bottom (Vertical) or left-to-right (Horizontal) in screen space.
    pub colors: Vec<Rgb>,
}

/// Lightweight ~10 Hz preview payload emitted while the Screen Sync view is open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPreview {
    pub aspect: f32,
    pub crop: SampleRect,
    /// `[top, right, bottom, left]` ambient colours for tinting the monitor stage.
    pub edge_colors: [Rgb; 4],
    pub placements: Vec<PlacementPreview>,
}

// ---- SKU capability defaults ---------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkuSyncShape {
    /// Two independent vertical bars sharing a single Govee controller IP (`H6056`, `H6046`, `H6047`).
    DualBars { segments_per_bar: u8 },
    /// Multi-segment linear RGBIC lamp/strip supporting `cmd: "razer"` (`H6072`, `H607C`, `H619A`, ...).
    LinearRazer {
        segments: u8,
        orientation: Orientation,
        corner_lamp: bool,
    },
    /// Single-zone bulb/lamp defaulting to `cmd: "colorwc"` solid updates (`H6076`, `H6008`, unknown).
    SingleSolid { corner_lamp: bool },
}

pub fn sku_sync_shape(sku: &str) -> SkuSyncShape {
    match sku.trim().to_ascii_uppercase().as_str() {
        // Flow Plus / Flow Pro 2-bar kits: 6 segments per bar (slots 0..6 and 6..12).
        "H6056" | "H6043" => SkuSyncShape::DualBars { segments_per_bar: 6 },
        // Gaming / TV Light Bars 2-bar kits: 10 segments per bar (slots 0..10 and 10..20).
        "H6046" | "H6047" | "H6048" => SkuSyncShape::DualBars { segments_per_bar: 10 },
        // Razer-capable RGBIC Floor Lamps.
        "H6072" | "H6079" | "H607C" | "H6078" | "H6672" => SkuSyncShape::LinearRazer {
            segments: 15,
            orientation: Orientation::Vertical,
            corner_lamp: true,
        },
        // Non-Razer Floor Lamps (solid fallback by default).
        "H6076" | "H6073" => SkuSyncShape::SingleSolid { corner_lamp: true },
        // Glide / Neon / Wall bars.
        "H6061" | "H6062" | "H6065" | "H6066" | "H6067" | "H610A" | "H610B" => {
            SkuSyncShape::LinearRazer {
                segments: 10,
                orientation: Orientation::Horizontal,
                corner_lamp: false,
            }
        }
        // RGBIC LED strips & COB strips.
        "H619A" | "H619B" | "H619C" | "H619D" | "H619E" | "H619Z" | "H618A" | "H618C"
        | "H61A0" | "H61A1" | "H61A2" | "H61A3" | "H61BA" | "H61BC" | "H61BE" | "H612F"
        | "H61E5" => SkuSyncShape::LinearRazer {
            segments: 15,
            orientation: Orientation::Horizontal,
            corner_lamp: false,
        },
        _ => SkuSyncShape::SingleSolid { corner_lamp: false },
    }
}

pub fn placement_id(device_id: &str, part: PlacementPart) -> String {
    match part {
        PlacementPart::Single => device_id.to_string(),
        PlacementPart::BarA => format!("{device_id}#A"),
        PlacementPart::BarB => format!("{device_id}#B"),
    }
}

/// Ensure every known `(device_id, sku)` has placements in `cfg.placements` without
/// overwriting existing user coordinates or settings. Returns `true` if placements were added.
pub fn reconcile_placements<'a>(
    cfg: &mut SyncConfig,
    devices: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> bool {
    let mut existing: HashSet<String> = cfg.placements.iter().map(|p| p.id.clone()).collect();
    let mut sorted: Vec<(&str, &str)> = devices.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));

    let mut corner_idx = cfg
        .placements
        .iter()
        .filter(|p| p.part == PlacementPart::Single && p.x > 1.15)
        .count();
    let mut ambient_idx = 0usize;
    let mut changed = false;

    for (device_id, sku) in sorted {
        match sku_sync_shape(sku) {
            SkuSyncShape::DualBars { segments_per_bar } => {
                let id_a = placement_id(device_id, PlacementPart::BarA);
                if existing.insert(id_a.clone()) {
                    cfg.placements.push(SyncPlacement {
                        id: id_a,
                        device_id: device_id.to_string(),
                        part: PlacementPart::BarA,
                        excluded: false,
                        x: -0.08,
                        y: 0.50,
                        orientation: Orientation::Vertical,
                        reverse: false,
                        mode: PlacementMode::Segments,
                        segments: segments_per_bar,
                    });
                    changed = true;
                }
                let id_b = placement_id(device_id, PlacementPart::BarB);
                if existing.insert(id_b.clone()) {
                    cfg.placements.push(SyncPlacement {
                        id: id_b,
                        device_id: device_id.to_string(),
                        part: PlacementPart::BarB,
                        excluded: false,
                        x: 1.08,
                        y: 0.50,
                        orientation: Orientation::Vertical,
                        reverse: false,
                        mode: PlacementMode::Segments,
                        segments: segments_per_bar,
                    });
                    changed = true;
                }
            }
            SkuSyncShape::LinearRazer { segments, orientation, corner_lamp } => {
                let id = placement_id(device_id, PlacementPart::Single);
                if existing.insert(id.clone()) {
                    let (x, y) = if corner_lamp {
                        let pos = if corner_idx % 2 == 0 { (1.30, 0.46) } else { (-0.30, 0.46) };
                        corner_idx += 1;
                        pos
                    } else if orientation == Orientation::Horizontal {
                        (0.50, -0.10)
                    } else {
                        (-0.18, 0.50)
                    };
                    cfg.placements.push(SyncPlacement {
                        id,
                        device_id: device_id.to_string(),
                        part: PlacementPart::Single,
                        excluded: false,
                        x,
                        y,
                        orientation,
                        reverse: false,
                        mode: PlacementMode::Segments,
                        segments,
                    });
                    changed = true;
                }
            }
            SkuSyncShape::SingleSolid { corner_lamp } => {
                let id = placement_id(device_id, PlacementPart::Single);
                if existing.insert(id.clone()) {
                    let (x, y) = if corner_lamp {
                        let pos = if corner_idx % 2 == 0 { (1.30, 0.46) } else { (-0.30, 0.46) };
                        corner_idx += 1;
                        pos
                    } else {
                        let slots = [(-0.24, 0.50), (1.24, 0.50), (0.50, -0.18)];
                        let pos = slots[ambient_idx % slots.len()];
                        ambient_idx += 1;
                        pos
                    };
                    cfg.placements.push(SyncPlacement {
                        id,
                        device_id: device_id.to_string(),
                        part: PlacementPart::Single,
                        excluded: false,
                        x,
                        y,
                        orientation: Orientation::Vertical,
                        reverse: false,
                        mode: PlacementMode::Solid,
                        segments: 1,
                    });
                    changed = true;
                }
            }
        }
    }
    changed
}

/// Whether a device should use `cmd: "razer"` streaming given its SKU and active placements.
pub fn device_uses_razer(sku: &str, placements: &[&SyncPlacement]) -> bool {
    match sku_sync_shape(sku) {
        SkuSyncShape::DualBars { .. } => true,
        SkuSyncShape::LinearRazer { .. } => {
            placements.iter().any(|p| p.mode == PlacementMode::Segments)
        }
        SkuSyncShape::SingleSolid { .. } => {
            placements.iter().any(|p| p.mode == PlacementMode::Segments)
        }
    }
}

// ---- 2D Spatial Projection -----------------------------------------------

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Project a placement's 2D desk coordinates `(x, y)` onto the active content
/// rectangle `crop` inside `[0..1] × [0..1]`.
///
/// Distance outside the monitor bezel smoothly widens sampling depth from a
/// tight `~13%` edge strip up to a `~44%` room-wash region, continuous across `0` and `1`.
pub fn compute_sample_rect(p: &SyncPlacement, crop: SampleRect) -> SampleRect {
    let dx_out = if p.x < 0.0 { -p.x } else if p.x > 1.0 { p.x - 1.0 } else { 0.0 };
    let dy_out = if p.y < 0.0 { -p.y } else if p.y > 1.0 { p.y - 1.0 } else { 0.0 };
    let dist = (dx_out * dx_out + dy_out * dy_out).sqrt();
    let t = ((dist - 0.03) / 0.35).clamp(0.0, 1.0);

    // Perpendicular sampling depth: 13% right at the bezel -> 44% far out in the room.
    let depth = lerp(0.13, 0.44, t);

    let (nx0, ny0, nx1, ny1) = match p.orientation {
        Orientation::Vertical => {
            let anchor_x = p.x.clamp(0.0, 1.0);
            let x0 = anchor_x * (1.0 - depth);
            let x1 = x0 + depth;

            let off_center = ((p.y - 0.5).abs() - 0.15).max(0.0) / 0.5;
            let span_y = (lerp(0.92, 1.0, t) - 0.32 * off_center).clamp(0.45, 1.0);
            let cy = p.y.clamp(span_y * 0.5, 1.0 - span_y * 0.5);
            (x0, cy - span_y * 0.5, x1, cy + span_y * 0.5)
        }
        Orientation::Horizontal => {
            let anchor_y = p.y.clamp(0.0, 1.0);
            let y0 = anchor_y * (1.0 - depth);
            let y1 = y0 + depth;

            let off_center = ((p.x - 0.5).abs() - 0.15).max(0.0) / 0.5;
            let span_x = (lerp(0.92, 1.0, t) - 0.32 * off_center).clamp(0.45, 1.0);
            let cx = p.x.clamp(span_x * 0.5, 1.0 - span_x * 0.5);
            (cx - span_x * 0.5, y0, cx + span_x * 0.5, y1)
        }
    };

    let cw = (crop.x1 - crop.x0).max(0.1);
    let ch = (crop.y1 - crop.y0).max(0.1);
    SampleRect {
        x0: crop.x0 + nx0 * cw,
        y0: crop.y0 + ny0 * ch,
        x1: crop.x0 + nx1 * cw,
        y1: crop.y0 + ny1 * ch,
    }
}

/// Convert screen-space segment colors (`0` = top/left, `N - 1` = bottom/right)
/// into Govee hardware wire order.
///
/// Upright vertical Govee bars (`H6056`, `H6047`) and floor lamps (`H6072`) feed
/// power at the base, so hardware index `0` is at the physical bottom (`reverse = false`).
pub fn screen_to_wire_colors(
    orientation: Orientation,
    reverse: bool,
    screen_colors: &[Rgb],
) -> Vec<Rgb> {
    let flip = match orientation {
        Orientation::Vertical => !reverse,
        Orientation::Horizontal => reverse,
    };
    if flip {
        screen_colors.iter().copied().rev().collect()
    } else {
        screen_colors.to_vec()
    }
}

// ---- Auto Letterbox Black-Bar Tracker ------------------------------------

#[derive(Debug, Clone)]
struct LetterboxTracker {
    crop: SampleRect,
    candidate: SampleRect,
    stable_frames: u32,
}

impl Default for LetterboxTracker {
    fn default() -> Self {
        Self {
            crop: SampleRect::default(),
            candidate: SampleRect::default(),
            stable_frames: 0,
        }
    }
}

impl LetterboxTracker {
    fn update(&mut self, frame: &SyncFrame) -> SampleRect {
        let Some(detected) = detect_frame_crop(frame) else {
            // Pitch-black frame (fade to black): preserve current crop without oscillating.
            return self.crop;
        };

        let approx_eq = |a: SampleRect, b: SampleRect| {
            (a.x0 - b.x0).abs() < 0.025
                && (a.y0 - b.y0).abs() < 0.035
                && (a.x1 - b.x1).abs() < 0.025
                && (a.y1 - b.y1).abs() < 0.035
        };

        if approx_eq(detected, self.crop) {
            self.candidate = self.crop;
            self.stable_frames = 0;
            return self.crop;
        }

        if approx_eq(detected, self.candidate) {
            self.stable_frames = self.stable_frames.saturating_add(1);
        } else {
            self.candidate = detected;
            self.stable_frames = 1;
        }

        // Expand back to full screen quickly (5 frames) when content enters cropped bars;
        // narrow into letterbox mode conservatively (12 frames) so dark sky/ground won't trigger it.
        let expanding = (detected.x1 - detected.x0) > (self.crop.x1 - self.crop.x0)
            || (detected.y1 - detected.y0) > (self.crop.y1 - self.crop.y0);
        let threshold = if expanding { 5 } else { 12 };
        if self.stable_frames >= threshold {
            self.crop = detected;
            self.stable_frames = 0;
        }
        self.crop
    }
}

fn pixel_peak(c: Rgb) -> u8 {
    c.r.max(c.g).max(c.b)
}

fn detect_frame_crop(frame: &SyncFrame) -> Option<SampleRect> {
    let (w, h) = (frame.width, frame.height);
    if w < 8 || h < 8 {
        return Some(SampleRect::default());
    }

    // Require non-black interior content before deciding outer rows are letterbox bars.
    let mut center_lit = 0usize;
    let (cx0, cx1) = (w / 4, (w * 3) / 4);
    let (cy0, cy1) = (h / 4, (h * 3) / 4);
    for y in cy0..cy1 {
        for x in cx0..cx1 {
            if pixel_peak(frame.pixels[y * w + x]) > 14 {
                center_lit += 1;
            }
        }
    }
    if center_lit * 20 < (cx1 - cx0) * (cy1 - cy0) {
        return None;
    }

    let row_is_black = |y: usize| -> bool {
        let row = &frame.pixels[y * w..(y + 1) * w];
        let mut sum = 0u32;
        let mut bright = 0usize;
        for &px in row {
            let p = pixel_peak(px);
            sum += p as u32;
            if p > 8 {
                bright += 1;
            }
        }
        bright * 50 <= w && sum <= (w as u32) * 3
    };

    let col_is_black = |x: usize| -> bool {
        let mut sum = 0u32;
        let mut bright = 0usize;
        for y in 0..h {
            let p = pixel_peak(frame.pixels[y * w + x]);
            sum += p as u32;
            if p > 8 {
                bright += 1;
            }
        }
        bright * 50 <= h && sum <= (h as u32) * 3
    };

    let max_bar_y = (h * 22) / 100;
    let top_bars = (0..max_bar_y).take_while(|&y| row_is_black(y)).count();
    let bot_bars = (0..max_bar_y).take_while(|&i| row_is_black(h - 1 - i)).count();
    // Letterbox bars are symmetric on top and bottom (allow ±1 row rounding).
    let sym_y = if top_bars > 0 && bot_bars > 0 && top_bars.abs_diff(bot_bars) <= 1 {
        top_bars.min(bot_bars)
    } else {
        0
    };

    let max_bar_x = (w * 22) / 100;
    let left_bars = (0..max_bar_x).take_while(|&x| col_is_black(x)).count();
    let right_bars = (0..max_bar_x).take_while(|&i| col_is_black(w - 1 - i)).count();
    let sym_x = if left_bars > 0 && right_bars > 0 && left_bars.abs_diff(right_bars) <= 1 {
        left_bars.min(right_bars)
    } else {
        0
    };

    Some(SampleRect {
        x0: sym_x as f32 / w as f32,
        y0: sym_y as f32 / h as f32,
        x1: (w - sym_x) as f32 / w as f32,
        y1: (h - sym_y) as f32 / h as f32,
    })
}

// ---- Perceptual Oklab Color Math & Smoother ------------------------------

#[derive(Debug, Clone, Copy, Default)]
struct Oklab {
    l: f32,
    a: f32,
    b: f32,
}

fn srgb_u8_to_linear(v: u8) -> f32 {
    let x = v as f32 / 255.0;
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb_u8(v: f32) -> u8 {
    let x = v.clamp(0.0, 1.0);
    let s = if x <= 0.0031308 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

fn linear_rgb_to_oklab(r: f32, g: f32, b: f32) -> Oklab {
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).max(0.0);
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).max(0.0);
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).max(0.0);

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    Oklab {
        l: 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
        a: 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
        b: 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
    }
}

fn oklab_to_linear_rgb(l_val: f32, a: f32, b: f32) -> (f32, f32, f32) {
    let l_ = l_val + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l_val - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l_val - 0.089_484_18 * a - 1.291_485_5 * b;

    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;

    (
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    )
}

fn in_srgb_gamut(r: f32, g: f32, b: f32) -> bool {
    (-0.0001..=1.0001).contains(&r)
        && (-0.0001..=1.0001).contains(&g)
        && (-0.0001..=1.0001).contains(&b)
}

fn oklab_to_srgb(lab: Oklab, min_glow: u8) -> Rgb {
    if min_glow == 0 && lab.l < 0.032 {
        return Rgb::new(0, 0, 0);
    }
    let l = lab.l.clamp(0.0, 1.0);
    let (mut r, mut g, mut b) = oklab_to_linear_rgb(l, lab.a, lab.b);
    if !in_srgb_gamut(r, g, b) {
        // Scale chroma back onto the sRGB gamut boundary at constant Oklab lightness & hue
        // so vibrance boosts don't skew saturated blues toward purple when clamped.
        let mut lo = 0.0_f32;
        let mut hi = 1.0_f32;
        for _ in 0..8 {
            let mid = 0.5 * (lo + hi);
            let (mr, mg, mb) = oklab_to_linear_rgb(l, lab.a * mid, lab.b * mid);
            if in_srgb_gamut(mr, mg, mb) {
                lo = mid;
                r = mr;
                g = mg;
                b = mb;
            } else {
                hi = mid;
            }
        }
    }

    Rgb::new(linear_to_srgb_u8(r), linear_to_srgb_u8(g), linear_to_srgb_u8(b))
}

fn apply_vibrance_and_floor(mut lab: Oklab, vibrance: u8, min_glow: u8) -> Oklab {
    let chroma = (lab.a * lab.a + lab.b * lab.b).sqrt();
    if vibrance > 0 && chroma > 0.0001 {
        // Leave near-neutral whites/greys untouched so web pages and B&W scenes stay clean.
        let neutral_gate = ((chroma - 0.010) / 0.024).clamp(0.0, 1.0);
        let headroom = (1.0 - (chroma / 0.28).clamp(0.0, 1.0)).powf(0.6);
        let boost = 1.0 + (vibrance as f32 / 100.0) * 1.35 * neutral_gate * (0.35 + 0.65 * headroom);
        lab.a *= boost;
        lab.b *= boost;
    }

    if min_glow > 0 {
        let floor_l = (min_glow as f32 / 100.0) * 0.36;
        if lab.l < floor_l {
            let blend = ((floor_l - lab.l) / floor_l.max(0.001)).clamp(0.0, 1.0);
            lab.l = floor_l;
            if chroma < 0.012 {
                // Subtle warm cinema bias (~3200K) rather than cold blue-grey LED glow.
                lab.a += 0.005 * blend;
                lab.b += 0.014 * blend;
            }
        }
    }
    lab
}

fn smooth_oklab(cur: Oklab, target: Oklab, smoothing: u8, dt_secs: f32) -> Oklab {
    if smoothing == 0 || dt_secs <= 0.0 {
        return target;
    }
    let dl = target.l - cur.l;
    let da = target.a - cur.a;
    let db = target.b - cur.b;
    let delta_e = (dl * dl + da * da + db * db).sqrt();

    let s = smoothing as f32 / 100.0;
    let tau_slow = 0.020 + 0.88 * s * s;
    let tau_fast = 0.015 + 0.16 * s * s;
    // Large scene cuts / flashes accelerate toward `tau_fast`; gentle motion glides at `tau_slow`.
    let cut_t = ((delta_e - 0.06) / 0.24).clamp(0.0, 1.0);
    let mut tau = lerp(tau_slow, tau_fast, cut_t);
    if target.l < cur.l {
        //Slightly softer decay when fading into darkness prevents harsh strobe flicker.
        tau *= 1.25;
    }

    let alpha = (1.0 - (-dt_secs / tau.max(0.001)).exp()).clamp(0.0, 1.0);
    Oklab {
        l: cur.l + dl * alpha,
        a: cur.a + da * alpha,
        b: cur.b + db * alpha,
    }
}

fn sample_rect_linear_avg(frame: &SyncFrame, rect: SampleRect) -> (f32, f32, f32) {
    let w = frame.width as f32;
    let h = frame.height as f32;
    let gx0 = (rect.x0.clamp(0.0, 1.0) * w).min(w - 0.001);
    let gx1 = (rect.x1.clamp(0.0, 1.0) * w).max(gx0 + 0.001).min(w);
    let gy0 = (rect.y0.clamp(0.0, 1.0) * h).min(h - 0.001);
    let gy1 = (rect.y1.clamp(0.0, 1.0) * h).max(gy0 + 0.001).min(h);

    let ix0 = gx0.floor() as usize;
    let ix1 = gx1.ceil().min(w) as usize;
    let iy0 = gy0.floor() as usize;
    let iy1 = gy1.ceil().min(h) as usize;

    let mut sr = 0.0f32;
    let mut sg = 0.0f32;
    let mut sb = 0.0f32;
    let mut sw = 0.0f32;

    for y in iy0..iy1.max(iy0 + 1).min(frame.height) {
        let wy = (gy1.min((y + 1) as f32) - gy0.max(y as f32)).max(0.001);
        let row = y * frame.width;
        for x in ix0..ix1.max(ix0 + 1).min(frame.width) {
            let wx = (gx1.min((x + 1) as f32) - gx0.max(x as f32)).max(0.001);
            let weight = wx * wy;
            let px = frame.pixels[row + x];
            sr += srgb_u8_to_linear(px.r) * weight;
            sg += srgb_u8_to_linear(px.g) * weight;
            sb += srgb_u8_to_linear(px.b) * weight;
            sw += weight;
        }
    }

    if sw > 0.0 {
        (sr / sw, sg / sw, sb / sw)
    } else {
        (0.0, 0.0, 0.0)
    }
}

fn sample_rect_srgb(frame: &SyncFrame, rect: SampleRect) -> Rgb {
    let (r, g, b) = sample_rect_linear_avg(frame, rect);
    Rgb::new(linear_to_srgb_u8(r), linear_to_srgb_u8(g), linear_to_srgb_u8(b))
}

// ---- Stateful Frame Processor --------------------------------------------

#[derive(Debug, Clone)]
pub enum DeviceSyncCommand {
    /// Stream per-segment RGB colors via `cmd: "razer"`.
    Razer { colors: Vec<Rgb> },
    /// Fallback solid RGB update via `cmd: "colorwc"`.
    Solid { color: Rgb },
}

#[derive(Debug, Clone)]
pub struct StepOutput {
    pub commands: HashMap<String, DeviceSyncCommand>,
    pub preview: SyncPreview,
}

#[derive(Debug, Default)]
pub struct SyncProcessor {
    letterbox: LetterboxTracker,
    /// Per-placement smoothed Oklab state (`placement.id -> Vec<Oklab>` in screen order).
    smoothed: HashMap<String, Vec<Oklab>>,
}

impl SyncProcessor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.letterbox = LetterboxTracker::default();
        self.smoothed.clear();
    }

    /// Advance the color pipeline by `dt_secs` against `frame`, returning device
    /// wire commands for `active_devices` (`device_id -> sku`) and a UI preview.
    pub fn step(
        &mut self,
        frame: &SyncFrame,
        cfg: &SyncConfig,
        active_devices: &HashMap<String, String>,
        dt_secs: f32,
    ) -> StepOutput {
        let crop = self.letterbox.update(frame);
        let mut preview_placements = Vec::with_capacity(cfg.placements.len());
        let mut screen_by_placement: HashMap<String, Vec<Rgb>> = HashMap::new();

        for p in &cfg.placements {
            let sample = compute_sample_rect(p, crop);
            let seg_count = match p.mode {
                PlacementMode::Segments => p.segments.clamp(1, 32) as usize,
                PlacementMode::Solid => 1,
            };

            let prev_state = self.smoothed.entry(p.id.clone()).or_default();
            let initialized = prev_state.len() == seg_count;
            if !initialized {
                prev_state.resize(seg_count, Oklab::default());
            }

            let mut out_colors = Vec::with_capacity(seg_count);
            for i in 0..seg_count {
                let t0 = i as f32 / seg_count as f32;
                let t1 = (i + 1) as f32 / seg_count as f32;
                let sub = match p.orientation {
                    Orientation::Vertical => SampleRect {
                        x0: sample.x0,
                        x1: sample.x1,
                        y0: lerp(sample.y0, sample.y1, t0),
                        y1: lerp(sample.y0, sample.y1, t1),
                    },
                    Orientation::Horizontal => SampleRect {
                        x0: lerp(sample.x0, sample.x1, t0),
                        x1: lerp(sample.x0, sample.x1, t1),
                        y0: sample.y0,
                        y1: sample.y1,
                    },
                };
                let (lr, lg, lb) = sample_rect_linear_avg(frame, sub);
                let target = apply_vibrance_and_floor(
                    linear_rgb_to_oklab(lr, lg, lb),
                    cfg.vibrance,
                    cfg.min_glow,
                );
                let next = if initialized {
                    smooth_oklab(prev_state[i], target, cfg.smoothing, dt_secs)
                } else {
                    target
                };
                prev_state[i] = next;
                out_colors.push(oklab_to_srgb(next, cfg.min_glow));
            }

            preview_placements.push(PlacementPreview {
                id: p.id.clone(),
                sample,
                colors: out_colors.clone(),
            });
            screen_by_placement.insert(p.id.clone(), out_colors);
        }

        // Assemble per-device UDP wire commands for active devices.
        let mut commands = HashMap::new();
        for (dev_id, sku) in active_devices {
            let dev_placements: Vec<&SyncPlacement> = cfg
                .placements
                .iter()
                .filter(|p| &p.device_id == dev_id && !p.excluded)
                .collect();
            if dev_placements.is_empty() {
                continue;
            }

            match sku_sync_shape(sku) {
                SkuSyncShape::DualBars { segments_per_bar } => {
                    let find_part = |part: PlacementPart| -> Vec<Rgb> {
                        let p_opt = cfg
                            .placements
                            .iter()
                            .find(|p| &p.device_id == dev_id && p.part == part);
                        let Some(p) = p_opt else {
                            return vec![Rgb::new(0, 0, 0); segments_per_bar as usize];
                        };
                        let n = p.segments.clamp(1, 32) as usize;
                        if p.excluded {
                            return vec![Rgb::new(0, 0, 0); n];
                        }
                        let Some(sc) = screen_by_placement.get(&p.id) else {
                            return vec![Rgb::new(0, 0, 0); n];
                        };
                        if p.mode == PlacementMode::Solid {
                            let c = sc.first().copied().unwrap_or_default();
                            vec![c; n]
                        } else {
                            screen_to_wire_colors(p.orientation, p.reverse, sc)
                        }
                    };
                    let mut wire = find_part(PlacementPart::BarA);
                    wire.extend(find_part(PlacementPart::BarB));
                    commands.insert(dev_id.clone(), DeviceSyncCommand::Razer { colors: wire });
                }
                _ => {
                    let p = dev_placements[0];
                    let Some(sc) = screen_by_placement.get(&p.id) else { continue };
                    if device_uses_razer(sku, &dev_placements) {
                        let wire = screen_to_wire_colors(p.orientation, p.reverse, sc);
                        commands.insert(dev_id.clone(), DeviceSyncCommand::Razer { colors: wire });
                    } else {
                        let color = sc.first().copied().unwrap_or_default();
                        commands.insert(dev_id.clone(), DeviceSyncCommand::Solid { color });
                    }
                }
            }
        }

        let edge_colors = [
            sample_rect_srgb(frame, SampleRect { x0: crop.x0, y0: crop.y0, x1: crop.x1, y1: lerp(crop.y0, crop.y1, 0.18) }),
            sample_rect_srgb(frame, SampleRect { x0: lerp(crop.x0, crop.x1, 0.82), y0: crop.y0, x1: crop.x1, y1: crop.y1 }),
            sample_rect_srgb(frame, SampleRect { x0: crop.x0, y0: lerp(crop.y0, crop.y1, 0.82), x1: crop.x1, y1: crop.y1 }),
            sample_rect_srgb(frame, SampleRect { x0: crop.x0, y0: crop.y0, x1: lerp(crop.x0, crop.x1, 0.18), y1: crop.y1 }),
        ];

        StepOutput {
            commands,
            preview: SyncPreview {
                aspect: frame.aspect,
                crop,
                edge_colors,
                placements: preview_placements,
            },
        }
    }
}

/// Build an animated directional comet frame (in hardware wire order) for the
/// **Identify** button (`progress` in `0.0..=1.0`, sweeping top-to-bottom or left-to-right).
pub fn build_identify_razer_colors(
    cfg: &SyncConfig,
    device_id: &str,
    sku: &str,
    target_placement_id: &str,
    progress: f32,
) -> Vec<Rgb> {
    let sweep = |n: usize| -> Vec<Rgb> {
        let n = n.max(1);
        let head = progress.clamp(0.0, 1.0) * (n.saturating_sub(1) as f32);
        (0..n)
            .map(|i| {
                let d = (i as f32 - head).abs();
                let intensity = (1.0 - d / (n as f32 * 0.42).max(1.2)).clamp(0.0, 1.0);
                let hot = intensity * intensity;
                Rgb::new(
                    (30.0 + 225.0 * intensity).round() as u8,
                    (18.0 + 185.0 * hot).round() as u8,
                    (6.0 + 130.0 * hot * hot).round() as u8,
                )
            })
            .collect()
    };

    match sku_sync_shape(sku) {
        SkuSyncShape::DualBars { segments_per_bar } => {
            let build_bar = |part: PlacementPart| -> Vec<Rgb> {
                let p_opt = cfg
                    .placements
                    .iter()
                    .find(|p| p.device_id == device_id && p.part == part);
                let n = p_opt.map(|p| p.segments.clamp(1, 32) as usize).unwrap_or(segments_per_bar as usize);
                match p_opt {
                    Some(p) if p.id == target_placement_id => {
                        screen_to_wire_colors(p.orientation, p.reverse, &sweep(n))
                    }
                    _ => vec![Rgb::new(0, 0, 0); n],
                }
            };
            let mut wire = build_bar(PlacementPart::BarA);
            wire.extend(build_bar(PlacementPart::BarB));
            wire
        }
        _ => {
            let Some(p) = cfg.placements.iter().find(|p| p.id == target_placement_id) else {
                return vec![Rgb::new(255, 176, 32)];
            };
            let n = if p.mode == PlacementMode::Solid {
                1
            } else {
                p.segments.clamp(1, 32) as usize
            };
            screen_to_wire_colors(p.orientation, p.reverse, &sweep(n))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_split_bars_and_corner_lamp_defaults() {
        let mut cfg = SyncConfig::default();
        assert!(cfg.is_default());
        let added = reconcile_placements(
            &mut cfg,
            [("bar-dev", "H6056"), ("lamp-dev", "H6072"), ("bulb-dev", "H6008")],
        );
        assert!(added);
        assert_eq!(cfg.placements.len(), 4);

        let bar_a = cfg.placements.iter().find(|p| p.id == "bar-dev#A").unwrap();
        let bar_b = cfg.placements.iter().find(|p| p.id == "bar-dev#B").unwrap();
        let lamp = cfg.placements.iter().find(|p| p.id == "lamp-dev").unwrap();
        let bulb = cfg.placements.iter().find(|p| p.id == "bulb-dev").unwrap();

        assert!(bar_a.x < 0.0 && bar_b.x > 1.0);
        assert_eq!(bar_a.segments, 6);
        assert_eq!(bar_b.segments, 6);
        assert_eq!(lamp.segments, 15);
        assert!(lamp.x > 1.2, "floor lamp defaults to right corner");
        assert_eq!(bulb.mode, PlacementMode::Solid);

        // Reconciling again preserves user edits.
        cfg.placements[0].x = 0.25;
        assert!(!reconcile_placements(&mut cfg, [("bar-dev", "H6056")]));
        assert_eq!(cfg.placements[0].x, 0.25);
    }

    #[test]
    fn sample_depth_widens_smoothly_with_bezel_distance() {
        let crop = SampleRect::default();
        let snug = SyncPlacement {
            id: "a".into(),
            device_id: "d".into(),
            part: PlacementPart::BarA,
            excluded: false,
            x: -0.05,
            y: 0.5,
            orientation: Orientation::Vertical,
            reverse: false,
            mode: PlacementMode::Segments,
            segments: 6,
        };
        let corner = SyncPlacement { x: 1.35, part: PlacementPart::Single, ..snug.clone() };
        let on_bezel_left = SyncPlacement { x: -0.0001, ..snug.clone() };
        let on_bezel_in = SyncPlacement { x: 0.0001, ..snug.clone() };

        let r_snug = compute_sample_rect(&snug, crop);
        let r_corner = compute_sample_rect(&corner, crop);
        assert!((r_snug.x1 - r_snug.x0) < 0.18, "snug bar samples narrow edge: {r_snug:?}");
        assert!((r_corner.x1 - r_corner.x0) > 0.38, "corner lamp samples wider wash: {r_corner:?}");
        assert_eq!(r_corner.x1, 1.0);

        // Continuity across x = 0 bezel boundary.
        let rl = compute_sample_rect(&on_bezel_left, crop);
        let ri = compute_sample_rect(&on_bezel_in, crop);
        assert!((rl.x0 - ri.x0).abs() < 0.01 && (rl.x1 - ri.x1).abs() < 0.01);
    }

    #[test]
    fn detects_21_by_9_letterbox_bars_without_false_cropping_dark_scenes() {
        let mut tracker = LetterboxTracker::default();
        // 21:9 movie inside 16:9 frame: 5 black rows top and bottom, blue/amber content in middle.
        let mut pixels = vec![Rgb::new(0, 0, 0); SYNC_GRID_W * SYNC_GRID_H];
        for y in 5..(SYNC_GRID_H - 5) {
            for x in 0..SYNC_GRID_W {
                pixels[y * SYNC_GRID_W + x] = Rgb::new(40, 120, 220);
            }
        }
        let movie = SyncFrame::new(SYNC_GRID_W, SYNC_GRID_H, 16.0 / 9.0, pixels).unwrap();
        let mut crop = SampleRect::default();
        for _ in 0..15 {
            crop = tracker.update(&movie);
        }
        assert!(crop.y0 > 0.12 && crop.y1 < 0.88, "expected letterbox crop, got {crop:?}");

        // Pitch-black fade-out must NOT reset or oscillate the crop.
        let black = SyncFrame::solid(SYNC_GRID_W, SYNC_GRID_H, Rgb::new(0, 0, 0));
        for _ in 0..20 {
            assert_eq!(tracker.update(&black), crop);
        }

        // Fullscreen 16:9 game frame expands back to 0..1 within 5 frames.
        let full = SyncFrame::solid(SYNC_GRID_W, SYNC_GRID_H, Rgb::new(80, 160, 90));
        for _ in 0..6 {
            crop = tracker.update(&full);
        }
        assert_eq!(crop, SampleRect::default());
    }

    #[test]
    fn processor_maps_split_bars_bottom_to_top_and_preserves_neutral_greys() {
        let mut cfg = SyncConfig { smoothing: 0, vibrance: 100, min_glow: 0, ..Default::default() };
        reconcile_placements(&mut cfg, [("bars", "H6056"), ("grey", "H6008")]);
        // Put grey bulb in center of screen.
        cfg.placements.iter_mut().find(|p| p.id == "grey").unwrap().x = 0.5;

        // Frame: left half has RED at top, BLUE at bottom; center is neutral mid-grey; right half is GREEN.
        let mut px = vec![Rgb::new(128, 128, 128); SYNC_GRID_W * SYNC_GRID_H];
        for y in 0..SYNC_GRID_H {
            for x in 0..16 {
                px[y * SYNC_GRID_W + x] = if y < SYNC_GRID_H / 2 {
                    Rgb::new(255, 10, 10)
                } else {
                    Rgb::new(10, 20, 255)
                };
            }
            for x in 48..SYNC_GRID_W {
                px[y * SYNC_GRID_W + x] = Rgb::new(10, 240, 30);
            }
        }
        let frame = SyncFrame::new(SYNC_GRID_W, SYNC_GRID_H, 16.0 / 9.0, px).unwrap();
        let active = HashMap::from([
            ("bars".to_string(), "H6056".to_string()),
            ("grey".to_string(), "H6008".to_string()),
        ]);

        let mut proc = SyncProcessor::new();
        let out = proc.step(&frame, &cfg, &active, 1.0 / 30.0);

        // H6056 wire order: Bar A (0..6) is bottom-to-top -> slot 0 is BLUE (bottom), slot 5 is RED (top);
        // Bar B (6..12) is GREEN.
        let Some(DeviceSyncCommand::Razer { colors }) = out.commands.get("bars") else {
            panic!("expected razer command for H6056");
        };
        assert_eq!(colors.len(), 12);
        assert!(colors[0].b > 200 && colors[0].r < 50, "slot 0 (bottom of Bar A) should be blue: {:?}", colors[0]);
        assert!(colors[5].r > 200 && colors[5].b < 50, "slot 5 (top of Bar A) should be red: {:?}", colors[5]);
        assert!(colors[6].g > 200, "Bar B should be green: {:?}", colors[6]);

        // Neutral grey at max vibrance must remain neutral grey.
        let Some(DeviceSyncCommand::Solid { color }) = out.commands.get("grey") else {
            panic!("expected solid command for H6008");
        };
        assert!(color.r.abs_diff(color.g) <= 2 && color.g.abs_diff(color.b) <= 2, "grey drifted: {color:?}");
    }

    #[test]
    fn adaptive_smoother_snaps_faster_on_hard_cuts_than_small_drifts() {
        let start = linear_rgb_to_oklab(0.05, 0.05, 0.05);
        let drift = linear_rgb_to_oklab(0.09, 0.09, 0.09);
        let hard_cut = linear_rgb_to_oklab(0.95, 0.2, 0.05);

        let after_drift = smooth_oklab(start, drift, 70, 1.0 / 30.0);
        let after_cut = smooth_oklab(start, hard_cut, 70, 1.0 / 30.0);

        let frac_drift = (after_drift.l - start.l) / (drift.l - start.l);
        let frac_cut = (after_cut.l - start.l) / (hard_cut.l - start.l);
        assert!(frac_cut > frac_drift * 2.5, "hard cut ({frac_cut}) should advance faster than drift ({frac_drift})");
    }
}
