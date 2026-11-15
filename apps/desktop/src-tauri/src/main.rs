// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;

use capture::CaptureManager;
use groovie_core::{
    Action, Config, ConfigInfo, Device, DisplayInfo, Group, Groovie, LogEntry, Scene, SyncConfig,
    SyncStatus, Target,
};
use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::RwLock;

/// Fast local sync while the window is open; quiet keepalive while hidden in the tray.
const ACTIVE_POLL: Duration = Duration::from_millis(2500);
const IDLE_POLL: Duration = Duration::from_secs(45);
const GITHUB_URL: &str = "https://github.com/rudyorre/groovie";

/// Holds the running engine, or the startup error if UDP port 4002 was busy so
/// the window can still open and tell the user what's holding it.
struct AppState(RwLock<Result<Groovie, String>>);

struct TraySyncItem(Mutex<Option<CheckMenuItem<tauri::Wry>>>);

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

async fn start_engine(app: &AppHandle) -> R<Groovie> {
    let groovie = Groovie::start(Config::default().with_env_overrides()).await.map_err(err)?;
    groovie.set_poll_interval(ACTIVE_POLL);
    let mut rx = groovie.subscribe();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if let groovie_core::Event::SyncChanged { ref status } = ev {
                        if !status.active {
                            handle.state::<CaptureManager>().stop();
                        }
                        if let Some(item) = handle.state::<TraySyncItem>().0.lock().unwrap().clone() {
                            let _ = item.set_checked(status.active);
                        }
                    }
                    let _ = handle.emit("groovie://event", ev);
                }
                Err(RecvError::Lagged(_)) => continue,
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
        let g = start_engine(&app).await?;
        *s.0.write().await = Ok(g);
        if app.tray_by_id("main").is_none() {
            let _ = build_tray(&app);
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
async fn save_scene(s: State<'_, AppState>, name: String) -> R<Scene> {
    engine(&s).await?.save_scene(&name, None).await.map_err(err)
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
async fn set_window_active(s: State<'_, AppState>, active: bool, sync_now: bool) -> R<()> {
    if let Ok(g) = s.0.read().await.clone() {
        let changed = g.set_poll_interval(if active { ACTIVE_POLL } else { IDLE_POLL });
        if active && (sync_now || changed) {
            g.sync_now().await;
        }
    }
    Ok(())
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
        let g = start_engine(&app).await?;
        let info = g.config_info().await;
        *s.0.write().await = Ok(g);
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
        let g = start_engine(&app).await?;
        let info = g.config_info().await;
        *s.0.write().await = Ok(g);
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
    *app.state::<TraySyncItem>().0.lock().unwrap() = None;
    if let Some(tray) = app.remove_tray_by_id("main") {
        let _ = tray.set_visible(false);
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    // Tray icon: left-click opens Groovie; right-click opens the menu.
    let on = MenuItem::with_id(app, "on", "All on", true, None::<&str>)?;
    let off = MenuItem::with_id(app, "off", "All off", true, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Show Groovie", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = if CaptureManager::is_supported() {
        let sync_item = CheckMenuItem::with_id(app, "sync", "Screen sync", true, false, None::<&str>)?;
        let sep = PredefinedMenuItem::separator(app)?;
        *app.state::<TraySyncItem>().0.lock().unwrap() = Some(sync_item.clone());
        Menu::with_items(app, &[&sync_item, &sep, &on, &off, &show, &quit])?
    } else {
        Menu::with_items(app, &[&on, &off, &show, &quit])?
    };

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
        .on_menu_event(|app, ev| match ev.id.as_ref() {
            "sync" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    match set_or_toggle_sync(&app, None).await {
                        Ok(st) => {
                            if let Some(item) = app.state::<TraySyncItem>().0.lock().unwrap().clone() {
                                let _ = item.set_checked(st.active);
                            }
                        }
                        Err(_) => {
                            if let Some(item) = app.state::<TraySyncItem>().0.lock().unwrap().clone() {
                                let _ = item.set_checked(false);
                            }
                            show_main(&app);
                        }
                    }
                });
            }
            "on" => all_power(app, true),
            "off" => all_power(app, false),
            "show" => show_main(app),
            "quit" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    shutdown_engine_cleanly(&app, "quitting").await;
                    remove_tray_icon(&app);
                    app.exit(0);
                });
            }
            _ => {}
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
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin({
            let builder = tauri_plugin_updater::Builder::new();
            builder.build()
        })
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(CaptureManager::new());
            app.manage(TraySyncItem(Mutex::new(None)));
            let initial = tauri::async_runtime::block_on(start_engine(&handle));
            app.manage(AppState(RwLock::new(initial)));
            build_tray(&handle)?;
            Ok(())
        })
        // Closing the window keeps Groovie in the tray; use the tray's Quit to exit.
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
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
            add_ip,
            list_groups,
            create_group,
            update_group,
            delete_group,
            list_scenes,
            save_scene,
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
