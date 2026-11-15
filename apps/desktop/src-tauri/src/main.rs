// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use groovie_core::{Action, Config, Device, Group, Groovie, Scene, Target};
use std::net::Ipv4Addr;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tokio::sync::broadcast::error::RecvError;

struct AppState(Groovie);

type R<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
async fn list_devices(s: State<'_, AppState>) -> R<Vec<Device>> {
    Ok(s.0.devices().await)
}

#[tauri::command]
async fn discover(s: State<'_, AppState>) -> R<()> {
    s.0.discover().await.map_err(err)?;
    s.0.refresh_all().await;
    Ok(())
}

#[tauri::command]
async fn apply(s: State<'_, AppState>, target: Target, action: Action) -> R<()> {
    s.0.apply(&target, action).await.map_err(err)
}

#[tauri::command]
async fn rename(s: State<'_, AppState>, id: String, name: String) -> R<()> {
    s.0.rename_device(&id, &name).await.map_err(err)
}

#[tauri::command]
async fn add_ip(s: State<'_, AppState>, ip: String) -> R<()> {
    let ip: Ipv4Addr = ip.trim().parse().map_err(|_| format!("'{ip}' is not an IPv4 address"))?;
    s.0.add_device_by_ip(ip).await.map_err(err)
}

#[tauri::command]
async fn list_groups(s: State<'_, AppState>) -> R<Vec<Group>> {
    Ok(s.0.groups().await)
}

#[tauri::command]
async fn create_group(s: State<'_, AppState>, name: String, members: Vec<String>) -> R<Group> {
    s.0.create_group(&name, members).await.map_err(err)
}

#[tauri::command]
async fn update_group(s: State<'_, AppState>, group: Group) -> R<()> {
    s.0.update_group(group).await.map_err(err)
}

#[tauri::command]
async fn delete_group(s: State<'_, AppState>, id: String) -> R<()> {
    s.0.delete_group(&id).await.map_err(err)
}

#[tauri::command]
async fn list_scenes(s: State<'_, AppState>) -> R<Vec<Scene>> {
    Ok(s.0.scenes().await)
}

#[tauri::command]
async fn save_scene(s: State<'_, AppState>, name: String) -> R<Scene> {
    s.0.save_scene(&name, None).await.map_err(err)
}

#[tauri::command]
async fn apply_scene(s: State<'_, AppState>, id: String) -> R<()> {
    s.0.apply_scene(&id).await.map_err(err)
}

#[tauri::command]
async fn delete_scene(s: State<'_, AppState>, id: String) -> R<()> {
    s.0.delete_scene(&id).await.map_err(err)
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn all_power(app: &AppHandle, on: bool) {
    let g = app.state::<AppState>().0.clone();
    tauri::async_runtime::spawn(async move {
        let _ = g.apply(&Target::All, Action::Power(on)).await;
    });
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // Start the engine on Tauri's tokio runtime.
            let groovie = tauri::async_runtime::block_on(Groovie::start(Config::default().with_env_overrides()))?;

            // Forward engine events to the UI.
            let mut rx = groovie.subscribe();
            let handle = app.handle().clone();
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
            app.manage(AppState(groovie));

            // Tray icon for quick on/off.
            let on = MenuItem::with_id(app, "on", "All on", true, None::<&str>)?;
            let off = MenuItem::with_id(app, "off", "All off", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "Show Groovie", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&on, &off, &show, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main").tooltip("Groovie").menu(&menu).on_menu_event(
                |app, ev| match ev.id.as_ref() {
                    "on" => all_power(app, true),
                    "off" => all_power(app, false),
                    "show" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                },
            );
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
