//! OS display enumeration and low-overhead screen capture for Screen Sync.
//!
//! - **Windows:** DXGI Desktop Duplication (`IDXGIOutput5::DuplicateOutput1` with
//!   HDR/SDR fallback to `IDXGIOutput1::DuplicateOutput`), downsampling GPU staging
//!   textures into `64×36` [`SyncFrame`]s at ~22 Hz and stopping on desktop lock.
//! - **macOS:** CoreGraphics display capture (`CGDisplayCreateImage`) across active
//!   displays with Retina/P3/EDR pixel decoding, Screen Recording permission checks,
//!   and automatic stop when `CGSessionCopyCurrentDictionary` reports screen lock.
//! - **Linux:** X11 + RandR monitor enumeration and root-window `ZPixmap` capture via
//!   `x11rb` (works on native X11 and XWayland sessions).

use groovie_core::protocol::Rgb;
use groovie_core::{DisplayInfo, Groovie, SyncFrame, SYNC_GRID_H, SYNC_GRID_W};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PixelLayout {
    Bgr8,
    Bgra8,
    Rgba8,
    Argb8,
    Abgr8,
    Rgb10A2,
    Bgr10A2Le,
    Bgr10A2Be,
    Rgba16F,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum SurfaceRotation {
    Identity,
    Rotate90,
    Rotate180,
    Rotate270,
}

#[allow(dead_code)]
struct RunningCapture {
    stop: Arc<AtomicBool>,
    display_id: Option<String>,
    handle: Option<JoinHandle<()>>,
}

pub struct CaptureManager {
    inner: Mutex<Option<RunningCapture>>,
}

impl Default for CaptureManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureManager {
    pub fn new() -> Self {
        Self { inner: Mutex::new(None) }
    }

    pub fn is_supported() -> bool {
        cfg!(any(target_os = "windows", target_os = "macos", target_os = "linux"))
    }

    pub fn list_displays() -> Vec<DisplayInfo> {
        #[cfg(target_os = "windows")]
        {
            win::list_displays()
        }
        #[cfg(target_os = "macos")]
        {
            mac::list_displays()
        }
        #[cfg(target_os = "linux")]
        {
            linux::list_displays()
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            Vec::new()
        }
    }

    pub fn start(&self, groovie: Groovie, display_id: Option<String>) -> Result<(), String> {
        let normalized = display_id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            let _ = (groovie, normalized);
            Err("Screen Sync is not supported on this platform".into())
        }

        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        {
            let mut guard = self.inner.lock().unwrap();
            if let Some(cur) = guard.as_ref() {
                let alive = cur.handle.as_ref().map(|h| !h.is_finished()).unwrap_or(false);
                if alive && !cur.stop.load(Ordering::SeqCst) && cur.display_id == normalized {
                    return Ok(());
                }
            }
            if let Some(prev) = guard.take() {
                prev.stop.store(true, Ordering::SeqCst);
                if let Some(h) = prev.handle {
                    std::thread::spawn(move || {
                        let _ = h.join();
                    });
                }
            }

            #[cfg(target_os = "windows")]
            win::verify_display_available(normalized.as_deref())?;
            #[cfg(target_os = "macos")]
            mac::verify_display_available(normalized.as_deref())?;
            #[cfg(target_os = "linux")]
            linux::verify_display_available(normalized.as_deref())?;

            let stop = Arc::new(AtomicBool::new(false));
            let stop_thread = stop.clone();
            let target_id = normalized.clone();
            let handle = std::thread::Builder::new()
                .name("groovie-screen-capture".into())
                .spawn(move || {
                    #[cfg(target_os = "windows")]
                    win::capture_loop(groovie, target_id, stop_thread);
                    #[cfg(target_os = "macos")]
                    mac::capture_loop(groovie, target_id, stop_thread);
                    #[cfg(target_os = "linux")]
                    linux::capture_loop(groovie, target_id, stop_thread);
                })
                .map_err(|e| format!("could not spawn capture thread: {e}"))?;

            *guard = Some(RunningCapture {
                stop,
                display_id: normalized,
                handle: Some(handle),
            });
            Ok(())
        }
    }

    pub fn stop(&self) {
        let prev = self.inner.lock().unwrap().take();
        if let Some(prev) = prev {
            prev.stop.store(true, Ordering::SeqCst);
            if let Some(h) = prev.handle {
                std::thread::spawn(move || {
                    let _ = h.join();
                });
            }
        }
    }
}

