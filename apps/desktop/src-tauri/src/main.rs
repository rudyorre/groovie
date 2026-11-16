// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;

use capture::CaptureManager;
use groovie_core::{
    Action, Config, ConfigInfo, DesktopPrefs, Device, DisplayInfo, Group, Groovie, LogEntry, Scene,
    SyncConfig, SyncStatus, Target,
};
use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_window_state::{AppHandleExt as WindowStateAppExt, StateFlags};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::RwLock;

/// Fast local sync while the window is open; quiet keepalive while hidden in the tray.
const ACTIVE_POLL: Duration = Duration::from_millis(2500);
const IDLE_POLL: Duration = Duration::from_secs(45);
const GITHUB_URL: &str = "https://github.com/rudyorre/groovie";

fn window_state_flags() -> StateFlags {
    // Exclude VISIBLE so closing to tray or launching with `--minimized` never
    // overrides startup visibility preferences.
    StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED | StateFlags::FULLSCREEN
}

fn has_minimized_arg<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().any(|a| {
        let s = a.as_ref();
        s == "--minimized" || s == "--hidden"
    })
}

/// Holds the running engine, or the startup error if UDP port 4002 was busy so
/// the window can still open and tell the user what's holding it.
struct AppState(RwLock<Result<Groovie, String>>);

/// Owns the loopback IPC server so `grooviectl` can share the running tray engine.
struct IpcHandle(tokio::sync::Mutex<Option<groovie_core::ipc::IpcServer>>);

#[derive(Clone, Default, PartialEq, Eq)]
struct TraySnapshot {
    sync_active: bool,
    /// `(group_id, group_name, any_online_member_on)`
    groups: Vec<(String, String, bool)>,
    /// `(scene_id, scene_name)`
    scenes: Vec<(String, String)>,
}

struct TrayState(Mutex<Option<TraySnapshot>>);

struct DesktopSyncController(AppHandle);

impl groovie_core::ipc::SyncController for DesktopSyncController {
    fn is_supported(&self) -> bool {
        CaptureManager::is_supported()
    }

    fn set_or_toggle<'a>(
        &'a self,
        desired: Option<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SyncStatus, String>> + Send + 'a>> {
        Box::pin(async move {
            let payload = set_or_toggle_sync(&self.0, desired).await?;
            refresh_tray_menu(&self.0, false).await;
            Ok(SyncStatus {
                active: payload.active,
                active_devices: payload.active_devices,
                config: payload.config,
            })
        })
    }
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncStatePayload {
    supported: bool,
    active: bool,
    active_devices: Vec<String>,
    config: SyncConfig,
}

impl SyncStatePayload {
    fn from_status(st: SyncStatus) -> Self {
        Self {
            supported: CaptureManager::is_supported(),
            active: st.active,
            active_devices: st.active_devices,
            config: st.config,
        }
    }
}

type R<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

async fn start_engine(app: &AppHandle, start_active: bool) -> R<Groovie> {
    let groovie = Groovie::start(Config::default().with_env_overrides()).await.map_err(err)?;
    groovie.set_poll_interval(if start_active { ACTIVE_POLL } else { IDLE_POLL });
    let sync_ctl: std::sync::Arc<dyn groovie_core::ipc::SyncController> =
        std::sync::Arc::new(DesktopSyncController(app.clone()));
    if let Ok(ipc) = groovie_core::ipc::IpcServer::start_default(groovie.clone(), Some(sync_ctl)).await {
        *app.state::<IpcHandle>().0.lock().await = ipc;
    }
    let mut rx = groovie.subscribe();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let tray_relevant = matches!(
                        ev,
                        groovie_core::Event::DeviceUpdated { .. }
                            | groovie_core::Event::ConfigChanged
                            | groovie_core::Event::SyncChanged { .. }
                    );
                    if let groovie_core::Event::SyncChanged { ref status } = ev {
                        if !status.active {
                            handle.state::<CaptureManager>().stop();
                        }
                    }
                    let _ = handle.emit("groovie://event", ev);
                    if tray_relevant {
                        refresh_tray_menu(&handle, false).await;
                    }
                }
                Err(RecvError::Lagged(_)) => {
                    refresh_tray_menu(&handle, false).await;
                    continue;
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
    Ok(groovie)
}

async fn engine(s: &State<'_, AppState>) -> R<Groovie> {
    s.0.read().await.clone()
}

