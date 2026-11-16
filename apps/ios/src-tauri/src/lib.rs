use groovie_core::{
    Action, Config, ConfigInfo, Device, DisplayInfo, Group, Groovie, LogEntry, Scene, SyncConfig,
    SyncStatus, Target,
};
use std::net::Ipv4Addr;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::RwLock;

const ACTIVE_POLL: Duration = Duration::from_millis(2500);
const IDLE_POLL: Duration = Duration::from_secs(45);

struct AppState(RwLock<Result<Groovie, String>>);

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
            supported: false,
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
    let groovie = Groovie::start(Config::default().with_env_overrides())
        .await
        .map_err(err)?;
    groovie.set_poll_interval(ACTIVE_POLL);

    let mut rx = groovie.subscribe();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let _ = handle.emit("groovie://event", ev);
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });

    // On first launch on iOS, the initial outgoing UDP packet triggers the system
    // "Allow Local Network" prompt and is dropped until the user taps Allow.
    // Follow-up scans automatically pick up lights right after permission is granted.
    let retry_engine = groovie.clone();
    tauri::async_runtime::spawn(async move {
        for delay_ms in [2500_u64, 4000, 6000] {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            let any_online = retry_engine
                .devices()
                .await
                .iter()
                .any(|d| d.state.is_some());
            if any_online {
                break;
            }
            let _ = retry_engine.discover().await;
            retry_engine.refresh_all().await;
        }
    });

    Ok(groovie)
}

async fn engine(s: &State<'_, AppState>) -> R<Groovie> {
    s.0.read().await.clone()
}

#[tauri::command]
async fn list_devices(s: State<'_, AppState>) -> R<Vec<Device>> {
    Ok(engine(&s).await?.devices().await)
}

#[tauri::command]
async fn discover(app: AppHandle, s: State<'_, AppState>) -> R<()> {
    if s.0.read().await.is_err() {
        let g = start_engine(&app).await?;
        *s.0.write().await = Ok(g);
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
    engine(&s)
        .await?
        .rename_device(&id, &name)
        .await
        .map_err(err)
}

#[tauri::command]
async fn add_ip(s: State<'_, AppState>, ip: String) -> R<()> {
    let ip: Ipv4Addr = ip
        .trim()
        .parse()
        .map_err(|_| format!("'{ip}' is not an IPv4 address"))?;
    engine(&s).await?.add_device_by_ip(ip).await.map_err(err)
}

#[tauri::command]
async fn list_groups(s: State<'_, AppState>) -> R<Vec<Group>> {
    Ok(engine(&s).await?.groups().await)
}

#[tauri::command]
async fn create_group(s: State<'_, AppState>, name: String, members: Vec<String>) -> R<Group> {
    engine(&s)
        .await?
        .create_group(&name, members)
        .await
        .map_err(err)
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
            supported: false,
            active: false,
            active_devices: Vec::new(),
            config: SyncConfig::default(),
        }),
    }
}

#[tauri::command]
fn list_displays() -> Vec<DisplayInfo> {
    Vec::new()
}

#[tauri::command]
async fn set_sync_enabled(_enabled: bool) -> R<SyncStatePayload> {
    Err("Screen Sync is currently supported on Windows desktop only".into())
}

#[tauri::command]
async fn update_sync_config(s: State<'_, AppState>, config: SyncConfig) -> R<SyncStatePayload> {
    let g = engine(&s).await?;
    let st = g.update_sync_config(config).await.map_err(err)?;
    Ok(SyncStatePayload::from_status(st))
}

#[tauri::command]
async fn identify_sync_placement(s: State<'_, AppState>, placement_id: String) -> R<()> {
    engine(&s)
        .await?
        .identify_sync_placement(&placement_id)
        .await
        .map_err(err)
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
            g.resume().await;
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
            Ok(ConfigInfo {
                path: cfg
                    .store_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
                symlink_target: None,
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
async fn set_config_path(
    app: AppHandle,
    s: State<'_, AppState>,
    path: Option<String>,
) -> R<ConfigInfo> {
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
    engine(&s)
        .await?
        .set_custom_config_path(path.as_deref())
        .await
        .map_err(err)
}

#[tauri::command]
async fn save_config_json(app: AppHandle, s: State<'_, AppState>, json: String) -> R<ConfigInfo> {
    if s.0.read().await.is_err() {
        let cfg = Config::default().with_env_overrides();
        if let Some(p) = &cfg.store_path {
            let mut store = groovie_core::store::Store::open(p).map_err(err)?;
            store.replace_json(&json, &[]).map_err(err)?;
        }
        let g = start_engine(&app).await?;
        let info = g.config_info().await;
        *s.0.write().await = Ok(g);
        let _ = app.emit("groovie://event", groovie_core::Event::ConfigChanged);
        return Ok(info);
    }
    engine(&s).await?.save_config_json(&json).await.map_err(err)
}

#[tauri::command]
async fn open_config_folder(s: State<'_, AppState>) -> R<()> {
    if let Ok(g) = s.0.read().await.clone() {
        let _ = g.ensure_config_saved().await;
    }
    Ok(())
}

#[tauri::command]
fn open_github() -> R<()> {
    Ok(())
}

#[tauri::command]
async fn prepare_update_install() -> R<()> {
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let initial = tauri::async_runtime::block_on(start_engine(&handle));
            app.manage(AppState(RwLock::new(initial)));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Focused(true) = event {
                let app = window.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Ok(g) = app.state::<AppState>().0.read().await.clone() {
                        g.set_poll_interval(ACTIVE_POLL);
                        g.resume().await;
                    }
                });
            }
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
            save_config_json,
            open_config_folder,
            open_github,
            prepare_update_install,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Groovie");
}
