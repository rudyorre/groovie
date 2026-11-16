//! `grooviectl`: control Govee lights from the terminal.
//!
//! Coordinates seamlessly with the Groovie desktop tray app over local loopback
//! IPC when it is running, uses a sub-10ms ephemeral UDP fast-path for cached
//! devices when the desktop app is closed, and falls back to full LAN discovery
//! when scanning or querying live state.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use groovie_core::ipc::{ForgottenDevice, IpcOutcome, IpcRequest, SyncStatusInfo};
use groovie_core::store::Store;
use groovie_core::{
    new_store_id, send_ephemeral_action, send_ephemeral_scene, Action, Config, Device, Group,
    Groovie, Rgb, Scene, Target,
};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "grooviectl", version, about = "Control Govee lights over your local network")]
struct Cli {
    /// Maximum time to wait for lights to answer when scanning or reading state (ms).
    #[arg(long, global = true, default_value_t = 1500)]
    wait: u64,
    /// Force a fresh LAN discovery scan before acting.
    #[arg(long, global = true)]
    scan: bool,
    /// Print JSON instead of human-readable output.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List discovered lights.
    List,
    /// Turn on. TARGET is a light name/IP/id, a group name, or "all".
    On { target: String },
    /// Turn off.
    Off { target: String },
    /// Toggle (if any light in the target is on, turn all off).
    Toggle { target: String },
    /// Set brightness, 1-100.
    Brightness {
        target: String,
        #[arg(value_parser = clap::value_parser!(u8).range(1..=100))]
        value: u8,
    },
    /// Set colour, e.g. "#ff8800" or "ff8800".
    Color { target: String, hex: String },
    /// Set white colour temperature in Kelvin (2000-9000).
    Kelvin {
        target: String,
        #[arg(value_parser = clap::value_parser!(u16).range(2000..=9000))]
        value: u16,
    },
    /// Give a light a nickname (empty string resets it).
    Rename { target: String, name: String },
    /// Forget a light across saved devices, nicknames, groups, scenes, and sync placements.
    Forget { target: String },
    /// Add a light by IP when discovery can't find it.
    AddIp { ip: Ipv4Addr },
    /// Manage groups.
    #[command(subcommand)]
    Group(GroupCmd),
    /// Manage saved scenes.
    #[command(subcommand)]
    Scene(SceneCmd),
    /// Check or control Screen Sync in the running desktop app.
    Sync {
        #[command(subcommand)]
        cmd: Option<SyncCmd>,
    },
    /// Show the active `groovie.json` path (or its JSON contents with `--json`).
    Config,
}

#[derive(Subcommand, Clone, Copy)]
enum SyncCmd {
    /// Show Screen Sync state and configured lights.
    Status,
    /// Start Screen Sync.
    Start,
    /// Stop Screen Sync.
    Stop,
    /// Toggle Screen Sync on or off.
    Toggle,
}

#[derive(Subcommand)]
enum GroupCmd {
    List,
    /// Create a group from lights (names, IPs or ids).
    Create { name: String, lights: Vec<String> },
    /// Add lights to a group.
    Add { group: String, lights: Vec<String> },
    /// Remove lights from a group.
    Remove { group: String, lights: Vec<String> },
    /// Delete a group.
    Delete { group: String },
}