async fn shutdown_engine_cleanly(app: &AppHandle, reason: &str) {
    *app.state::<IpcHandle>().0.lock().await = None;
    app.state::<CaptureManager>().stop();
    let prev = {
        let state = app.state::<AppState>();
        let mut guard = state.0.write().await;
        std::mem::replace(&mut *guard, Err(reason.into()))
    };
    if let Ok(g) = prev {
        g.stop_sync(Some(reason)).await;
    }
}

async fn set_or_toggle_sync(app: &AppHandle, desired: Option<bool>) -> R<SyncStatePayload> {
    let g = app.state::<AppState>().0.read().await.clone()?;
    let cur = g.sync_status().await;
    let enable = desired.unwrap_or(!cur.active);

    if enable {
        if !CaptureManager::is_supported() {
            return Err("Screen Sync is currently supported on Windows only".into());
        }
        app.state::<CaptureManager>()
            .start(g.clone(), cur.config.display_id.clone())?;
        match g.start_sync().await {
            Ok(st) => Ok(SyncStatePayload::from_status(st)),
            Err(e) => {
                app.state::<CaptureManager>().stop();
                Err(err(e))
            }
        }
    } else {
        app.state::<CaptureManager>().stop();
        let st = g.stop_sync(Some("stopped")).await;
        Ok(SyncStatePayload::from_status(st))
    }
}

#[tauri::command]
async fn list_devices(s: State<'_, AppState>) -> R<Vec<Device>> {
    Ok(engine(&s).await?.devices().await)
}

#[tauri::command]
async fn discover(app: AppHandle, s: State<'_, AppState>) -> R<()> {
    // If startup failed because the port was busy (or an update was cancelled), Retry comes through here.
    if s.0.read().await.is_err() {
        let g = start_engine(&app, true).await?;
        *s.0.write().await = Ok(g);
        let snap = collect_tray_snapshot(&app).await;
        if app.tray_by_id("main").is_none() {
            *app.state::<TrayState>().0.lock().unwrap() = Some(snap.clone());
            let _ = build_tray(&app, &snap);
        } else {
            refresh_tray_menu(&app, true).await;
        }
        let _ = app.emit("groovie://event", groovie_core::Event::ConfigChanged);
        return Ok(());
    }
    let g = engine(&s).await?;
    g.discover().await.map_err(err)?;
    g.refresh_all().await;
    Ok(())
}

#[tauri::command]
async fn apply(s: State<'_, AppState>, target: Target, action: Action) -> R<()> {
    engine(&s).await?.apply(&target, action).await.map_err(err)
}

#[tauri::command]
async fn rename(s: State<'_, AppState>, id: String, name: String) -> R<()> {
    engine(&s).await?.rename_device(&id, &name).await.map_err(err)
}

#[tauri::command]
async fn remove_device(s: State<'_, AppState>, id: String) -> R<()> {
    engine(&s).await?.remove_device(&id).await.map_err(err)
}

#[tauri::command]
async fn add_ip(s: State<'_, AppState>, ip: String) -> R<()> {
    let ip: Ipv4Addr = ip.trim().parse().map_err(|_| format!("'{ip}' is not an IPv4 address"))?;
    engine(&s).await?.add_device_by_ip(ip).await.map_err(err)
}

#[tauri::command]
async fn list_groups(s: State<'_, AppState>) -> R<Vec<Group>> {
    Ok(engine(&s).await?.groups().await)
}

#[tauri::command]
async fn create_group(s: State<'_, AppState>, name: String, members: Vec<String>) -> R<Group> {
    engine(&s).await?.create_group(&name, members).await.map_err(err)
}

#[tauri::command]
async fn update_group(s: State<'_, AppState>, group: Group) -> R<()> {
    engine(&s).await?.update_group(group).await.map_err(err)
}

#[tauri::command]
async fn delete_group(s: State<'_, AppState>, id: String) -> R<()> {
    engine(&s).await?.delete_group(&id).await.map_err(err)
}

#[tauri::command]
async fn list_scenes(s: State<'_, AppState>) -> R<Vec<Scene>> {
    Ok(engine(&s).await?.scenes().await)
}

#[tauri::command]
async fn save_scene(s: State<'_, AppState>, name: String, devices: Option<Vec<String>>) -> R<Scene> {
    engine(&s).await?.save_scene(&name, devices).await.map_err(err)
}

#[tauri::command]
async fn rename_scene(s: State<'_, AppState>, id: String, name: String) -> R<Scene> {
    engine(&s).await?.rename_scene(&id, &name).await.map_err(err)
}