impl Drop for CaptureManager {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---- Cross-platform strided area downsampler & HDR pixel decoders --------

#[inline]
#[allow(dead_code)]
fn f16_to_f32(bits: u16) -> f32 {
    let sign = ((bits & 0x8000) as u32) << 16;
    let exp = ((bits >> 10) & 0x1f) as u32;
    let mant = (bits & 0x03ff) as u32;
    let f_bits = if exp == 0 {
        if mant == 0 {
            sign
        } else {
            let mut m = mant;
            let mut e = 127u32 - 14;
            while (m & 0x0400) == 0 {
                m <<= 1;
                e -= 1;
            }
            m &= 0x03ff;
            sign | (e << 23) | (m << 13)
        }
    } else if exp == 31 {
        // Clamp NaNs/Infs from HDR render targets safely to 0 or finite peak later.
        if mant == 0 { sign | 0x7f80_0000 } else { 0 }
    } else {
        sign | ((exp + 127 - 15) << 23) | (mant << 13)
    };
    f32::from_bits(f_bits)
}

#[inline]
#[allow(dead_code)]
fn linear_to_srgb_u8(v: f32) -> u8 {
    let x = v.clamp(0.0, 1.0);
    let s = if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

#[inline]
#[allow(dead_code)]
fn decode_pixel(bytes: &[u8], layout: PixelLayout) -> Rgb {
    match layout {
        PixelLayout::Bgr8 | PixelLayout::Bgra8 => Rgb::new(bytes[2], bytes[1], bytes[0]),
        PixelLayout::Rgba8 => Rgb::new(bytes[0], bytes[1], bytes[2]),
        PixelLayout::Argb8 => Rgb::new(bytes[1], bytes[2], bytes[3]),
        PixelLayout::Abgr8 => Rgb::new(bytes[3], bytes[2], bytes[1]),
        PixelLayout::Rgb10A2 => {
            let w = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            let r = ((w & 0x3ff) >> 2) as u8;
            let g = (((w >> 10) & 0x3ff) >> 2) as u8;
            let b = (((w >> 20) & 0x3ff) >> 2) as u8;
            Rgb::new(r, g, b)
        }
        PixelLayout::Bgr10A2Le => {
            let w = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            let r = (((w >> 20) & 0x3ff) >> 2) as u8;
            let g = (((w >> 10) & 0x3ff) >> 2) as u8;
            let b = ((w & 0x3ff) >> 2) as u8;
            Rgb::new(r, g, b)
        }
        PixelLayout::Bgr10A2Be => {
            let w = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            let r = (((w >> 20) & 0x3ff) >> 2) as u8;
            let g = (((w >> 10) & 0x3ff) >> 2) as u8;
            let b = ((w & 0x3ff) >> 2) as u8;
            Rgb::new(r, g, b)
        }
        PixelLayout::Rgba16F => {
            // Windows scRGB / macOS EDR linear float: 1.0 = SDR reference, ~2.0 = typical bright white.
            // Scale by 0.5 and apply gentle luminance Reinhard rolloff so both SDR-on-HDR and HDR scenes map naturally.
            let mut r = f16_to_f32(u16::from_le_bytes([bytes[0], bytes[1]]));
            let mut g = f16_to_f32(u16::from_le_bytes([bytes[2], bytes[3]]));
            let mut b = f16_to_f32(u16::from_le_bytes([bytes[4], bytes[5]]));
            r = if r.is_finite() { (r * 0.5).max(0.0) } else { 0.0 };
            g = if g.is_finite() { (g * 0.5).max(0.0) } else { 0.0 };
            b = if b.is_finite() { (b * 0.5).max(0.0) } else { 0.0 };
            let peak = r.max(g).max(b);
            let denom = 1.0 + 0.30 * peak;
            Rgb::new(
                linear_to_srgb_u8(r / denom),
                linear_to_srgb_u8(g / denom),
                linear_to_srgb_u8(b / denom),
            )
        }
    }
}

#[allow(dead_code)]
pub fn downsample_surface_to_frame(
    data: &[u8],
    row_pitch: usize,
    tex_w: usize,
    tex_h: usize,
    layout: PixelLayout,
    rotation: SurfaceRotation,
) -> Option<SyncFrame> {
    if tex_w == 0 || tex_h == 0 {
        return None;
    }
    let bpp = match layout {
        PixelLayout::Bgr8 => 3usize,
        PixelLayout::Bgra8
        | PixelLayout::Rgba8
        | PixelLayout::Argb8
        | PixelLayout::Abgr8
        | PixelLayout::Rgb10A2
        | PixelLayout::Bgr10A2Le
        | PixelLayout::Bgr10A2Be => 4usize,
        PixelLayout::Rgba16F => 8usize,
    };
    let min_bytes = tex_h.checked_sub(1)?.checked_mul(row_pitch)?.checked_add(tex_w.checked_mul(bpp)?)?;
    if data.len() < min_bytes {
        return None;
    }

    let (disp_w, disp_h) = match rotation {
        SurfaceRotation::Identity | SurfaceRotation::Rotate180 => (tex_w, tex_h),
        SurfaceRotation::Rotate90 | SurfaceRotation::Rotate270 => (tex_h, tex_w),
    };

    const TAPS: usize = 4;
    let mut pixels = Vec::with_capacity(SYNC_GRID_W * SYNC_GRID_H);

    for gy in 0..SYNC_GRID_H {
        let y0 = (gy * disp_h) / SYNC_GRID_H;
        let y1 = (((gy + 1) * disp_h) / SYNC_GRID_H).max(y0 + 1).min(disp_h);
        let span_y = y1 - y0;

        for gx in 0..SYNC_GRID_W {
            let x0 = (gx * disp_w) / SYNC_GRID_W;
            let x1 = (((gx + 1) * disp_w) / SYNC_GRID_W).max(x0 + 1).min(disp_w);
            let span_x = x1 - x0;

            let mut sum_r = 0u32;
            let mut sum_g = 0u32;
            let mut sum_b = 0u32;
            let mut count = 0u32;

            let ty_n = TAPS.min(span_y);
            let tx_n = TAPS.min(span_x);

            for ty in 0..ty_n {
                let dy = y0 + ((ty * 2 + 1) * span_y) / (ty_n * 2);
                for tx in 0..tx_n {
                    let dx = x0 + ((tx * 2 + 1) * span_x) / (tx_n * 2);
                    let (sx, sy) = match rotation {
                        SurfaceRotation::Identity => (dx, dy),
                        SurfaceRotation::Rotate90 => (dy, tex_h.saturating_sub(1 + dx)),
                        SurfaceRotation::Rotate180 => (tex_w.saturating_sub(1 + dx), tex_h.saturating_sub(1 + dy)),
                        SurfaceRotation::Rotate270 => (tex_w.saturating_sub(1 + dy), dx),
                    };
                    let off = sy * row_pitch + sx * bpp;
                    let c = decode_pixel(&data[off..off + bpp], layout);
                    sum_r += c.r as u32;
                    sum_g += c.g as u32;
                    sum_b += c.b as u32;
                    count += 1;
                }
            }

            if count > 0 {
                pixels.push(Rgb::new(
                    ((sum_r + count / 2) / count) as u8,
                    ((sum_g + count / 2) / count) as u8,
                    ((sum_b + count / 2) / count) as u8,
                ));
            } else {
                pixels.push(Rgb::new(0, 0, 0));
            }
        }
    }

    SyncFrame::new(SYNC_GRID_W, SYNC_GRID_H, disp_w as f32 / disp_h as f32, pixels)
}

// ---- Windows DXGI Desktop Duplication implementation ---------------------

#[cfg(target_os = "windows")]
mod win {
    use super::{downsample_surface_to_frame, PixelLayout, SurfaceRotation};
    use groovie_core::{DisplayInfo, Groovie};
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, HMODULE};
    use windows::Win32::Graphics::Direct3D::{
        D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0,
    };
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
        D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8X8_UNORM,
        DXGI_FORMAT_R10G10B10A2_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R8G8B8A8_UNORM,
        DXGI_MODE_ROTATION_ROTATE180, DXGI_MODE_ROTATION_ROTATE270, DXGI_MODE_ROTATION_ROTATE90,
        DXGI_SAMPLE_DESC,
    };
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1, IDXGIOutput5,
        IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_DENIED, DXGI_ERROR_ACCESS_LOST,
        DXGI_ERROR_NOT_FOUND, DXGI_ERROR_SESSION_DISCONNECTED, DXGI_ERROR_WAIT_TIMEOUT,
        DXGI_OUTDUPL_FRAME_INFO,
    };
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayDevicesW, GetMonitorInfoW, DISPLAY_DEVICEW, MONITORINFO, MONITORINFOEXW,
    };
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_CONTROL_FLAGS,
        DESKTOP_SWITCHDESKTOP, UOI_NAME,
    };

    const MONITORINFOF_PRIMARY_FLAG: u32 = 1;
    const DXGI_FORMAT_B8G8R8A8_UNORM_SRGB_VAL: i32 = 91;
    const DXGI_FORMAT_B8G8R8X8_UNORM_SRGB_VAL: i32 = 93;
    const DXGI_FORMAT_R8G8B8A8_UNORM_SRGB_VAL: i32 = 29;
    const CAPTURE_INTERVAL: Duration = Duration::from_millis(45);

    fn wide_to_string(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len]).trim().to_string()
    }

    fn short_display_number(device_name: &str, fallback_index: usize) -> String {
        if let Some(num) = device_name
            .rsplit('\\')
            .next()
            .and_then(|s| s.strip_prefix("DISPLAY"))
            .filter(|s| !s.is_empty())
        {
            format!("Display {num}")
        } else {
            format!("Display {}", fallback_index + 1)
        }
    }

    struct EnumeratedOutput {
        info: DisplayInfo,
        adapter: IDXGIAdapter1,
        output: IDXGIOutput,
        rotation: SurfaceRotation,
    }

    fn enumerate_outputs() -> Result<Vec<EnumeratedOutput>, String> {
        let factory: IDXGIFactory1 =
            unsafe { CreateDXGIFactory1() }.map_err(|e| format!("DXGI factory unavailable: {e}"))?;
        let mut results = Vec::new();
        let mut adapter_idx = 0u32;

        loop {
            let adapter = match unsafe { factory.EnumAdapters1(adapter_idx) } {
                Ok(a) => a,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(_) => break,
            };
            adapter_idx += 1;

            let mut output_idx = 0u32;
            loop {
                let output = match unsafe { adapter.EnumOutputs(output_idx) } {
                    Ok(o) => o,
                    Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(_) => break,
                };
                output_idx += 1;

                let Ok(desc) = (unsafe { output.GetDesc() }) else { continue };
                if !desc.AttachedToDesktop.as_bool() {
                    continue;
                }

                let id = wide_to_string(&desc.DeviceName);
                let rc = desc.DesktopCoordinates;
                let mut width = (rc.right - rc.left).unsigned_abs();
                let mut height = (rc.bottom - rc.top).unsigned_abs();
                let mut is_primary = rc.left == 0 && rc.top == 0;

                let mut mi = MONITORINFOEXW::default();
                mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
                if unsafe { GetMonitorInfoW(desc.Monitor, &mut mi.monitorInfo as *mut MONITORINFO) }.as_bool() {
                    is_primary = (mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY_FLAG) != 0;
                    let mrc = mi.monitorInfo.rcMonitor;
                    let mw = (mrc.right - mrc.left).unsigned_abs();
                    let mh = (mrc.bottom - mrc.top).unsigned_abs();
                    if mw > 0 && mh > 0 {
                        width = mw;
                        height = mh;
                    }
                }

                let mut dd = DISPLAY_DEVICEW::default();
                dd.cb = size_of::<DISPLAY_DEVICEW>() as u32;
                let model = if unsafe { EnumDisplayDevicesW(PCWSTR(desc.DeviceName.as_ptr()), 0, &mut dd, 0) }
                    .as_bool()
                {
                    let s = wide_to_string(&dd.DeviceString);
                    if s.is_empty()
                        || s.eq_ignore_ascii_case("Generic PnP Monitor")
                        || s.eq_ignore_ascii_case("Generic Non-PnP Monitor")
                    {
                        None
                    } else {
                        Some(s)
                    }
                } else {
                    None
                };

                let base_label = short_display_number(&id, results.len());
                let name = match model {
                    Some(m) => format!("{base_label} · {m}"),
                    None => base_label,
                };

                let rotation = if desc.Rotation == DXGI_MODE_ROTATION_ROTATE90 {
                    SurfaceRotation::Rotate90
                } else if desc.Rotation == DXGI_MODE_ROTATION_ROTATE180 {
                    SurfaceRotation::Rotate180
                } else if desc.Rotation == DXGI_MODE_ROTATION_ROTATE270 {
                    SurfaceRotation::Rotate270
                } else {
                    SurfaceRotation::Identity
                };

                results.push(EnumeratedOutput {
                    info: DisplayInfo { id, name, width, height, is_primary },
                    adapter: adapter.clone(),
                    output,
                    rotation,
                });
            }
        }

        results.sort_by(|a, b| b.info.is_primary.cmp(&a.info.is_primary).then_with(|| a.info.id.cmp(&b.info.id)));
        Ok(results)
    }

    pub fn list_displays() -> Vec<DisplayInfo> {
        enumerate_outputs()
            .map(|v| v.into_iter().map(|e| e.info).collect())
            .unwrap_or_default()
    }

    pub fn verify_display_available(preferred_id: Option<&str>) -> Result<(), String> {
        let outputs = enumerate_outputs()?;
        if outputs.is_empty() {
            return Err("No active display outputs found for Screen Sync".into());
        }
        let _ = preferred_id;
        Ok(())
    }

    fn is_input_desktop_locked() -> bool {
        unsafe {
            let Ok(hdesk) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_SWITCHDESKTOP) else {
                // Access denied or no interactive input desktop -> Winlogon lock screen or secure desktop.
                return true;
            };
            let mut name_buf = [0u16; 128];
            let mut needed = 0u32;
            let locked = if GetUserObjectInformationW(
                HANDLE(hdesk.0),
                UOI_NAME,
                Some(name_buf.as_mut_ptr() as *mut c_void),
                (name_buf.len() * size_of::<u16>()) as u32,
                Some(&mut needed),
            )
            .is_ok()
            {
                let name = wide_to_string(&name_buf);
                name.eq_ignore_ascii_case("Winlogon") || name.eq_ignore_ascii_case("Screensaver")
            } else {
                false
            };
            let _ = CloseDesktop(hdesk);
            locked
        }
    }

    struct DuplicatorSession {
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        dupl: IDXGIOutputDuplication,
        rotation: SurfaceRotation,
        staging: Option<(ID3D11Texture2D, u32, u32, DXGI_FORMAT)>,
    }

    enum OpenError {
        Locked,
        Failed(String),
    }

    fn open_duplicator(preferred_id: Option<&str>) -> Result<DuplicatorSession, OpenError> {
        if is_input_desktop_locked() {
            return Err(OpenError::Locked);
        }
        let outputs = enumerate_outputs().map_err(OpenError::Failed)?;
        let chosen = preferred_id
            .and_then(|id| outputs.iter().find(|o| o.info.id.eq_ignore_ascii_case(id)))
            .or_else(|| outputs.iter().find(|o| o.info.is_primary))
            .or_else(|| outputs.first())
            .ok_or_else(|| OpenError::Failed("no attached displays found".into()))?;

        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let feature_levels = [D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1];
        unsafe {
            D3D11CreateDevice(
                &chosen.adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|e| OpenError::Failed(format!("D3D11CreateDevice failed: {e}")))?;

        let device = device.ok_or_else(|| OpenError::Failed("missing D3D11 device".into()))?;
        let context = context.ok_or_else(|| OpenError::Failed("missing D3D11 context".into()))?;

        let dupl = unsafe {
            if let Ok(out5) = chosen.output.cast::<IDXGIOutput5>() {
                let hdr_formats = [
                    DXGI_FORMAT_R16G16B16A16_FLOAT,
                    DXGI_FORMAT_R10G10B10A2_UNORM,
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    DXGI_FORMAT_R8G8B8A8_UNORM,
                ];
                out5.DuplicateOutput1(&device, 0, &hdr_formats).or_else(|_| {
                    let out1: IDXGIOutput1 = chosen.output.cast()?;
                    out1.DuplicateOutput(&device)
                })
            } else {
                let out1: IDXGIOutput1 = chosen.output.cast().map_err(|e| {
                    OpenError::Failed(format!("display does not support IDXGIOutput1: {e}"))
                })?;
                out1.DuplicateOutput(&device)
            }
        }
        .map_err(|e| {
            if e.code() == DXGI_ERROR_ACCESS_DENIED && is_input_desktop_locked() {
                OpenError::Locked
            } else {
                OpenError::Failed(format!("DuplicateOutput failed: {e}"))
            }
        })?;

        Ok(DuplicatorSession {
            device,
            context,
            dupl,
            rotation: chosen.rotation,
            staging: None,
        })
    }

    fn map_format(fmt: DXGI_FORMAT) -> Option<PixelLayout> {
        if fmt == DXGI_FORMAT_B8G8R8A8_UNORM
            || fmt == DXGI_FORMAT_B8G8R8X8_UNORM
            || fmt.0 == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB_VAL
            || fmt.0 == DXGI_FORMAT_B8G8R8X8_UNORM_SRGB_VAL
        {
            Some(PixelLayout::Bgra8)
        } else if fmt == DXGI_FORMAT_R8G8B8A8_UNORM || fmt.0 == DXGI_FORMAT_R8G8B8A8_UNORM_SRGB_VAL {
            Some(PixelLayout::Rgba8)
        } else if fmt == DXGI_FORMAT_R10G10B10A2_UNORM {
            Some(PixelLayout::Rgb10A2)
        } else if fmt == DXGI_FORMAT_R16G16B16A16_FLOAT {
            Some(PixelLayout::Rgba16F)
        } else {
            None
        }
    }

    struct ReleaseFrameGuard<'a>(&'a IDXGIOutputDuplication);
    impl Drop for ReleaseFrameGuard<'_> {
        fn drop(&mut self) {
            unsafe {
                let _ = self.0.ReleaseFrame();
            }
        }
    }

    pub fn capture_loop(groovie: Groovie, display_id: Option<String>, stop: Arc<AtomicBool>) {
        let mut session: Option<DuplicatorSession> = None;
        let mut lock_strikes = 0u32;
        let mut reopen_failures = 0u32;
        let mut pushed_any = false;
        let mut last_lock_probe = Instant::now();

        while !stop.load(Ordering::SeqCst) {
            if session.is_none() {
                match open_duplicator(display_id.as_deref()) {
                    Ok(s) => {
                        session = Some(s);
                        lock_strikes = 0;
                        reopen_failures = 0;
                        pushed_any = false;
                    }
                    Err(OpenError::Locked) => {
                        lock_strikes += 1;
                        if lock_strikes >= 3 {
                            if !stop.load(Ordering::SeqCst) {
                                tauri::async_runtime::block_on(groovie.stop_sync(Some("screen locked")));
                            }
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(220));
                        continue;
                    }
                    Err(OpenError::Failed(_)) => {
                        reopen_failures += 1;
                        if reopen_failures >= 16 {
                            if !stop.load(Ordering::SeqCst) {
                                tauri::async_runtime::block_on(
                                    groovie.stop_sync(Some("display unavailable")),
                                );
                            }
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                        continue;
                    }
                }
            }

            let tick_start = Instant::now();
            let sess = session.as_mut().unwrap();
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;

            match unsafe { sess.dupl.AcquireNextFrame(55, &mut frame_info, &mut resource) } {
                Ok(()) => {
                    lock_strikes = 0;
                    let frame_guard = ReleaseFrameGuard(&sess.dupl);

                    // Skip mouse-pointer-only updates once we already have an initial desktop frame.
                    if frame_info.LastPresentTime == 0 && pushed_any {
                        drop(frame_guard);
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    }

                    let Some(res) = resource else {
                        drop(frame_guard);
                        continue;
                    };
                    let Ok(src_tex) = res.cast::<ID3D11Texture2D>() else {
                        drop(frame_guard);
                        continue;
                    };

                    let mut desc = D3D11_TEXTURE2D_DESC::default();
                    unsafe { src_tex.GetDesc(&mut desc) };

                    let Some(layout) = map_format(desc.Format) else {
                        drop(frame_guard);
                        continue;
                    };

                    let needs_new_staging = sess
                        .staging
                        .as_ref()
                        .map(|(_, w, h, f)| *w != desc.Width || *h != desc.Height || *f != desc.Format)
                        .unwrap_or(true);

                    if needs_new_staging {
                        let stage_desc = D3D11_TEXTURE2D_DESC {
                            Width: desc.Width,
                            Height: desc.Height,
                            MipLevels: 1,
                            ArraySize: 1,
                            Format: desc.Format,
                            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                            Usage: D3D11_USAGE_STAGING,
                            BindFlags: 0,
                            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                            MiscFlags: 0,
                        };
                        let mut new_tex: Option<ID3D11Texture2D> = None;
                        if unsafe { sess.device.CreateTexture2D(&stage_desc, None, Some(&mut new_tex)) }.is_err() {
                            drop(frame_guard);
                            continue;
                        }
                        if let Some(t) = new_tex {
                            sess.staging = Some((t, desc.Width, desc.Height, desc.Format));
                        } else {
                            drop(frame_guard);
                            continue;
                        }
                    }

                    let (staging_tex, tex_w, tex_h, _) = sess.staging.as_ref().unwrap();
                    unsafe { sess.context.CopyResource(staging_tex, &src_tex) };
                    // Return the compositor surface immediately before CPU readback & downsampling.
                    drop(src_tex);
                    drop(frame_guard);

                    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                    if unsafe { sess.context.Map(staging_tex, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.is_ok() {
                        let pitch = mapped.RowPitch as usize;
                        let total_bytes = pitch.saturating_mul(*tex_h as usize);
                        let slice = if !mapped.pData.is_null() && total_bytes > 0 {
                            Some(unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, total_bytes) })
                        } else {
                            None
                        };
                        let maybe_frame = slice.and_then(|bytes| {
                            downsample_surface_to_frame(
                                bytes,
                                pitch,
                                *tex_w as usize,
                                *tex_h as usize,
                                layout,
                                sess.rotation,
                            )
                        });
                        unsafe { sess.context.Unmap(staging_tex, 0) };

                        if let Some(frame) = maybe_frame {
                            if !stop.load(Ordering::SeqCst) {
                                pushed_any = true;
                                tauri::async_runtime::block_on(groovie.push_sync_frame(frame));
                            }
                        }
                    }
                }
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                    // Static desktop: zero GPU reads. Periodically probe for lock screen just in case.
                    if last_lock_probe.elapsed() >= Duration::from_millis(900) {
                        last_lock_probe = Instant::now();
                        if is_input_desktop_locked() {
                            session = None;
                            lock_strikes = 3;
                            if !stop.load(Ordering::SeqCst) {
                                tauri::async_runtime::block_on(groovie.stop_sync(Some("screen locked")));
                            }
                            return;
                        }
                    }
                    continue;
                }
                Err(e) if e.code() == DXGI_ERROR_SESSION_DISCONNECTED => {
                    if !stop.load(Ordering::SeqCst) {
                        tauri::async_runtime::block_on(groovie.stop_sync(Some("session disconnected")));
                    }
                    return;
                }
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST || e.code() == DXGI_ERROR_ACCESS_DENIED => {
                    // Happens on fullscreen mode switch, HDR toggle, or Win+L lock screen.
                    session = None;
                    std::thread::sleep(Duration::from_millis(150));
                    continue;
                }
                Err(_) => {
                    session = None;
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
            }

            let elapsed = tick_start.elapsed();
            if let Some(rem) = CAPTURE_INTERVAL.checked_sub(elapsed) {
                std::thread::sleep(rem);
            }
        }
    }
}