#[derive(Subcommand)]
enum SceneCmd {
    List,
    /// Save the current state of every light (or just the listed ones).
    Save { name: String, lights: Vec<String> },
    /// Rename a saved scene.
    Rename { scene: String, name: String },
    /// Overwrite a saved scene with the current look (defaults to its existing lights, or `--all` / listed lights).
    Update {
        scene: String,
        /// Capture every discovered light instead of only the scene's existing lights.
        #[arg(long)]
        all: bool,
        lights: Vec<String>,
    },
    Apply { scene: String },
    Delete { scene: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("GROOVIE_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let cfg = Config::default().with_env_overrides();

    if matches!(cli.cmd, Cmd::Config) {
        let store = open_store(&cfg)?;
        if cli.json {
            println!("{}", store.pretty_json());
        } else if let Some(p) = store.path() {
            println!("{}", p.display());
        }
        return Ok(());
    }

    // Validate hex colours upfront before touching IPC or UDP sockets.
    let parsed_color = match &cli.cmd {
        Cmd::Color { hex, .. } => Some(
            Rgb::from_hex(hex).with_context(|| format!("'{hex}' is not a colour like #ff8800"))?,
        ),
        _ => None,
    };

    // Tier 1: Coordinate with the running Groovie desktop daemon over loopback IPC.
    if let Some(ipc_path) = effective_ipc_path() {
        if try_run_via_ipc(&ipc_path, &cli, parsed_color).await? {
            return Ok(());
        }
    }

    // Screen Sync capture lives in the desktop app process.
    if let Cmd::Sync { cmd } = &cli.cmd {
        return handle_offline_sync(&cfg, cmd.unwrap_or(SyncCmd::Status), cli.json);
    }

    // Tier 2: Fast offline store / ephemeral UDP path (<10ms, never binds port 4002).
    if !cli.scan {
        if try_run_ephemeral(&cfg, &cli, parsed_color).await? {
            return Ok(());
        }
    }

    // Tier 3: Standalone engine for live discovery, state-dependent toggles, or scene capture.
    run_standalone(cfg, cli, parsed_color).await
}

fn open_store(cfg: &Config) -> Result<Store> {
    match &cfg.store_path {
        Some(p) => Ok(Store::open(p)?),
        None => Ok(Store::in_memory()),
    }
}

/// Returns the IPC descriptor path to consult. When simulator/dev overrides
/// (`GROOVIE_CONFIG`, `GROOVIE_LISTEN`, `GROOVIE_SCAN_TARGETS`, ...) are set
/// without an explicit `GROOVIE_IPC_FILE`, skip the user's global desktop
/// daemon so simulator runs stay completely isolated.
fn effective_ipc_path() -> Option<PathBuf> {
    let non_empty = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    if let Some(explicit) = non_empty("GROOVIE_IPC_FILE") {
        return Some(PathBuf::from(explicit));
    }
    let has_sim_override = [
        "GROOVIE_CONFIG",
        "GROOVIE_LISTEN",
        "GROOVIE_SCAN_TARGETS",
        "GROOVIE_COMMAND_PORT",
        "GROOVIE_SCAN_PORT",
    ]
    .iter()
    .any(|k| non_empty(k).is_some());
    if has_sim_override {
        return None;
    }
    groovie_core::store::default_ipc_path()
}

async fn call_daemon<T: serde::de::DeserializeOwned>(
    ipc_path: &std::path::Path,
    req: IpcRequest,
) -> Result<Option<T>> {
    match groovie_core::ipc::try_call_at(ipc_path, req).await {
        IpcOutcome::Ok(val) => Ok(Some(val)),
        IpcOutcome::Unavailable => Ok(None),
        IpcOutcome::DaemonErr(msg) => bail!("{msg}"),
    }
}

async fn try_run_via_ipc(
    ipc_path: &std::path::Path,
    cli: &Cli,
    parsed_color: Option<Rgb>,
) -> Result<bool> {
    match &cli.cmd {
        Cmd::Config => unreachable!(),
        Cmd::List => {
            let Some(devices) = call_daemon::<Vec<Device>>(
                ipc_path,
                IpcRequest::ListDevices { scan: cli.scan, wait_ms: cli.wait },
            )
            .await?
            else {
                return Ok(false);
            };
            output_devices(&devices, cli.json)?;
            Ok(true)
        }
        Cmd::On { target } => Ok(call_daemon::<serde_json::Value>(
            ipc_path,
            IpcRequest::Apply { target: target.clone(), action: Action::Power(true) },
        )
        .await?
        .is_some()),
        Cmd::Off { target } => Ok(call_daemon::<serde_json::Value>(
            ipc_path,
            IpcRequest::Apply { target: target.clone(), action: Action::Power(false) },
        )
        .await?
        .is_some()),
        Cmd::Toggle { target } => Ok(call_daemon::<serde_json::Value>(
            ipc_path,
            IpcRequest::Apply { target: target.clone(), action: Action::Toggle },
        )
        .await?
        .is_some()),
        Cmd::Brightness { target, value } => Ok(call_daemon::<serde_json::Value>(
            ipc_path,
            IpcRequest::Apply { target: target.clone(), action: Action::Brightness(*value) },
        )
        .await?
        .is_some()),
        Cmd::Color { target, .. } => Ok(call_daemon::<serde_json::Value>(
            ipc_path,
            IpcRequest::Apply {
                target: target.clone(),
                action: Action::Color(parsed_color.expect("validated colour")),
            },
        )
        .await?
        .is_some()),
        Cmd::Kelvin { target, value } => Ok(call_daemon::<serde_json::Value>(
            ipc_path,
            IpcRequest::Apply { target: target.clone(), action: Action::Kelvin(*value) },
        )
        .await?
        .is_some()),
        Cmd::Rename { target, name } => Ok(call_daemon::<serde_json::Value>(
            ipc_path,
            IpcRequest::Rename { target: target.clone(), name: name.clone() },
        )
        .await?
        .is_some()),
        Cmd::Forget { target } => {
            let Some(forgotten) = call_daemon::<ForgottenDevice>(
                ipc_path,
                IpcRequest::Forget { target: target.clone() },
            )
            .await?
            else {
                return Ok(false);
            };
            output_forgotten(&forgotten, cli.json)?;
            Ok(true)
        }
        Cmd::AddIp { ip } => {
            let Some(d) = call_daemon::<Device>(
                ipc_path,
                IpcRequest::AddIp { ip: *ip, wait_ms: cli.wait },
            )
            .await?
            else {
                return Ok(false);
            };
            println!("Added {} ({})", d.name, d.sku);
            Ok(true)
        }
        Cmd::Group(gc) => match gc {
            GroupCmd::List => {
                let Some(groups) = call_daemon::<Vec<Group>>(ipc_path, IpcRequest::GroupList).await? else {
                    return Ok(false);
                };
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&groups)?);
                } else {
                    let devices = call_daemon::<Vec<Device>>(
                        ipc_path,
                        IpcRequest::ListDevices { scan: false, wait_ms: 0 },
                    )
                    .await?
                    .unwrap_or_default();
                    print_groups(&groups, |id| {
                        devices
                            .iter()
                            .find(|d| d.id == id)
                            .map(|d| d.name.clone())
                            .unwrap_or_else(|| id.to_string())
                    });
                }
                Ok(true)
            }
            GroupCmd::Create { name, lights } => {
                let Some(_gr) = call_daemon::<Group>(
                    ipc_path,
                    IpcRequest::GroupCreate { name: name.clone(), lights: lights.clone() },
                )
                .await?
                else {
                    return Ok(false);
                };
                println!("Created group '{name}'");
                Ok(true)
            }
            GroupCmd::Add { group, lights } => Ok(call_daemon::<serde_json::Value>(
                ipc_path,
                IpcRequest::GroupAdd { group: group.clone(), lights: lights.clone() },
            )
            .await?
            .is_some()),
            GroupCmd::Remove { group, lights } => Ok(call_daemon::<serde_json::Value>(
                ipc_path,
                IpcRequest::GroupRemove { group: group.clone(), lights: lights.clone() },
            )
            .await?
            .is_some()),
            GroupCmd::Delete { group } => Ok(call_daemon::<serde_json::Value>(
                ipc_path,
                IpcRequest::GroupDelete { group: group.clone() },
            )
            .await?
            .is_some()),
        },
        Cmd::Scene(sc) => match sc {
            SceneCmd::List => {
                let Some(scenes) = call_daemon::<Vec<Scene>>(ipc_path, IpcRequest::SceneList).await? else {
                    return Ok(false);
                };
                output_scenes(&scenes, cli.json)?;
                Ok(true)
            }
            SceneCmd::Save { name, lights } => {
                let Some(saved) = call_daemon::<Scene>(
                    ipc_path,
                    IpcRequest::SceneSave { name: name.clone(), lights: lights.clone() },
                )
                .await?
                else {
                    return Ok(false);
                };
                println!("Saved scene '{}' with {} lights", saved.name, saved.entries.len());
                Ok(true)
            }
            SceneCmd::Rename { scene, name } => {
                let Some(updated) = call_daemon::<Scene>(
                    ipc_path,
                    IpcRequest::SceneRename { scene: scene.clone(), name: name.clone() },
                )
                .await?
                else {
                    return Ok(false);
                };
                println!("Renamed scene to '{}'", updated.name);
                Ok(true)
            }
            SceneCmd::Update { scene, all, lights } => {
                let Some(updated) = call_daemon::<Scene>(
                    ipc_path,
                    IpcRequest::SceneUpdate {
                        scene: scene.clone(),
                        all: *all,
                        lights: lights.clone(),
                    },
                )
                .await?
                else {
                    return Ok(false);
                };
                println!("Updated scene '{}' with {} lights", updated.name, updated.entries.len());
                Ok(true)
            }
            SceneCmd::Apply { scene } => Ok(call_daemon::<serde_json::Value>(
                ipc_path,
                IpcRequest::SceneApply { scene: scene.clone() },
            )
            .await?
            .is_some()),
            SceneCmd::Delete { scene } => Ok(call_daemon::<serde_json::Value>(
                ipc_path,
                IpcRequest::SceneDelete { scene: scene.clone() },
            )
            .await?
            .is_some()),
        },
        Cmd::Sync { cmd } => {
            let sub = cmd.unwrap_or(SyncCmd::Status);
            let req = match sub {
                SyncCmd::Status => IpcRequest::SyncStatus,
                SyncCmd::Start => IpcRequest::SyncSet { enabled: Some(true) },
                SyncCmd::Stop => IpcRequest::SyncSet { enabled: Some(false) },
                SyncCmd::Toggle => IpcRequest::SyncSet { enabled: None },
            };
            let Some(info) = call_daemon::<SyncStatusInfo>(ipc_path, req).await? else {
                return Ok(false);
            };
            output_sync_status(&info, cli.json)?;
            Ok(true)
        }
    }
}