#[tauri::command]
async fn update_scene(
    s: State<'_, AppState>,
    id: String,
    name: Option<String>,
    devices: Option<Vec<String>>,
) -> R<Scene> {
    engine(&s)
        .await?
        .update_scene(&id, name.as_deref(), devices)
        .await
        .map_err(err)
}

#[tauri::command]
async fn restore_scene(s: State<'_, AppState>, scene: Scene) -> R<Scene> {
    engine(&s).await?.restore_scene(scene).await.map_err(err)
}

#[tauri::command]
async fn apply_scene(s: State<'_, AppState>, id: String) -> R<()> {
    engine(&s).await?.apply_scene(&id).await.map_err(err)
}

#[tauri::command]
async fn delete_scene(s: State<'_, AppState>, id: String) -> R<()> {
    engine(&s).await?.delete_scene(&id).await.map_err(err)
}

#[tauri::command]
async fn get_sync_state(s: State<'_, AppState>) -> R<SyncStatePayload> {
    match s.0.read().await.clone() {
        Ok(g) => Ok(SyncStatePayload::from_status(g.sync_status().await)),
        Err(_) => Ok(SyncStatePayload {
            supported: CaptureManager::is_supported(),
            active: false,
            active_devices: Vec::new(),
            config: SyncConfig::default(),
        }),
    }
}

#[tauri::command]
fn list_displays() -> Vec<DisplayInfo> {
    CaptureManager::list_displays()
}

#[tauri::command]
async fn set_sync_enabled(app: AppHandle, enabled: bool) -> R<SyncStatePayload> {
    set_or_toggle_sync(&app, Some(enabled)).await
}

#[tauri::command]
async fn update_sync_config(
    app: AppHandle,
    s: State<'_, AppState>,
    config: SyncConfig,
) -> R<SyncStatePayload> {
    let g = engine(&s).await?;
    let prev = g.sync_status().await;
    let st = g.update_sync_config(config).await.map_err(err)?;
    if st.active && st.config.display_id != prev.config.display_id {
        app.state::<CaptureManager>()
            .start(g, st.config.display_id.clone())?;
    }
    Ok(SyncStatePayload::from_status(st))
}

#[tauri::command]
async fn identify_sync_placement(s: State<'_, AppState>, placement_id: String) -> R<()> {
    engine(&s).await?.identify_sync_placement(&placement_id).await.map_err(err)
}

#[tauri::command]
async fn set_sync_preview(s: State<'_, AppState>, enabled: bool) -> R<()> {
    if let Ok(g) = s.0.read().await.clone() {
        g.set_sync_preview(enabled).await;
    }
    Ok(())
}

#[tauri::command]
async fn list_logs(s: State<'_, AppState>) -> R<Vec<LogEntry>> {
    match s.0.read().await.clone() {
        Ok(g) => Ok(g.logs().await),
        Err(_) => Ok(vec![]),
    }
}

#[tauri::command]
async fn clear_logs(s: State<'_, AppState>) -> R<()> {
    if let Ok(g) = s.0.read().await.clone() {
        g.clear_logs().await;
    }
    Ok(())
}

#[tauri::command]
async fn set_window_active(app: AppHandle, s: State<'_, AppState>, active: bool, sync_now: bool) -> R<()> {
    let win_visible = app
        .get_webview_window("main")
        .map(|w| w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false))
        .unwrap_or(false);
    let effective_active = active && win_visible;
    if let Ok(g) = s.0.read().await.clone() {
        let changed = g.set_poll_interval(if effective_active { ACTIVE_POLL } else { IDLE_POLL });
        if effective_active && (sync_now || changed) {
            g.sync_now().await;
        }
    }
    Ok(())
}

#[tauri::command]
fn get_desktop_prefs(app: AppHandle) -> R<DesktopPrefs> {
    let stored = groovie_core::store::read_desktop_prefs();
    let launch_at_login = app.autolaunch().is_enabled().unwrap_or(false);
    Ok(DesktopPrefs {
        launch_at_login,
        start_minimized: stored.start_minimized,
    })
}

#[tauri::command]
fn set_desktop_prefs(app: AppHandle, prefs: DesktopPrefs) -> R<DesktopPrefs> {
    let auto = app.autolaunch();
    let cur_auto = auto.is_enabled().unwrap_or(false);
    if prefs.launch_at_login != cur_auto {
        if prefs.launch_at_login {
            auto.enable().map_err(err)?;
        } else {
            auto.disable().map_err(err)?;
        }
    }
    groovie_core::store::write_desktop_prefs(&groovie_core::store::StoredDesktopPrefs {
        start_minimized: prefs.start_minimized,
    })
    .map_err(err)?;
    get_desktop_prefs(app)
}