#[allow(dead_code)]
fn map_cg_bitmap(bits_per_pixel: usize, bits_per_component: usize, bitmap_info: u32) -> Option<PixelLayout> {
    const ALPHA_MASK: u32 = 0x1F;
    const FLOAT_COMPONENTS: u32 = 1 << 8;
    const BYTE_ORDER_MASK: u32 = 0x7000;
    const BYTE_ORDER_32_LITTLE: u32 = 2 << 12;
    const BYTE_ORDER_32_BIG: u32 = 4 << 12;

    if bits_per_pixel == 64 && bits_per_component == 16 && (bitmap_info & FLOAT_COMPONENTS) != 0 {
        return Some(PixelLayout::Rgba16F);
    }
    if bits_per_pixel == 32 && bits_per_component == 10 {
        return match bitmap_info & BYTE_ORDER_MASK {
            BYTE_ORDER_32_LITTLE => Some(PixelLayout::Bgr10A2Le),
            _ => Some(PixelLayout::Bgr10A2Be),
        };
    }
    if bits_per_pixel != 32 || bits_per_component != 8 {
        return None;
    }

    let alpha_info = bitmap_info & ALPHA_MASK;
    // CGImageAlphaInfo: 3 = PremultipliedFirst, 4 = First, 6 = NoneSkipFirst
    let alpha_first = matches!(alpha_info, 3 | 4 | 6);
    match bitmap_info & BYTE_ORDER_MASK {
        BYTE_ORDER_32_LITTLE => {
            if alpha_first {
                Some(PixelLayout::Bgra8)
            } else {
                Some(PixelLayout::Abgr8)
            }
        }
        BYTE_ORDER_32_BIG | 0 => {
            if alpha_first {
                Some(PixelLayout::Argb8)
            } else {
                Some(PixelLayout::Rgba8)
            }
        }
        _ => None,
    }
}