fn handle_offline_sync(cfg: &Config, cmd: SyncCmd, json: bool) -> Result<()> {
    match cmd {
        SyncCmd::Status => {
            let store = open_store(cfg)?;
            let info = SyncStatusInfo {
                supported: cfg!(target_os = "windows"),
                active: false,
                active_devices: Vec::new(),
                config: store.data.sync,
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                println!("Screen Sync: off (Groovie desktop app is not running)");
            }
            Ok(())
        }
        SyncCmd::Start | SyncCmd::Stop | SyncCmd::Toggle => {
            bail!("Screen Sync requires the Groovie desktop app to be running")
        }
    }
}

async fn try_run_ephemeral(
    cfg: &Config,
    cli: &Cli,
    parsed_color: Option<Rgb>,
) -> Result<bool> {
    let mut store = open_store(cfg)?;
    match &cli.cmd {
        Cmd::On { target } => try_ephemeral_act(&store, target, Action::Power(true), cfg.command_port).await,
        Cmd::Off { target } => try_ephemeral_act(&store, target, Action::Power(false), cfg.command_port).await,
        Cmd::Brightness { target, value } => {
            try_ephemeral_act(&store, target, Action::Brightness(*value), cfg.command_port).await
        }
        Cmd::Color { target, .. } => {
            try_ephemeral_act(
                &store,
                target,
                Action::Color(parsed_color.expect("validated colour")),
                cfg.command_port,
            )
            .await
        }
        Cmd::Kelvin { target, value } => {
            try_ephemeral_act(&store, target, Action::Kelvin(*value), cfg.command_port).await
        }
        Cmd::Rename { target, name } => {
            let Some(Target::Device(id)) = store.data.resolve_target(target) else {
                return Ok(false);
            };
            let trimmed = name.trim();
            if trimmed.is_empty() {
                store.data.nicknames.remove(&id);
            } else {
                store.data.nicknames.insert(id, trimmed.to_string());
            }
            store.save()?;
            Ok(true)
        }
        Cmd::Forget { target } => {
            let Some(Target::Device(id)) = store.data.resolve_target(target) else {
                return Ok(false);
            };
            let name = store
                .data
                .known_devices
                .iter()
                .find(|d| d.id == id)
                .map(|d| store.data.display_name(&d.id, &d.sku))
                .unwrap_or_else(|| id.clone());
            store.data.forget_device(&id);
            store.save()?;
            output_forgotten(&ForgottenDevice { id, name }, cli.json)?;
            Ok(true)
        }
        Cmd::Group(gc) => match gc {
            GroupCmd::List => {
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&store.data.groups)?);
                } else {
                    print_groups(&store.data.groups, |id| {
                        store
                            .data
                            .known_devices
                            .iter()
                            .find(|d| d.id == id)
                            .map(|d| store.data.display_name(&d.id, &d.sku))
                            .unwrap_or_else(|| id.to_string())
                    });
                }
                Ok(true)
            }
            GroupCmd::Create { name, lights } => {
                let Some(ids) = resolve_store_device_ids(&store, lights) else {
                    return Ok(false);
                };
                let gr = Group {
                    id: new_store_id("g"),
                    name: name.trim().to_string(),
                    members: ids,
                };
                store.data.groups.push(gr);
                store.save()?;
                println!("Created group '{name}'");
                Ok(true)
            }
            GroupCmd::Add { group, lights } => {
                let Some(ids) = resolve_store_device_ids(&store, lights) else {
                    return Ok(false);
                };
                let gr = store
                    .data
                    .groups
                    .iter_mut()
                    .find(|x| x.id == *group || x.name.eq_ignore_ascii_case(group))
                    .with_context(|| format!("no group called '{group}'"))?;
                for id in ids {
                    if !gr.members.contains(&id) {
                        gr.members.push(id);
                    }
                }
                store.save()?;
                Ok(true)
            }
            GroupCmd::Remove { group, lights } => {
                let Some(ids) = resolve_store_device_ids(&store, lights) else {
                    return Ok(false);
                };
                let gr = store
                    .data
                    .groups
                    .iter_mut()
                    .find(|x| x.id == *group || x.name.eq_ignore_ascii_case(group))
                    .with_context(|| format!("no group called '{group}'"))?;
                gr.members.retain(|m| !ids.contains(m));
                store.save()?;
                Ok(true)
            }
            GroupCmd::Delete { group } => {
                let before = store.data.groups.len();
                store
                    .data
                    .groups
                    .retain(|x| !(x.id == *group || x.name.eq_ignore_ascii_case(group)));
                if store.data.groups.len() == before {
                    bail!("no group called '{group}'");
                }
                store.save()?;
                Ok(true)
            }
        },
        Cmd::Scene(sc) => match sc {
            SceneCmd::List => {
                output_scenes(&store.data.scenes, cli.json)?;
                Ok(true)
            }
            SceneCmd::Save { .. } | SceneCmd::Update { .. } => Ok(false),
            SceneCmd::Rename { scene, name } => {
                let trimmed = name.trim();
                if trimmed.is_empty() {
                    bail!("scene name cannot be empty");
                }
                let sc = store
                    .data
                    .scenes
                    .iter_mut()
                    .find(|s| s.id == *scene || s.name.eq_ignore_ascii_case(scene))
                    .with_context(|| format!("no scene called '{scene}'"))?;
                sc.name = trimmed.to_string();
                let updated = sc.name.clone();
                store.save()?;
                println!("Renamed scene to '{updated}'");
                Ok(true)
            }
            SceneCmd::Apply { scene } => Ok(send_ephemeral_scene(&store, scene, cfg.command_port).await?),
            SceneCmd::Delete { scene } => {
                let before = store.data.scenes.len();
                store
                    .data
                    .scenes
                    .retain(|x| !(x.id == *scene || x.name.eq_ignore_ascii_case(scene)));
                if store.data.scenes.len() == before {
                    bail!("no scene called '{scene}'");
                }
                store.save()?;
                Ok(true)
            }
        },
        Cmd::List | Cmd::Toggle { .. } | Cmd::AddIp { .. } | Cmd::Sync { .. } | Cmd::Config => Ok(false),
    }
}