#[tauri::command]
async fn get_config_info(s: State<'_, AppState>) -> R<ConfigInfo> {
    match s.0.read().await.clone() {
        Ok(g) => Ok(g.config_info().await),
        Err(_) => {
            let cfg = Config::default().with_env_overrides();
            let auto_default = groovie_core::store::default_auto_store_path();
            let is_custom = match (cfg.store_path.as_deref(), auto_default.as_deref()) {
                (Some(cur), Some(def)) => cur != def,
                (Some(_), None) => true,
                _ => false,
            };
            let raw_json = cfg
                .store_path
                .as_deref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .unwrap_or_else(|| "{\n  \"version\": 1\n}".into());
            let symlink_target = cfg
                .store_path
                .as_deref()
                .and_then(|p| {
                    let meta = std::fs::symlink_metadata(p).ok()?;
                    meta.file_type()
                        .is_symlink()
                        .then(|| groovie_core::store::resolve_write_target(p).ok())
                        .flatten()
                })
                .map(|p| p.to_string_lossy().into_owned());
            Ok(ConfigInfo {
                path: cfg.store_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                symlink_target,
                default_path: auto_default.map(|p| p.to_string_lossy().into_owned()),
                is_custom,
                json: raw_json,
            })
        }
    }
}

#[tauri::command]
async fn reload_config(app: AppHandle, s: State<'_, AppState>) -> R<ConfigInfo> {
    if s.0.read().await.is_err() {
        let g = start_engine(&app, true).await?;
        let info = g.config_info().await;
        *s.0.write().await = Ok(g);
        refresh_tray_menu(&app, true).await;
        let _ = app.emit("groovie://event", groovie_core::Event::ConfigChanged);
        return Ok(info);
    }
    engine(&s).await?.reload_config().await.map_err(err)
}

#[tauri::command]
async fn set_config_path(app: AppHandle, s: State<'_, AppState>, path: Option<String>) -> R<ConfigInfo> {
    if s.0.read().await.is_err() {
        let cleaned = path.as_deref().map(str::trim).filter(|p| !p.is_empty());
        let target = cleaned.map(groovie_core::store::normalize_user_store_path);
        groovie_core::store::write_custom_store_path(target.as_deref()).map_err(err)?;
        let g = start_engine(&app, true).await?;
        let info = g.config_info().await;
        *s.0.write().await = Ok(g);
        refresh_tray_menu(&app, true).await;
        let _ = app.emit("groovie://event", groovie_core::Event::ConfigChanged);
        return Ok(info);
    }
    engine(&s).await?.set_custom_config_path(path.as_deref()).await.map_err(err)
}

#[tauri::command]
async fn open_config_folder(s: State<'_, AppState>) -> R<()> {
    let path = match s.0.read().await.clone() {
        Ok(g) => g.ensure_config_saved().await.map_err(err)?,
        Err(_) => Config::default().with_env_overrides().store_path,
    }
    .ok_or_else(|| "no config file configured".to_string())?;
    let resolved = groovie_core::store::resolve_write_target(&path).unwrap_or(path);
    let dir = resolved
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .ok_or_else(|| "could not determine config directory".to_string())?;
    std::fs::create_dir_all(dir).map_err(err)?;
    open_directory(dir)
}

#[tauri::command]
fn open_github() -> R<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", GITHUB_URL])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(err)?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(GITHUB_URL).spawn().map_err(err)?;
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(GITHUB_URL).spawn().map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
async fn prepare_update_install(app: AppHandle, before_install: bool) -> R<()> {
    // On Windows, `Update::install()` spawns the installer and exits the process
    // immediately (`std::process::exit(0)`), so Screen Sync, UDP port 4002, and the
    // tray icon must be cleaned up *before* `install()` runs.
    // On macOS and Linux, `install()` returns to JS (and `.deb` prompts via `pkexec`
    // first), so teardown happens *after* `install()` right before `relaunch()`.
    if before_install == cfg!(windows) {
        let _ = app.save_window_state(window_state_flags());
        shutdown_engine_cleanly(&app, "restarting to finish update").await;
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(25)).await;
        remove_tray_icon(&app);
    }
    Ok(())
}

fn open_directory(dir: &std::path::Path) -> R<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe").arg(dir).spawn().map_err(err)?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(dir).spawn().map_err(err)?;
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(dir).spawn().map_err(err)?;
    }
    Ok(())
}

