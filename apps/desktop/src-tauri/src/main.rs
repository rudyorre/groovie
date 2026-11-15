// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use groovie_core::{Action, Config, Device, Group, Groovie, Scene, Target};
use std::net::Ipv4Addr;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::RwLock;

/// Holds the running engine, or the startup error if UDP port 4002 was busy so
/// the window can still open and tell the user what's holding it.
struct AppState(RwLock<Result<Groovie, String>>);

type R<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

async fn start_engine(app: &AppHandle) -> R<Groovie> {
    let groovie = Groovie::start(Config::default().with_env_overrides()).await.map_err(err)?;
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
    // If startup failed because the port was busy, Retry comes through here.
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

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn all_power(app: &AppHandle, on: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Ok(g) = app.state::<AppState>().0.read().await.clone() {
            let _ = g.apply(&Target::All, Action::Power(on)).await;
        }
    });
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
            let initial = tauri::async_runtime::block_on(start_engine(&handle));
            app.manage(AppState(RwLock::new(initial)));

            // Tray icon: left-click opens Groovie; right-click opens the menu.
            let on = MenuItem::with_id(app, "on", "All on", true, None::<&str>)?;
            let off = MenuItem::with_id(app, "off", "All off", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "Show Groovie", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&on, &off, &show, &quit])?;
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
                    "on" => all_power(app, true),
                    "off" => all_power(app, false),
                    "show" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        // Closing the window keeps Groovie in the tray; use the tray's Quit to exit.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Groovie");
}