async fn try_ephemeral_act(
    store: &Store,
    target: &str,
    action: Action,
    command_port: u16,
) -> Result<bool> {
    let Some(t) = store.data.resolve_target(target) else {
        return Ok(false);
    };
    Ok(send_ephemeral_action(store, &t, action, command_port).await?)
}

fn resolve_store_device_ids(store: &Store, queries: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::with_capacity(queries.len());
    for q in queries {
        match store.data.resolve_target(q)? {
            Target::Device(id) => out.push(id),
            _ => return None,
        }
    }
    Some(out)
}

async fn run_standalone(cfg: Config, cli: Cli, parsed_color: Option<Rgb>) -> Result<()> {
    let g = Groovie::start(cfg)
        .await
        .context("could not start (is UDP port 4002 free?)")?;
    let wait = Duration::from_millis(cli.wait);

    // `add-ip` probes a single known IP directly and exits as soon as it answers.
    if let Cmd::AddIp { ip } = cli.cmd {
        match g.add_device_by_ip_and_wait(ip, wait).await? {
            Some(d) => {
                println!("Added {} ({})", d.name, d.sku);
                return Ok(());
            }
            None => bail!("no answer from {ip}; check the IP and that LAN Control is on"),
        }
    }

    // When targets are already cached in `known_devices` and `--scan` wasn't forced,
    // wait only until those lights report `devStatus` instead of sleeping the full `--wait`.
    let smart_ready = if !cli.scan {
        known_ready_ids(&g, &cli.cmd).await
    } else {
        None
    };
    match smart_ready {
        Some(ids) if !ids.is_empty() => g.wait_for_devices_ready(&ids, wait).await,
        _ => tokio::time::sleep(wait).await,
    }

    match cli.cmd {
        Cmd::Config | Cmd::AddIp { .. } | Cmd::Sync { .. } => unreachable!(),
        Cmd::List => {
            let devices = g.devices().await;
            output_devices(&devices, cli.json)?;
            return Ok(());
        }
        Cmd::On { target } => act(&g, &target, Action::Power(true)).await?,
        Cmd::Off { target } => act(&g, &target, Action::Power(false)).await?,
        Cmd::Toggle { target } => act(&g, &target, Action::Toggle).await?,
        Cmd::Brightness { target, value } => act(&g, &target, Action::Brightness(value)).await?,
        Cmd::Color { target, .. } => {
            act(&g, &target, Action::Color(parsed_color.expect("validated colour"))).await?
        }
        Cmd::Kelvin { target, value } => act(&g, &target, Action::Kelvin(value)).await?,
        Cmd::Rename { target, name } => {
            let id = device_id(&g, &target).await?;
            g.rename_device(&id, &name).await?;
        }
        Cmd::Forget { target } => {
            let id = device_id(&g, &target).await?;
            let name = g
                .device(&id)
                .await
                .map(|d| d.name)
                .unwrap_or_else(|| id.clone());
            g.remove_device(&id).await?;
            output_forgotten(&ForgottenDevice { id, name }, cli.json)?;
        }
        Cmd::Group(c) => group(&g, c, cli.json).await?,
        Cmd::Scene(c) => scene(&g, c, cli.json).await?,
    }
    // Give UDP sends a moment to leave before the process exits.
    tokio::time::sleep(Duration::from_millis(25)).await;
    Ok(())
}