fn update_poll_mode(app: &AppHandle, active: bool, force_sync: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Ok(g) = app.state::<AppState>().0.read().await.clone() {
            let changed = g.set_poll_interval(if active { ACTIVE_POLL } else { IDLE_POLL });
            if !active {
                g.set_sync_preview(false).await;
            }
            if active && (force_sync || changed) {
                g.sync_now().await;
            }
        }
    });
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    update_poll_mode(app, true, true);
}

fn all_power(app: &AppHandle, on: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Ok(g) = app.state::<AppState>().0.read().await.clone() {
            let _ = g.apply(&Target::All, Action::Power(on)).await;
        }
    });
}

fn remove_tray_icon(app: &AppHandle) {
    *app.state::<TrayState>().0.lock().unwrap() = None;
    if let Some(tray) = app.remove_tray_by_id("main") {
        let _ = tray.set_visible(false);
    }
}

async fn collect_tray_snapshot(app: &AppHandle) -> TraySnapshot {
    let Ok(g) = app.state::<AppState>().0.read().await.clone() else {
        return TraySnapshot::default();
    };
    let (devices, groups, scenes, sync) =
        tokio::join!(g.devices(), g.groups(), g.scenes(), g.sync_status());
    let group_items = groups
        .into_iter()
        .map(|grp| {
            let any_on = devices.iter().any(|d| {
                d.online
                    && grp.members.iter().any(|m| m == &d.id)
                    && d.state.map(|s| s.on).unwrap_or(false)
            });
            (grp.id, grp.name, any_on)
        })
        .collect();
    let scene_items = scenes.into_iter().map(|sc| (sc.id, sc.name)).collect();
    TraySnapshot {
        sync_active: sync.active,
        groups: group_items,
        scenes: scene_items,
    }
}

fn build_tray_menu(app: &AppHandle, snap: &TraySnapshot) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;

    if CaptureManager::is_supported() {
        let sync_item = CheckMenuItem::with_id(
            app,
            "sync",
            "Screen sync",
            true,
            snap.sync_active,
            None::<&str>,
        )?;
        let sep = PredefinedMenuItem::separator(app)?;
        menu.append(&sync_item)?;
        menu.append(&sep)?;
    }

    let on = MenuItem::with_id(app, "on", "All on", true, None::<&str>)?;
    let off = MenuItem::with_id(app, "off", "All off", true, None::<&str>)?;
    menu.append(&on)?;
    menu.append(&off)?;

    if !snap.groups.is_empty() || !snap.scenes.is_empty() {
        let sep = PredefinedMenuItem::separator(app)?;
        menu.append(&sep)?;

        if !snap.groups.is_empty() {
            let groups_sub = Submenu::new(app, "Groups", true)?;
            for (id, name, any_on) in &snap.groups {
                let item = CheckMenuItem::with_id(
                    app,
                    format!("group:{id}"),
                    name,
                    true,
                    *any_on,
                    None::<&str>,
                )?;
                groups_sub.append(&item)?;
            }
            menu.append(&groups_sub)?;
        }

        if !snap.scenes.is_empty() {
            let scenes_sub = Submenu::new(app, "Scenes", true)?;
            for (id, name) in &snap.scenes {
                let item = MenuItem::with_id(app, format!("scene:{id}"), name, true, None::<&str>)?;
                scenes_sub.append(&item)?;
            }
            menu.append(&scenes_sub)?;
        }
    }

    let sep_bottom = PredefinedMenuItem::separator(app)?;
    let show = MenuItem::with_id(app, "show", "Show Groovie", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    menu.append(&sep_bottom)?;
    menu.append(&show)?;
    menu.append(&quit)?;

    Ok(menu)
}