// ---- macOS CoreGraphics implementation ------------------------------------

#[cfg(target_os = "macos")]
mod mac {
    use super::{downsample_surface_to_frame, map_cg_bitmap, SurfaceRotation};
    use groovie_core::{DisplayInfo, Groovie};
    use std::ffi::c_void;
    use std::os::raw::c_char;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    type CGDirectDisplayID = u32;
    type CGError = i32;
    type CFIndex = isize;
    type CFTypeRef = *const c_void;
    type CFDataRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFBooleanRef = *const c_void;
    type CGImageRef = *const c_void;
    type CGDataProviderRef = *const c_void;

    const K_CG_ERROR_SUCCESS: CGError = 0;
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const CAPTURE_INTERVAL: Duration = Duration::from_millis(45);

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGGetActiveDisplayList(
            max_displays: u32,
            active_displays: *mut CGDirectDisplayID,
            display_count: *mut u32,
        ) -> CGError;
        fn CGMainDisplayID() -> CGDirectDisplayID;
        fn CGDisplayPixelsWide(display: CGDirectDisplayID) -> usize;
        fn CGDisplayPixelsHigh(display: CGDirectDisplayID) -> usize;
        fn CGDisplayIsBuiltin(display: CGDirectDisplayID) -> u32;
        fn CGDisplayCreateImage(display_id: CGDirectDisplayID) -> CGImageRef;
        fn CGImageGetWidth(image: CGImageRef) -> usize;
        fn CGImageGetHeight(image: CGImageRef) -> usize;
        fn CGImageGetBitsPerComponent(image: CGImageRef) -> usize;
        fn CGImageGetBitsPerPixel(image: CGImageRef) -> usize;
        fn CGImageGetBytesPerRow(image: CGImageRef) -> usize;
        fn CGImageGetBitmapInfo(image: CGImageRef) -> u32;
        fn CGImageGetDataProvider(image: CGImageRef) -> CGDataProviderRef;
        fn CGDataProviderCopyData(provider: CGDataProviderRef) -> CFDataRef;
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
        fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFDataGetBytePtr(the_data: CFDataRef) -> *const u8;
        fn CFDataGetLength(the_data: CFDataRef) -> CFIndex;
        fn CFRelease(cf: CFTypeRef);
        fn CFStringCreateWithCString(
            alloc: CFTypeRef,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFDictionaryGetValue(the_dict: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef;
        fn CFBooleanGetValue(boolean: CFBooleanRef) -> u8;
    }

    struct CfGuard(CFTypeRef);
    impl Drop for CfGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) };
            }
        }
    }

    fn active_display_ids() -> Result<Vec<CGDirectDisplayID>, String> {
        let mut ids = [0 as CGDirectDisplayID; 16];
        let mut count = 0u32;
        let rc = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
        if rc != K_CG_ERROR_SUCCESS {
            return Err(format!("CoreGraphics display enumeration failed ({rc})"));
        }
        let n = (count as usize).min(ids.len());
        Ok(ids[..n].to_vec())
    }

    pub fn list_displays() -> Vec<DisplayInfo> {
        let Ok(ids) = active_display_ids() else {
            return Vec::new();
        };
        let main_id = unsafe { CGMainDisplayID() };
        let mut out = Vec::with_capacity(ids.len());
        for (idx, id) in ids.into_iter().enumerate() {
            let width = unsafe { CGDisplayPixelsWide(id) } as u32;
            let height = unsafe { CGDisplayPixelsHigh(id) } as u32;
            if width == 0 || height == 0 {
                continue;
            }
            let is_primary = id == main_id;
            let builtin = unsafe { CGDisplayIsBuiltin(id) != 0 };
            let name = if builtin {
                format!("Display {} · Built-in Display", idx + 1)
            } else {
                format!("Display {}", idx + 1)
            };
            out.push(DisplayInfo {
                id: id.to_string(),
                name,
                width,
                height,
                is_primary,
            });
        }
        out.sort_by(|a, b| b.is_primary.cmp(&a.is_primary).then_with(|| a.id.cmp(&b.id)));
        out
    }

    pub fn verify_display_available(preferred_id: Option<&str>) -> Result<(), String> {
        let displays = list_displays();
        if displays.is_empty() {
            return Err("No active displays found for Screen Sync".into());
        }
        let _ = preferred_id;
        let granted = unsafe { CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() };
        if !granted {
            return Err(
                "Screen Recording permission is required — enable Groovie in System Settings → Privacy & Security → Screen & System Audio Recording"
                    .into(),
            );
        }
        Ok(())
    }

    fn is_session_locked() -> bool {
        unsafe {
            let dict = CGSessionCopyCurrentDictionary();
            if dict.is_null() {
                return false;
            }
            let _dict_guard = CfGuard(dict);
            let key = CFStringCreateWithCString(
                std::ptr::null(),
                c"CGSSessionScreenIsLocked".as_ptr(),
                K_CF_STRING_ENCODING_UTF8,
            );
            if key.is_null() {
                return false;
            }
            let _key_guard = CfGuard(key);
            let val = CFDictionaryGetValue(dict, key);
            !val.is_null() && CFBooleanGetValue(val) != 0
        }
    }

    fn resolve_target_display(preferred_id: Option<&str>) -> Option<CGDirectDisplayID> {
        let ids = active_display_ids().ok()?;
        if ids.is_empty() {
            return None;
        }
        if let Some(raw) = preferred_id {
            if let Ok(parsed) = raw.parse::<CGDirectDisplayID>() {
                if ids.contains(&parsed) {
                    return Some(parsed);
                }
            }
        }
        let main = unsafe { CGMainDisplayID() };
        if ids.contains(&main) {
            Some(main)
        } else {
            ids.into_iter().next()
        }
    }

    pub fn capture_loop(groovie: Groovie, display_id: Option<String>, stop: Arc<AtomicBool>) {
        let mut reopen_failures = 0u32;
        let mut last_lock_probe = Instant::now();

        while !stop.load(Ordering::SeqCst) {
            let tick_start = Instant::now();

            if last_lock_probe.elapsed() >= Duration::from_millis(850) {
                last_lock_probe = Instant::now();
                if is_session_locked() {
                    if !stop.load(Ordering::SeqCst) {
                        tauri::async_runtime::block_on(groovie.stop_sync(Some("screen locked")));
                    }
                    return;
                }
            }

            let Some(target_id) = resolve_target_display(display_id.as_deref()) else {
                reopen_failures += 1;
                if reopen_failures >= 16 {
                    if !stop.load(Ordering::SeqCst) {
                        tauri::async_runtime::block_on(groovie.stop_sync(Some("display unavailable")));
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
                continue;
            };

            let img = unsafe { CGDisplayCreateImage(target_id) };
            if img.is_null() {
                if is_session_locked() {
                    if !stop.load(Ordering::SeqCst) {
                        tauri::async_runtime::block_on(groovie.stop_sync(Some("screen locked")));
                    }
                    return;
                }
                reopen_failures += 1;
                if reopen_failures >= 16 {
                    if !stop.load(Ordering::SeqCst) {
                        tauri::async_runtime::block_on(groovie.stop_sync(Some("display unavailable")));
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            reopen_failures = 0;
            let _img_guard = CfGuard(img);

            let tex_w = unsafe { CGImageGetWidth(img) };
            let tex_h = unsafe { CGImageGetHeight(img) };
            let bpc = unsafe { CGImageGetBitsPerComponent(img) };
            let bpp = unsafe { CGImageGetBitsPerPixel(img) };
            let pitch = unsafe { CGImageGetBytesPerRow(img) };
            let bitmap_info = unsafe { CGImageGetBitmapInfo(img) };

            if let Some(layout) = map_cg_bitmap(bpp, bpc, bitmap_info) {
                let provider = unsafe { CGImageGetDataProvider(img) };
                if !provider.is_null() {
                    let cf_data = unsafe { CGDataProviderCopyData(provider) };
                    if !cf_data.is_null() {
                        let _data_guard = CfGuard(cf_data);
                        let ptr = unsafe { CFDataGetBytePtr(cf_data) };
                        let len = unsafe { CFDataGetLength(cf_data) };
                        if !ptr.is_null() && len > 0 {
                            let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
                            if let Some(frame) = downsample_surface_to_frame(
                                bytes,
                                pitch,
                                tex_w,
                                tex_h,
                                layout,
                                SurfaceRotation::Identity,
                            ) {
                                if !stop.load(Ordering::SeqCst) {
                                    tauri::async_runtime::block_on(groovie.push_sync_frame(frame));
                                }
                            }
                        }
                    }
                }
            }

            let elapsed = tick_start.elapsed();
            if let Some(rem) = CAPTURE_INTERVAL.checked_sub(elapsed) {
                std::thread::sleep(rem);
            }
        }
    }
}

// ---- Linux X11 / RandR implementation -------------------------------------

#[cfg(target_os = "linux")]
mod linux {
    use super::{downsample_surface_to_frame, PixelLayout, SurfaceRotation};
    use groovie_core::{DisplayInfo, Groovie};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder};
    use x11rb::rust_connection::RustConnection;

    const CAPTURE_INTERVAL: Duration = Duration::from_millis(45);

    struct MonitorTarget {
        info: DisplayInfo,
        x: i16,
        y: i16,
        w: u16,
        h: u16,
    }

    fn connect_x11() -> Result<(RustConnection, usize), String> {
        x11rb::connect(None).map_err(|e| {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_none() {
                "Screen Sync on Linux requires X11 or XWayland (DISPLAY is not set in this Wayland session)"
                    .to_string()
            } else {
                format!("Could not connect to X11 display for Screen Sync: {e}")
            }
        })
    }

    fn enumerate_monitors(conn: &RustConnection, screen_num: usize) -> Vec<MonitorTarget> {
        let Some(screen) = conn.setup().roots.get(screen_num) else {
            return Vec::new();
        };
        let root_w = screen.width_in_pixels;
        let root_h = screen.height_in_pixels;
        let mut results = Vec::new();

        if let Ok(cookie) = conn.randr_get_monitors(screen.root, true) {
            if let Ok(reply) = cookie.reply() {
                for (idx, m) in reply.monitors.into_iter().enumerate() {
                    let sx = m.x.max(0);
                    let sy = m.y.max(0);
                    let max_w = (root_w as i32 - sx as i32).max(0) as u16;
                    let max_h = (root_h as i32 - sy as i32).max(0) as u16;
                    let sw = m.width.min(max_w);
                    let sh = m.height.min(max_h);
                    if sw == 0 || sh == 0 {
                        continue;
                    }
                    let atom_label = conn
                        .get_atom_name(m.name)
                        .ok()
                        .and_then(|c| c.reply().ok())
                        .map(|r| String::from_utf8_lossy(&r.name).trim().to_string())
                        .filter(|s| !s.is_empty());
                    let id = atom_label.clone().unwrap_or_else(|| format!("xrandr-{idx}"));
                    let name = match atom_label {
                        Some(lbl) => format!("Display {} · {lbl}", idx + 1),
                        None => format!("Display {}", idx + 1),
                    };
                    results.push(MonitorTarget {
                        info: DisplayInfo {
                            id,
                            name,
                            width: sw as u32,
                            height: sh as u32,
                            is_primary: m.primary,
                        },
                        x: sx,
                        y: sy,
                        w: sw,
                        h: sh,
                    });
                }
            }
        }

        if results.is_empty() && root_w > 0 && root_h > 0 {
            results.push(MonitorTarget {
                info: DisplayInfo {
                    id: "x11-default".into(),
                    name: "Primary display".into(),
                    width: root_w as u32,
                    height: root_h as u32,
                    is_primary: true,
                },
                x: 0,
                y: 0,
                w: root_w,
                h: root_h,
            });
        }

        if !results.is_empty() && !results.iter().any(|m| m.info.is_primary) {
            results[0].info.is_primary = true;
        }
        results.sort_by(|a, b| {
            b.info
                .is_primary
                .cmp(&a.info.is_primary)
                .then_with(|| a.info.id.cmp(&b.info.id))
        });
        results
    }

    pub fn list_displays() -> Vec<DisplayInfo> {
        let Ok((conn, screen_num)) = connect_x11() else {
            return Vec::new();
        };
        enumerate_monitors(&conn, screen_num)
            .into_iter()
            .map(|m| m.info)
            .collect()
    }

    pub fn verify_display_available(preferred_id: Option<&str>) -> Result<(), String> {
        let (conn, screen_num) = connect_x11()?;
        let monitors = enumerate_monitors(&conn, screen_num);
        let chosen = preferred_id
            .and_then(|id| monitors.iter().find(|m| m.info.id == id))
            .or_else(|| monitors.iter().find(|m| m.info.is_primary))
            .or_else(|| monitors.first())
            .ok_or_else(|| "No active X11 display outputs found for Screen Sync".to_string())?;
        let screen = &conn.setup().roots[screen_num];
        conn.get_image(ImageFormat::Z_PIXMAP, screen.root, chosen.x, chosen.y, 1, 1, !0)
            .map_err(|e| format!("X11 capture request failed: {e}"))?
            .reply()
            .map_err(|e| {
                format!(
                    "X11 screen capture was rejected ({e}) — on Wayland, direct root-window readback may be restricted"
                )
            })?;
        Ok(())
    }

    pub fn capture_loop(groovie: Groovie, display_id: Option<String>, stop: Arc<AtomicBool>) {
        let mut session: Option<(RustConnection, usize)> = None;
        let mut reopen_failures = 0u32;

        while !stop.load(Ordering::SeqCst) {
            if session.is_none() {
                match connect_x11() {
                    Ok(pair) => {
                        session = Some(pair);
                        reopen_failures = 0;
                    }
                    Err(_) => {
                        reopen_failures += 1;
                        if reopen_failures >= 16 {
                            if !stop.load(Ordering::SeqCst) {
                                tauri::async_runtime::block_on(
                                    groovie.stop_sync(Some("display unavailable")),
                                );
                            }
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                        continue;
                    }
                }
            }

            let tick_start = Instant::now();
            let (conn, screen_num) = session.as_ref().unwrap();
            let monitors = enumerate_monitors(conn, *screen_num);
            let Some(chosen) = display_id
                .as_deref()
                .and_then(|id| monitors.iter().find(|m| m.info.id == id))
                .or_else(|| monitors.iter().find(|m| m.info.is_primary))
                .or_else(|| monitors.first())
            else {
                session = None;
                std::thread::sleep(Duration::from_millis(250));
                continue;
            };

            let root = conn.setup().roots[*screen_num].root;
            let byte_order = conn.setup().image_byte_order;
            let reply = conn
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    root,
                    chosen.x,
                    chosen.y,
                    chosen.w,
                    chosen.h,
                    !0,
                )
                .ok()
                .and_then(|c| c.reply().ok());

            let Some(img) = reply else {
                session = None;
                std::thread::sleep(Duration::from_millis(200));
                continue;
            };

            let w_usize = chosen.w as usize;
            let h_usize = chosen.h as usize;
            if w_usize > 0 && h_usize > 0 && img.data.len() >= h_usize {
                let pitch = img.data.len() / h_usize;
                let layout = if pitch >= w_usize * 4 {
                    if byte_order == ImageOrder::MSB_FIRST {
                        Some(PixelLayout::Argb8)
                    } else {
                        Some(PixelLayout::Bgra8)
                    }
                } else if pitch >= w_usize * 3 && byte_order == ImageOrder::LSB_FIRST {
                    Some(PixelLayout::Bgr8)
                } else {
                    None
                };

                if let Some(layout) = layout {
                    if let Some(frame) = downsample_surface_to_frame(
                        &img.data,
                        pitch,
                        w_usize,
                        h_usize,
                        layout,
                        SurfaceRotation::Identity,
                    ) {
                        if !stop.load(Ordering::SeqCst) {
                            tauri::async_runtime::block_on(groovie.push_sync_frame(frame));
                        }
                    }
                }
            }

            let elapsed = tick_start.elapsed();
            if let Some(rem) = CAPTURE_INTERVAL.checked_sub(elapsed) {
                std::thread::sleep(rem);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_all_pixel_layouts_cleanly() {
        assert_eq!(decode_pixel(&[30, 120, 240], PixelLayout::Bgr8), Rgb::new(240, 120, 30));
        assert_eq!(decode_pixel(&[30, 120, 240, 255], PixelLayout::Bgra8), Rgb::new(240, 120, 30));
        assert_eq!(decode_pixel(&[240, 120, 30, 255], PixelLayout::Rgba8), Rgb::new(240, 120, 30));
        assert_eq!(decode_pixel(&[255, 240, 120, 30], PixelLayout::Argb8), Rgb::new(240, 120, 30));
        assert_eq!(decode_pixel(&[255, 30, 120, 240], PixelLayout::Abgr8), Rgb::new(240, 120, 30));

        // DXGI R10G10B10A2: R=1023 (~255), G=512 (~128), B=0
        let packed_dxgi: u32 = 1023 | (512 << 10);
        let c10 = decode_pixel(&packed_dxgi.to_le_bytes(), PixelLayout::Rgb10A2);
        assert_eq!(c10.r, 255);
        assert!((127..=129).contains(&c10.g));
        assert_eq!(c10.b, 0);

        // CoreGraphics RGB101010: R in [29:20], G in [19:10], B in [9:0]
        let packed_cg: u32 = (1023 << 20) | (512 << 10) | 256;
        let cg_le = decode_pixel(&packed_cg.to_le_bytes(), PixelLayout::Bgr10A2Le);
        let cg_be = decode_pixel(&packed_cg.to_be_bytes(), PixelLayout::Bgr10A2Be);
        assert_eq!(cg_le, Rgb::new(255, 128, 64));
        assert_eq!(cg_be, Rgb::new(255, 128, 64));

        // IEEE binary16: 0x4000 = 2.0 (SDR white in scRGB), 0x0000 = 0.0
        let mut f16_bytes = [0u8; 8];
        f16_bytes[0..2].copy_from_slice(&0x4000u16.to_le_bytes()); // R = 2.0
        let hdr_red = decode_pixel(&f16_bytes, PixelLayout::Rgba16F);
        assert!(hdr_red.r > 200 && hdr_red.g == 0 && hdr_red.b == 0, "hdr_red={hdr_red:?}");
    }

    #[test]
    fn maps_coregraphics_bitmap_info_variants() {
        // kCGBitmapByteOrder32Little (2<<12) | kCGImageAlphaPremultipliedFirst (3)
        assert_eq!(map_cg_bitmap(32, 8, (2 << 12) | 3), Some(PixelLayout::Bgra8));
        // kCGBitmapByteOrder32Little (2<<12) | kCGImageAlphaNoneSkipFirst (6)
        assert_eq!(map_cg_bitmap(32, 8, (2 << 12) | 6), Some(PixelLayout::Bgra8));
        // kCGBitmapByteOrder32Big (4<<12) | kCGImageAlphaPremultipliedLast (1)
        assert_eq!(map_cg_bitmap(32, 8, (4 << 12) | 1), Some(PixelLayout::Rgba8));
        // kCGBitmapByteOrder32Big (4<<12) | kCGImageAlphaNoneSkipFirst (6)
        assert_eq!(map_cg_bitmap(32, 8, (4 << 12) | 6), Some(PixelLayout::Argb8));
        // 10-bit P3 wide gamut
        assert_eq!(map_cg_bitmap(32, 10, 2 << 12), Some(PixelLayout::Bgr10A2Le));
        // 16-bit float EDR
        assert_eq!(map_cg_bitmap(64, 16, 1 << 8), Some(PixelLayout::Rgba16F));
    }

    #[test]
    fn strided_downsampler_produces_64x36_grid() {
        let (w, h) = (128usize, 72usize);
        let pitch = w * 4;
        let mut buf = vec![0u8; h * pitch];
        for y in 0..h {
            for x in 0..w {
                let off = y * pitch + x * 4;
                if x < w / 2 {
                    // Left half BGRA = Red (B=0, G=0, R=255)
                    buf[off + 2] = 255;
                } else {
                    // Right half BGRA = Blue (B=255, G=0, R=0)
                    buf[off] = 255;
                }
            }
        }
        let frame = downsample_surface_to_frame(
            &buf,
            pitch,
            w,
            h,
            PixelLayout::Bgra8,
            SurfaceRotation::Identity,
        )
        .unwrap();
        assert_eq!(frame.width, SYNC_GRID_W);
        assert_eq!(frame.height, SYNC_GRID_H);
        assert_eq!(frame.pixels[0], Rgb::new(255, 0, 0));
        assert_eq!(frame.pixels[SYNC_GRID_W - 1], Rgb::new(0, 0, 255));
    }
}