async fn known_ready_ids(g: &Groovie, cmd: &Cmd) -> Option<Vec<String>> {
    match cmd {
        Cmd::Toggle { target } => match g.resolve(target).await.ok()? {
            Target::All => {
                let ids: Vec<String> = g.devices().await.into_iter().map(|d| d.id).collect();
                (!ids.is_empty()).then_some(ids)
            }
            Target::Device(id) => Some(vec![id]),
            Target::Group(gid) => {
                let gr = g.groups().await.into_iter().find(|x| x.id == gid)?;
                (!gr.members.is_empty()).then_some(gr.members)
            }
        },
        Cmd::Scene(SceneCmd::Save { lights, .. }) if !lights.is_empty() => {
            let mut ids = Vec::with_capacity(lights.len());
            for q in lights {
                match g.resolve(q).await.ok()? {
                    Target::Device(id) => ids.push(id),
                    _ => return None,
                }
            }
            Some(ids)
        }
        Cmd::Scene(SceneCmd::Update { scene, all, lights }) if !*all => {
            if !lights.is_empty() {
                let mut ids = Vec::with_capacity(lights.len());
                for q in lights {
                    match g.resolve(q).await.ok()? {
                        Target::Device(id) => ids.push(id),
                        _ => return None,
                    }
                }
                Some(ids)
            } else {
                let existing = g
                    .scenes()
                    .await
                    .into_iter()
                    .find(|s| s.id == *scene || s.name.eq_ignore_ascii_case(scene))?;
                (!existing.entries.is_empty())
                    .then(|| existing.entries.into_iter().map(|e| e.device).collect())
            }
        }
        _ => None,
    }
}