async fn refresh_tray_menu(app: &AppHandle, force: bool) {
    let snap = collect_tray_snapshot(app).await;
    let should_update = {
        let tray_state = app.state::<TrayState>();
        let mut guard = tray_state.0.lock().unwrap();
        if !force && guard.as_ref() == Some(&snap) {
            false
        } else {
            *guard = Some(snap.clone());
            true
        }
    };
    if !should_update {
        return;
    }
    if let Some(tray) = app.tray_by_id("main") {
        if let Ok(menu) = build_tray_menu(app, &snap) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

fn build_tray(app: &AppHandle, initial_snap: &TraySnapshot) -> tauri::Result<()> {
    // Tray icon: left-click opens Groovie; right-click opens the menu.
    let menu = build_tray_menu(app, initial_snap)?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Groovie")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .on_menu_event(|app, ev| {
            let id = ev.id.as_ref();
            if let Some(group_id) = id.strip_prefix("group:") {
                let app = app.clone();
                let group_id = group_id.to_string();
                tauri::async_runtime::spawn(async move {
                    if let Ok(g) = app.state::<AppState>().0.read().await.clone() {
                        let _ = g.apply(&Target::Group(group_id), Action::Toggle).await;
                    }
                    refresh_tray_menu(&app, true).await;
                });
                return;
            }
            if let Some(scene_id) = id.strip_prefix("scene:") {
                let app = app.clone();
                let scene_id = scene_id.to_string();
                tauri::async_runtime::spawn(async move {
                    if let Ok(g) = app.state::<AppState>().0.read().await.clone() {
                        let _ = g.apply_scene(&scene_id).await;
                    }
                    refresh_tray_menu(&app, false).await;
                });
                return;
            }
            match id {
                "sync" => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if set_or_toggle_sync(&app, None).await.is_err() {
                            show_main(&app);
                        }
                        refresh_tray_menu(&app, true).await;
                    });
                }
                "on" => all_power(app, true),
                "off" => all_power(app, false),
                "show" => show_main(app),
                "quit" => {
                    let _ = app.save_window_state(window_state_flags());
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        shutdown_engine_cleanly(&app, "quitting").await;
                        remove_tray_icon(&app);
                        app.exit(0);
                    });
                }
                _ => {}
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        // Must come first: if Groovie is already running in the tray, focus its
        // window and exit before touching UDP port 4002.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if !has_minimized_arg(&args) {
                show_main(app);
            }
        }))
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(window_state_flags())
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin({
            let builder = tauri_plugin_updater::Builder::new();
            builder.build()
        })
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let start_hidden = has_minimized_arg(std::env::args())
                || groovie_core::store::read_desktop_prefs().start_minimized;
            app.manage(CaptureManager::new());
            app.manage(IpcHandle(tokio::sync::Mutex::new(None)));
            app.manage(TrayState(Mutex::new(None)));
            let initial = tauri::async_runtime::block_on(start_engine(&handle, !start_hidden));
            app.manage(AppState(RwLock::new(initial)));
            let snap = tauri::async_runtime::block_on(collect_tray_snapshot(&handle));
            *handle.state::<TrayState>().0.lock().unwrap() = Some(snap.clone());
            build_tray(&handle, &snap)?;
            if !start_hidden {
                show_main(&handle);
            }
            Ok(())
        })
        // Closing the window keeps Groovie in the tray; use the tray's Quit to exit.
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.app_handle().save_window_state(window_state_flags());
                let _ = window.hide();
                update_poll_mode(window.app_handle(), false, false);
            }
            WindowEvent::Focused(true) => {
                let visible = window.is_visible().unwrap_or(true) && !window.is_minimized().unwrap_or(false);
                if visible {
                    update_poll_mode(window.app_handle(), true, true);
                }
            }
            WindowEvent::Resized(_) => {
                let visible = window.is_visible().unwrap_or(true) && !window.is_minimized().unwrap_or(false);
                update_poll_mode(window.app_handle(), visible, false);
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            list_devices,
            discover,
            apply,
            rename,
            remove_device,
            add_ip,
            list_groups,
            create_group,
            update_group,
            delete_group,
            list_scenes,
            save_scene,
            rename_scene,
            update_scene,
            restore_scene,
            apply_scene,
            delete_scene,
            get_sync_state,
            list_displays,
            set_sync_enabled,
            update_sync_config,
            identify_sync_placement,
            set_sync_preview,
            list_logs,
            clear_logs,
            set_window_active,
            get_desktop_prefs,
            set_desktop_prefs,
            get_config_info,
            reload_config,
            set_config_path,
            open_config_folder,
            open_github,
            prepare_update_install,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Groovie");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimized_cli_flags() {
        assert!(!has_minimized_arg(["groovie"]));
        assert!(has_minimized_arg(["groovie", "--minimized"]));
        assert!(has_minimized_arg(["groovie", "--hidden"]));
        assert!(!has_minimized_arg(["groovie", "--other"]));
    }

    #[test]
    fn window_state_flags_exclude_visibility() {
        let flags = window_state_flags();
        assert!(flags.contains(StateFlags::SIZE));
        assert!(flags.contains(StateFlags::POSITION));
        assert!(flags.contains(StateFlags::MAXIMIZED));
        assert!(!flags.contains(StateFlags::VISIBLE));
    }
}