async fn act(g: &Groovie, target: &str, action: Action) -> Result<()> {
    let t = g.resolve(target).await?;
    g.apply(&t, action).await?;
    Ok(())
}

async fn device_id(g: &Groovie, query: &str) -> Result<String> {
    match g.resolve(query).await? {
        Target::Device(id) => Ok(id),
        _ => bail!("'{query}' is not a single light"),
    }
}

async fn device_ids(g: &Groovie, queries: &[String]) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for q in queries {
        ids.push(device_id(g, q).await?);
    }
    Ok(ids)
}

async fn find_group(g: &Groovie, q: &str) -> Result<Group> {
    g.groups()
        .await
        .into_iter()
        .find(|x| x.id == q || x.name.eq_ignore_ascii_case(q))
        .with_context(|| format!("no group called '{q}'"))
}

async fn find_scene(g: &Groovie, q: &str) -> Result<Scene> {
    g.scenes()
        .await
        .into_iter()
        .find(|x| x.id == q || x.name.eq_ignore_ascii_case(q))
        .with_context(|| format!("no scene called '{q}'"))
}

async fn group(g: &Groovie, c: GroupCmd, json: bool) -> Result<()> {
    match c {
        GroupCmd::List => {
            let groups = g.groups().await;
            if json {
                println!("{}", serde_json::to_string_pretty(&groups)?);
                return Ok(());
            }
            let devices = g.devices().await;
            print_groups(&groups, |id| {
                devices
                    .iter()
                    .find(|d| d.id == id)
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| id.to_string())
            });
        }
        GroupCmd::Create { name, lights } => {
            let ids = device_ids(g, &lights).await?;
            g.create_group(&name, ids).await?;
            println!("Created group '{name}'");
        }
        GroupCmd::Add { group, lights } => {
            let mut gr = find_group(g, &group).await?;
            for id in device_ids(g, &lights).await? {
                if !gr.members.contains(&id) {
                    gr.members.push(id);
                }
            }
            g.update_group(gr).await?;
        }
        GroupCmd::Remove { group, lights } => {
            let mut gr = find_group(g, &group).await?;
            let ids = device_ids(g, &lights).await?;
            gr.members.retain(|m| !ids.contains(m));
            g.update_group(gr).await?;
        }
        GroupCmd::Delete { group } => {
            let gr = find_group(g, &group).await?;
            g.delete_group(&gr.id).await?;
        }
    }
    Ok(())
}

async fn scene(g: &Groovie, c: SceneCmd, json: bool) -> Result<()> {
    match c {
        SceneCmd::List => {
            let scenes = g.scenes().await;
            output_scenes(&scenes, json)?;
        }
        SceneCmd::Save { name, lights } => {
            let ids = if lights.is_empty() { None } else { Some(device_ids(g, &lights).await?) };
            let s = g.save_scene(&name, ids).await?;
            println!("Saved scene '{}' with {} lights", s.name, s.entries.len());
        }
        SceneCmd::Rename { scene, name } => {
            let existing = find_scene(g, &scene).await?;
            let updated = g.rename_scene(&existing.id, &name).await?;
            println!("Renamed scene to '{}'", updated.name);
        }
        SceneCmd::Update { scene, all, lights } => {
            let existing = find_scene(g, &scene).await?;
            let ids = if all {
                None
            } else if !lights.is_empty() {
                Some(device_ids(g, &lights).await?)
            } else if !existing.entries.is_empty() {
                Some(existing.entries.iter().map(|e| e.device.clone()).collect())
            } else {
                None
            };
            let updated = g.update_scene(&existing.id, None, ids).await?;
            println!("Updated scene '{}' with {} lights", updated.name, updated.entries.len());
        }
        SceneCmd::Apply { scene } => g.apply_scene(&scene).await?,
        SceneCmd::Delete { scene } => {
            let s = find_scene(g, &scene).await?;
            g.delete_scene(&s.id).await?;
        }
    }
    Ok(())
}

fn output_devices(devices: &[Device], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(devices)?);
    } else if devices.is_empty() {
        println!("No lights found. Is LAN Control enabled in the Govee app?");
    } else {
        print_devices(devices);
    }
    Ok(())
}

fn output_forgotten(forgotten: &ForgottenDevice, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(forgotten)?);
    } else {
        println!("Forgot '{}' ({})", forgotten.name, forgotten.id);
    }
    Ok(())
}

fn output_scenes(scenes: &[Scene], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(scenes)?);
    } else {
        for s in scenes {
            println!("{} ({} lights)", s.name, s.entries.len());
        }
    }
    Ok(())
}

fn output_sync_status(info: &SyncStatusInfo, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(info)?);
    } else if info.active {
        println!(
            "Screen Sync: active ({} {})",
            info.active_devices.len(),
            if info.active_devices.len() == 1 { "light" } else { "lights" }
        );
    } else if !info.supported {
        println!("Screen Sync: unsupported on this platform");
    } else {
        println!("Screen Sync: off");
    }
    Ok(())
}

fn print_groups(groups: &[Group], mut name_of: impl FnMut(&str) -> String) {
    for gr in groups {
        let members: Vec<_> = gr.members.iter().map(|id| name_of(id)).collect();
        println!("{}: {}", gr.name, members.join(", "));
    }
}

fn print_devices(devices: &[Device]) {
    println!("{:<20} {:<7} {:<15} {:<8} {:<5} {:<8}", "NAME", "SKU", "IP", "STATUS", "BRI", "COLOUR");
    for d in devices {
        let (status, bri, colour) = match (d.online, d.state) {
            (false, _) => ("offline".into(), "-".into(), "-".into()),
            (true, None) => ("online".into(), "?".into(), "?".into()),
            (true, Some(s)) => (
                if s.on { "on" } else { "off" }.to_string(),
                format!("{}%", s.brightness),
                if s.kelvin > 0 { format!("{}K", s.kelvin) } else { s.color.to_hex() },
            ),
        };
        println!("{:<20} {:<7} {:<15} {:<8} {:<5} {:<8}", d.name, d.sku, d.ip, status, bri, colour);
    }
}
