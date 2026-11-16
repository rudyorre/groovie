//! The engine: owns the UDP socket, discovers devices, tracks their state,
//! sends commands and persists user config.

use crate::model::*;
use crate::protocol::*;
use crate::store::{
    default_auto_store_path, default_store_path, normalize_user_store_path, write_custom_store_path,
    KnownDevice, Store,
};
use crate::sync::{
    build_identify_razer_colors, device_uses_razer, reconcile_placements, DeviceSyncCommand,
    SyncConfig, SyncFrame, SyncPlacement, SyncProcessor, SyncStatus,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, watch, Mutex, Notify, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

const MAX_LOGS: usize = 500;
const SYNC_TICK: Duration = Duration::from_millis(33);
const RAZER_KEEPALIVE: Duration = Duration::from_millis(450);
const RAZER_REASSERT: Duration = Duration::from_secs(25);
const SOLID_MIN_INTERVAL: Duration = Duration::from_millis(100);
const PREVIEW_MIN_INTERVAL: Duration = Duration::from_millis(100);
const UNICAST_SCAN_BATCH: usize = 24;
const UNICAST_SCAN_PACE: Duration = Duration::from_millis(8);
const MAX_SUBNET_HOSTS: usize = 512;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Io(#[from] io::Error),
    #[error("unknown device '{0}'")]
    UnknownDevice(String),
    #[error("unknown group '{0}'")]
    UnknownGroup(String),
    #[error("unknown scene '{0}'")]
    UnknownScene(String),
    #[error("nothing matches '{0}'")]
    NoMatch(String),
    #[error("no lights are enabled for Screen Sync")]
    NoSyncDevices,
    #[error("no enabled lights are online yet")]
    NoOnlineSyncDevices,
    #[error("unknown placement '{0}'")]
    UnknownPlacement(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub struct Config {
    /// Where device replies arrive. Govee always replies to port 4002.
    pub listen: SocketAddr,
    /// Where `scan` requests go. Defaults to the Govee multicast group.
    pub scan_targets: Vec<SocketAddr>,
    /// Port used for unicast scans when adding a device by IP or sweeping a `/24`.
    pub scan_port: u16,
    /// Port devices accept commands on.
    pub command_port: u16,
    /// Also send unicast `scan` packets across private `/24` LAN interfaces so
    /// discovery works without paid multicast entitlements (enabled by default on iOS).
    pub subnet_scan: bool,
    /// JSON config file. `None` keeps everything in memory.
    pub store_path: Option<PathBuf>,
    /// How often to ask every device for its status.
    pub poll_interval: Duration,
    /// How often to re-run discovery.
    pub rescan_interval: Duration,
    /// After a command, wait this long and then confirm the real state.
    pub refresh_delay: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from((Ipv4Addr::UNSPECIFIED, LISTEN_PORT)),
            scan_targets: vec![SocketAddr::V4(SocketAddrV4::new(MULTICAST_ADDR, SCAN_PORT))],
            scan_port: SCAN_PORT,
            command_port: COMMAND_PORT,
            subnet_scan: cfg!(target_os = "ios"),
            store_path: default_store_path(),
            poll_interval: Duration::from_secs(15),
            rescan_interval: Duration::from_secs(60),
            refresh_delay: Duration::from_millis(1800),
        }
    }
}

struct Entry {
    device: Device,
    last_seen: Option<Instant>,
    last_command: Option<Instant>,
    missed_polls: u32,
    /// Bumped on every command so stale delayed refreshes can be skipped.
    generation: u64,
}

struct SyncDeviceSession {
    ip: Ipv4Addr,
    sku: String,
    snapshot: DeviceState,
    uses_razer: bool,
    last_razer: Option<Vec<Rgb>>,
    last_solid: Option<Rgb>,
    last_sent_at: Option<Instant>,
    last_razer_enable_at: Instant,
}

struct IdentifyPulse {
    placement_id: String,
    device_id: String,
    ip: Ipv4Addr,
    sku: String,
    uses_razer: bool,
    started_at: Instant,
    duration: Duration,
    /// Set when Screen Sync was not already active for this device so state is restored afterwards.
    restore_after: Option<DeviceState>,
}

#[derive(Default)]
struct SyncRuntime {
    active: bool,
    sessions: HashMap<String, SyncDeviceSession>,
    detached: HashSet<String>,
    frame: Option<SyncFrame>,
    processor: SyncProcessor,
    preview_enabled: bool,
    preview_dirty: bool,
    last_preview_at: Option<Instant>,
    identify: Option<IdentifyPulse>,
}

struct Inner {
    cfg: Config,
    sock: std::sync::RwLock<Arc<UdpSocket>>,
    sock_dead: AtomicBool,
    sock_rebound: Notify,
    rebind_lock: Mutex<()>,
    scan_lock: Mutex<()>,
    devices: RwLock<HashMap<String, Entry>>,
    store: Mutex<Store>,
    logs: Mutex<VecDeque<LogEntry>>,
    poll_tx: watch::Sender<Duration>,
    last_refresh: Mutex<Option<Instant>>,
    sync_rt: Mutex<SyncRuntime>,
    sync_notify: Notify,
    events: broadcast::Sender<Event>,
}

/// Aborts background tasks when the last [`Groovie`] handle is dropped.
struct TaskGuard(Vec<JoinHandle<()>>);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        for t in &self.0 {
            t.abort();
        }
    }
}

/// Cheap to clone handle to the running engine.
#[derive(Clone)]
pub struct Groovie {
    inner: Arc<Inner>,
    _tasks: Arc<TaskGuard>,
}

impl Groovie {
    /// Bind the socket, load config, start background tasks and kick off
    /// discovery.
    pub async fn start(cfg: Config) -> Result<Self> {
        let sock = Arc::new(bind_socket(cfg.listen)?);
        let store = match &cfg.store_path {
            Some(p) => Store::open(p)?,
            None => Store::in_memory(),
        };
        #[cfg(target_os = "ios")]
        let store = {
            // Seed ~/Documents/groovie.json immediately so the Groovie folder
            // appears right away in the iOS Files app (On My iPhone -> Groovie).
            let mut store = store;
            let _ = store.ensure_saved();
            store
        };

        // Show previously seen devices immediately (offline until they answer).
        let mut devices = HashMap::new();
        for k in &store.data.known_devices {
            let name = display_name(&store, &k.id, &k.sku);
            devices.insert(
                k.id.clone(),
                Entry {
                    device: Device {
                        id: k.id.clone(),
                        ip: k.ip,
                        sku: k.sku.clone(),
                        name,
                        online: false,
                        state: None,
                        firmware: None,
                        last_color: None,
                        last_kelvin: None,
                    },
                    last_seen: None,
                    last_command: None,
                    missed_polls: 0,
                    generation: 0,
                },
            );
        }

        let (events, _) = broadcast::channel(256);
        let (poll_tx, _) = watch::channel(cfg.poll_interval);
        let inner = Arc::new(Inner {
            cfg,
            sock: std::sync::RwLock::new(sock),
            sock_dead: AtomicBool::new(false),
            sock_rebound: Notify::new(),
            rebind_lock: Mutex::new(()),
            scan_lock: Mutex::new(()),
            devices: RwLock::new(devices),
            store: Mutex::new(store),
            logs: Mutex::new(VecDeque::with_capacity(MAX_LOGS)),
            poll_tx,
            last_refresh: Mutex::new(None),
            sync_rt: Mutex::new(SyncRuntime::default()),
            sync_notify: Notify::new(),
            events,
        });

        let tasks = vec![
            tokio::spawn(recv_loop(inner.clone())),
            tokio::spawn(poll_loop(inner.clone())),
            tokio::spawn(rescan_loop(inner.clone())),
            tokio::spawn(sync_loop(inner.clone())),
        ];

        let me = Self { inner, _tasks: Arc::new(TaskGuard(tasks)) };
        let _ = me.discover().await;
        me.refresh_all().await;
        Ok(me)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// Multicast (and optionally unicast-sweep) a scan request; replies are handled in the background.
    pub async fn discover(&self) -> Result<()> {
        self.inner.send_scan().await
    }

    /// Ask every known device for its status.
    pub async fn refresh_all(&self) {
        self.inner.refresh_all().await
    }

    /// Dynamically adjust how often background `devStatus` polls run (e.g. fast
    /// while the desktop window is visible, quiet while hidden in the tray).
    /// Returns `true` if the interval changed.
    pub fn set_poll_interval(&self, interval: Duration) -> bool {
        self.inner.poll_tx.send_if_modified(|cur| {
            if *cur != interval {
                *cur = interval;
                true
            } else {
                false
            }
        })
    }

    /// Immediately check for external `groovie.json` edits and poll idle devices
    /// unless a refresh already ran within the last 400 ms.
    pub async fn sync_now(&self) {
        self.inner.reload_store_if_changed().await;
        let recent = self
            .inner
            .last_refresh
            .lock()
            .await
            .map(|t| t.elapsed() < Duration::from_millis(400))
            .unwrap_or(false);
        if !recent {
            self.inner.refresh_idle().await;
        }
    }

    /// Recover cleanly after mobile background suspension or the iOS Local Network
    /// permission prompt: rebinds the UDP listener if invalidated by the OS, reloads
    /// external `groovie.json` edits, and re-runs discovery when no lights are online.
    pub async fn resume(&self) {
        if self.inner.sock_dead.load(Ordering::Acquire)
            || self.inner.socket().local_addr().is_err()
        {
            let _ = self.inner.rebind_socket("foreground resume").await;
        }
        self.sync_now().await;
        let any_online = self.inner.devices.read().await.values().any(|e| e.device.online);
        if !any_online {
            let _ = self.discover().await;
            self.refresh_all().await;
        }
    }

    /// Manually add a device that discovery can't see (e.g. other subnet,
    /// multicast blocked by the router).
    pub async fn add_device_by_ip(&self, ip: Ipv4Addr) -> Result<()> {
        let i = &self.inner;
        i.send(Command::Scan, SocketAddr::from((ip, i.cfg.scan_port))).await?;
        i.send(Command::DevStatus, SocketAddr::from((ip, i.cfg.command_port))).await?;
        Ok(())
    }

    pub async fn devices(&self) -> Vec<Device> {
        let mut v: Vec<Device> =
            self.inner.devices.read().await.values().map(|e| e.device.clone()).collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
        v
    }

    pub async fn device(&self, id: &str) -> Option<Device> {
        self.inner.devices.read().await.get(id).map(|e| e.device.clone())
    }

    /// Forget a device (it will come back if discovery sees it again).
    pub async fn remove_device(&self, id: &str) -> Result<()> {
        self.inner.devices.write().await.remove(id);
        let mut store = self.inner.store.lock().await;
        store.data.known_devices.retain(|d| d.id != id);
        store.save()?;
        drop(store);
        self.inner.emit(Event::ConfigChanged);
        Ok(())
    }

    /// Send an action to a device, a group, or everything.
    pub async fn apply(&self, target: &Target, action: Action) -> Result<()> {
        let ids = self.resolve_ids(target).await?;
        let action = match action {
            Action::Toggle => {
                let devices = self.inner.devices.read().await;
                let any_on = ids
                    .iter()
                    .filter_map(|id| devices.get(id))
                    .any(|e| e.device.state.map(|s| s.on).unwrap_or(false));
                Action::Power(!any_on)
            }
            a => a,
        };
        let summary = {
            let devices = self.inner.devices.read().await;
            let resolved: Vec<String> = ids
                .iter()
                .map(|id| match devices.get(id) {
                    Some(e) => format!("{} [{} @ {}]", e.device.name, e.device.id, e.device.ip),
                    None => id.clone(),
                })
                .collect();
            format!("apply {target:?} -> [{}]: {action:?}", resolved.join(", "))
        };
        self.inner.log("action", "", None, summary, false).await;

        // Turning off or manually recolouring a synced light detaches it from the
        // current Screen Sync session cleanly (sending RazerEnable(false) first).
        if matches!(action, Action::Power(false) | Action::Color(_) | Action::Kelvin(_)) {
            self.inner.detach_synced_devices(&ids).await;
        }

        for id in &ids {
            self.inner.send_action(id, action).await?;
        }
        Ok(())
    }

    async fn resolve_ids(&self, target: &Target) -> Result<Vec<String>> {
        match target {
            Target::All => Ok(self.inner.devices.read().await.keys().cloned().collect()),
            Target::Device(id) => {
                if self.inner.devices.read().await.contains_key(id) {
                    Ok(vec![id.clone()])
                } else {
                    Err(Error::UnknownDevice(id.clone()))
                }
            }
            Target::Group(gid) => {
                let store = self.inner.store.lock().await;
                let g = store
                    .data
                    .groups
                    .iter()
                    .find(|g| &g.id == gid)
                    .ok_or_else(|| Error::UnknownGroup(gid.clone()))?;
                let devices = self.inner.devices.read().await;
                Ok(g.members.iter().filter(|m| devices.contains_key(*m)).cloned().collect())
            }
        }
    }

    /// Resolve a human query ("all", device id/name/ip, group id/name).
    pub async fn resolve(&self, query: &str) -> Result<Target> {
        let q = query.trim();
        if q.eq_ignore_ascii_case("all") {
            return Ok(Target::All);
        }
        {
            let devices = self.inner.devices.read().await;
            for e in devices.values() {
                let d = &e.device;
                if d.id.eq_ignore_ascii_case(q) || d.name.eq_ignore_ascii_case(q) || d.ip.to_string() == q {
                    return Ok(Target::Device(d.id.clone()));
                }
            }
        }
        let store = self.inner.store.lock().await;
        if let Some(g) = store
            .data
            .groups
            .iter()
            .find(|g| g.id == q || g.name.eq_ignore_ascii_case(q))
        {
            return Ok(Target::Group(g.id.clone()));
        }
        Err(Error::NoMatch(q.to_string()))
    }

    // ---- nicknames -------------------------------------------------------

    pub async fn rename_device(&self, id: &str, name: &str) -> Result<()> {
        let name = name.trim();
        let mut store = self.inner.store.lock().await;
        let mut devices = self.inner.devices.write().await;
        let e = devices.get_mut(id).ok_or_else(|| Error::UnknownDevice(id.into()))?;
        if name.is_empty() {
            store.data.nicknames.remove(id);
        } else {
            store.data.nicknames.insert(id.to_string(), name.to_string());
        }
        e.device.name = display_name(&store, id, &e.device.sku);
        let dev = e.device.clone();
        drop(devices);
        store.save()?;
        drop(store);
        self.inner.emit(Event::DeviceUpdated { device: dev });
        Ok(())
    }

    // ---- groups ----------------------------------------------------------

    pub async fn groups(&self) -> Vec<Group> {
        self.inner.store.lock().await.data.groups.clone()
    }

    pub async fn create_group(&self, name: &str, members: Vec<String>) -> Result<Group> {
        let g = Group { id: new_id("g"), name: name.trim().to_string(), members };
        self.inner
            .mutate_store(|s| {
                s.data.groups.push(g.clone());
                Ok(())
            })
            .await?;
        Ok(g)
    }

    pub async fn update_group(&self, group: Group) -> Result<()> {
        self.inner
            .mutate_store(|s| {
                let g = s
                    .data
                    .groups
                    .iter_mut()
                    .find(|g| g.id == group.id)
                    .ok_or_else(|| Error::UnknownGroup(group.id.clone()))?;
                *g = group;
                Ok(())
            })
            .await
    }

    pub async fn delete_group(&self, id: &str) -> Result<()> {
        self.inner
            .mutate_store(|s| {
                s.data.groups.retain(|g| g.id != id);
                Ok(())
            })
            .await
    }

    // ---- scenes ----------------------------------------------------------

    pub async fn scenes(&self) -> Vec<Scene> {
        self.inner.store.lock().await.data.scenes.clone()
    }

    /// Snapshot the current state of `devices` (or every device) as a scene.
    pub async fn save_scene(&self, name: &str, devices: Option<Vec<String>>) -> Result<Scene> {
        let entries: Vec<SceneEntry> = {
            let all = self.inner.devices.read().await;
            let ids: Vec<String> = devices.unwrap_or_else(|| all.keys().cloned().collect());
            ids.iter()
                .filter_map(|id| all.get(id))
                .filter_map(|e| {
                    let s = e.device.state?;
                    Some(SceneEntry {
                        device: e.device.id.clone(),
                        on: s.on,
                        brightness: Some(s.brightness),
                        color: (s.kelvin == 0).then_some(s.color),
                        kelvin: (s.kelvin > 0).then_some(s.kelvin),
                    })
                })
                .collect()
        };
        let scene = Scene { id: new_id("s"), name: name.trim().to_string(), entries };
        self.inner
            .mutate_store(|s| {
                s.data.scenes.push(scene.clone());
                Ok(())
            })
            .await?;
        Ok(scene)
    }

    pub async fn delete_scene(&self, id: &str) -> Result<()> {
        self.inner
            .mutate_store(|s| {
                s.data.scenes.retain(|x| x.id != id);
                Ok(())
            })
            .await
    }

    pub async fn apply_scene(&self, id: &str) -> Result<()> {
        let scene = self
            .scenes()
            .await
            .into_iter()
            .find(|s| s.id == id || s.name.eq_ignore_ascii_case(id))
            .ok_or_else(|| Error::UnknownScene(id.into()))?;
        // Exit Screen Sync without restoring pre-sync snapshots so the scene's own look takes over.
        self.inner.abort_sync_without_restore("scene applied").await;
        self.inner
            .log("action", "", None, format!("apply_scene '{}' ({})", scene.name, scene.id), false)
            .await;
        let gap = Duration::from_millis(40);
        for e in &scene.entries {
            if !self.inner.devices.read().await.contains_key(&e.device) {
                continue;
            }
            self.inner.send_action(&e.device, Action::Power(e.on)).await?;
            if !e.on {
                continue;
            }
            if let Some(b) = e.brightness {
                tokio::time::sleep(gap).await;
                self.inner.send_action(&e.device, Action::Brightness(b)).await?;
            }
            if let Some(k) = e.kelvin {
                tokio::time::sleep(gap).await;
                self.inner.send_action(&e.device, Action::Kelvin(k)).await?;
            } else if let Some(c) = e.color {
                tokio::time::sleep(gap).await;
                self.inner.send_action(&e.device, Action::Color(c)).await?;
            }
        }
        Ok(())
    }

    // ---- screen sync -----------------------------------------------------

    pub async fn sync_status(&self) -> SyncStatus {
        self.inner.current_sync_status().await
    }

    pub async fn update_sync_config(&self, config: SyncConfig) -> Result<SyncStatus> {
        self.inner.update_sync_config(config).await
    }

    pub async fn start_sync(&self) -> Result<SyncStatus> {
        self.inner.start_sync().await
    }

    pub async fn stop_sync(&self, reason: Option<&str>) -> SyncStatus {
        self.inner.stop_sync(reason).await
    }

    pub async fn push_sync_frame(&self, frame: SyncFrame) {
        let should_wake = {
            let mut rt = self.inner.sync_rt.lock().await;
            rt.frame = Some(frame);
            rt.preview_dirty = true;
            rt.active || rt.preview_enabled
        };
        if should_wake {
            self.inner.sync_notify.notify_one();
        }
    }

    pub async fn set_sync_preview(&self, enabled: bool) {
        let should_wake = {
            let mut rt = self.inner.sync_rt.lock().await;
            rt.preview_enabled = enabled;
            rt.preview_dirty = enabled;
            rt.last_preview_at = None;
            enabled && rt.frame.is_some()
        };
        if should_wake {
            self.inner.sync_notify.notify_one();
        }
    }

    pub async fn identify_sync_placement(&self, placement_id: &str) -> Result<()> {
        self.inner.identify_sync_placement(placement_id).await
    }

    // ---- config file -----------------------------------------------------

    /// Return the active `groovie.json` path metadata and formatted JSON.
    pub async fn config_info(&self) -> ConfigInfo {
        self.inner.reload_store_if_changed().await;
        let store = self.inner.store.lock().await;
        build_config_info(&store)
    }

    /// Re-read `groovie.json` from disk and reconcile device nicknames/groups/scenes.
    pub async fn reload_config(&self) -> Result<ConfigInfo> {
        let mut store = self.inner.store.lock().await;
        store.reload()?;
        self.inner.apply_store_to_devices(&store).await;
        Ok(build_config_info(&store))
    }

    /// Switch the engine's config file to `path` (adopting existing JSON if
    /// `path` exists, or seeding `path` with the current config otherwise).
    pub async fn switch_store_path(&self, path: PathBuf) -> Result<ConfigInfo> {
        let mut store = self.inner.store.lock().await;
        store.set_path(path)?;
        self.inner.apply_store_to_devices(&store).await;
        Ok(build_config_info(&store))
    }

    /// Set or clear a persistent custom `groovie.json` location for dotfiles.
    pub async fn set_custom_config_path(&self, custom: Option<&str>) -> Result<ConfigInfo> {
        let cleaned = custom.map(str::trim).filter(|s| !s.is_empty());
        let target = match cleaned {
            Some(raw) => normalize_user_store_path(raw),
            None => default_auto_store_path()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "could not determine default config path"))?,
        };
        let info = self.switch_store_path(target.clone()).await?;
        write_custom_store_path(cleaned.map(|_| target.as_path()))?;
        Ok(info)
    }

    /// Ensure `groovie.json` exists on disk and return its path.
    pub async fn ensure_config_saved(&self) -> Result<Option<PathBuf>> {
        let mut store = self.inner.store.lock().await;
        store.ensure_saved()?;
        Ok(store.path().map(PathBuf::from))
    }

    /// Validate and atomically save raw `groovie.json` text (preserving any currently
    /// online discovered lights), reconcile nicknames/groups/scenes/sync, and probe any
    /// newly imported offline devices so they come online immediately.
    pub async fn save_config_json(&self, raw_json: &str) -> Result<ConfigInfo> {
        let live_known: Vec<KnownDevice> = self
            .inner
            .devices
            .read()
            .await
            .values()
            .filter(|e| e.device.online)
            .map(|e| KnownDevice {
                id: e.device.id.clone(),
                ip: e.device.ip,
                sku: e.device.sku.clone(),
            })
            .collect();

        let info = {
            let mut store = self.inner.store.lock().await;
            store.replace_json(raw_json, &live_known)?;
            self.inner.apply_store_to_devices(&store).await;
            build_config_info(&store)
        };

        let status = self.inner.current_sync_status().await;
        self.inner.emit(Event::SyncChanged { status });
        self.inner
            .log("action", "", None, "saved configuration JSON", false)
            .await;

        // Probe offline known devices right away so pasting a desktop groovie.json
        // onto iOS lights up reachable devices without waiting for the next poll.
        let offline_ips: Vec<Ipv4Addr> = self
            .inner
            .devices
            .read()
            .await
            .values()
            .filter(|e| !e.device.online)
            .map(|e| e.device.ip)
            .collect();
        for ip in offline_ips {
            let _ = self
                .inner
                .send(Command::Scan, SocketAddr::from((ip, self.inner.cfg.scan_port)))
                .await;
            let _ = self
                .inner
                .send(Command::DevStatus, SocketAddr::from((ip, self.inner.cfg.command_port)))
                .await;
        }

        Ok(info)
    }

    // ---- diagnostic logs -------------------------------------------------

    pub async fn logs(&self) -> Vec<LogEntry> {
        self.inner.logs.lock().await.iter().cloned().collect()
    }

    pub async fn clear_logs(&self) {
        self.inner.logs.lock().await.clear();
    }
}

fn build_config_info(store: &Store) -> ConfigInfo {
    let auto_default = default_auto_store_path();
    let is_custom = match (store.path(), auto_default.as_deref()) {
        (Some(cur), Some(def)) => cur != def,
        (Some(_), None) => true,
        _ => false,
    };
    ConfigInfo {
        path: store.path().map(|p| p.to_string_lossy().into_owned()),
        symlink_target: store.symlink_target().map(|p| p.to_string_lossy().into_owned()),
        default_path: auto_default.map(|p| p.to_string_lossy().into_owned()),
        is_custom,
        json: store.pretty_json(),
    }
}

impl Inner {
    fn emit(&self, ev: Event) {
        // No subscribers is fine.
        let _ = self.events.send(ev);
    }

    fn socket(&self) -> Arc<UdpSocket> {
        self.sock.read().unwrap().clone()
    }

    async fn rebind_socket(&self, reason: &str) -> Result<Arc<UdpSocket>> {
        let _guard = self.rebind_lock.lock().await;
        if !self.sock_dead.load(Ordering::Acquire) && self.socket().local_addr().is_ok() {
            return Ok(self.socket());
        }
        let next = Arc::new(bind_socket(self.cfg.listen)?);
        *self.sock.write().unwrap() = next.clone();
        self.sock_dead.store(false, Ordering::Release);
        self.sock_rebound.notify_waiters();
        self.log(
            "state",
            self.cfg.listen.to_string(),
            None,
            format!("rebound UDP listener ({reason})"),
            false,
        )
        .await;
        Ok(next)
    }

    async fn device_label_for_ip(&self, ip: IpAddr) -> Option<String> {
        let IpAddr::V4(v4) = ip else { return None };
        let devices = self.devices.read().await;
        devices
            .values()
            .filter(|e| e.device.ip == v4)
            .max_by_key(|e| (e.device.online, e.last_seen))
            .map(|e| format!("{} ({})", e.device.name, e.device.sku))
    }

    async fn log(
        &self,
        dir: &str,
        addr: impl Into<String>,
        device: Option<String>,
        message: impl Into<String>,
        poll: bool,
    ) {
        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let entry = LogEntry {
            ts_ms,
            dir: dir.to_string(),
            addr: addr.into(),
            device,
            message: message.into(),
            poll,
        };
        {
            let mut logs = self.logs.lock().await;
            if logs.len() >= MAX_LOGS {
                // Evict old background `devStatus` polls before commands/state changes.
                let poll_count = logs.iter().filter(|e| e.poll).count();
                if poll_count > MAX_LOGS / 2 {
                    if let Some(pos) = logs.iter().position(|e| e.poll) {
                        logs.remove(pos);
                    } else {
                        logs.pop_front();
                    }
                } else {
                    logs.pop_front();
                }
            }
            logs.push_back(entry.clone());
        }
        self.emit(Event::Logged { entry });
    }

    async fn mutate_store(&self, f: impl FnOnce(&mut Store) -> Result<()>) -> Result<()> {
        let mut store = self.store.lock().await;
        f(&mut store)?;
        store.save()?;
        drop(store);
        self.emit(Event::ConfigChanged);
        Ok(())
    }

    async fn reload_store_if_changed(&self) {
        let mut store = self.store.lock().await;
        match store.reload_if_changed() {
            Ok(true) => {
                self.apply_store_to_devices(&store).await;
            }
            Ok(false) => {}
            Err(e) => warn!(error = %e, "ignoring unreadable external config file"),
        }
    }

    async fn apply_store_to_devices(&self, store: &Store) {
        let known_by_id: HashMap<&str, &KnownDevice> = store
            .data
            .known_devices
            .iter()
            .map(|k| (k.id.as_str(), k))
            .collect();
        let mut updated = Vec::new();
        {
            let mut devices = self.devices.write().await;
            for (id, e) in devices.iter_mut() {
                let mut changed = false;
                if !e.device.online {
                    if let Some(k) = known_by_id.get(id.as_str()) {
                        if e.device.ip != k.ip || e.device.sku != k.sku {
                            e.device.ip = k.ip;
                            e.device.sku = k.sku.clone();
                            changed = true;
                        }
                    }
                }
                let next_name = display_name(store, id, &e.device.sku);
                if e.device.name != next_name {
                    e.device.name = next_name;
                    changed = true;
                }
                if changed {
                    updated.push(e.device.clone());
                }
            }
            for k in &store.data.known_devices {
                if !devices.contains_key(&k.id) {
                    let dev = Device {
                        id: k.id.clone(),
                        ip: k.ip,
                        sku: k.sku.clone(),
                        name: display_name(store, &k.id, &k.sku),
                        online: false,
                        state: None,
                        firmware: None,
                        last_color: None,
                        last_kelvin: None,
                    };
                    updated.push(dev.clone());
                    devices.insert(
                        k.id.clone(),
                        Entry {
                            device: dev,
                            last_seen: None,
                            last_command: None,
                            missed_polls: 0,
                            generation: 0,
                        },
                    );
                }
            }
        }
        for device in updated {
            self.emit(Event::DeviceUpdated { device });
        }
        self.emit(Event::ConfigChanged);
    }

    async fn send(&self, cmd: Command, to: SocketAddr) -> Result<()> {
        debug!(?cmd, %to, "send");
        let json = cmd.to_json().to_string();
        let poll = matches!(cmd, Command::DevStatus);
        let label = self.device_label_for_ip(to.ip()).await;
        let mut res = self.socket().send_to(json.as_bytes(), to).await;
        if let Err(ref e) = res {
            if is_fatal_socket_error(e) {
                self.sock_dead.store(true, Ordering::Release);
                if let Ok(rebound) = self.rebind_socket("send after OS wake").await {
                    res = rebound.send_to(json.as_bytes(), to).await;
                }
            }
        }
        match &res {
            Ok(_) => self.log("tx", to.to_string(), label, json, poll).await,
            Err(e) => self.log("tx", to.to_string(), label, format!("ERROR {e}: {json}"), false).await,
        }
        res?;
        Ok(())
    }

    async fn send_scan(&self) -> Result<()> {
        let _guard = self.scan_lock.lock().await;
        let json = Command::Scan.to_json().to_string();
        let bytes = json.as_bytes();
        let mut explicit_unicast_ips = HashSet::new();
        let mut had_multicast_target = false;
        let mut any_multicast_ok = false;

        for target in &self.cfg.scan_targets {
            let multicast = matches!(target, SocketAddr::V4(v4) if v4.ip().is_multicast());
            if !multicast {
                if let SocketAddr::V4(v4) = target {
                    if v4.port() == self.cfg.scan_port {
                        explicit_unicast_ips.insert(*v4.ip());
                    }
                }
                let mut res = self.socket().send_to(bytes, target).await;
                if let Err(ref e) = res {
                    if is_fatal_socket_error(e) {
                        self.sock_dead.store(true, Ordering::Release);
                        if let Ok(rebound) = self.rebind_socket("scan after OS wake").await {
                            res = rebound.send_to(bytes, target).await;
                        }
                    }
                }
                match res {
                    Ok(_) => self.log("tx", target.to_string(), None, json.clone(), false).await,
                    Err(e) => {
                        self.log(
                            "tx",
                            target.to_string(),
                            None,
                            format!("ERROR {e}: {json}"),
                            false,
                        )
                        .await;
                    }
                }
                continue;
            }

            had_multicast_target = true;
            // Send the multicast scan out of every interface so multi-NIC
            // machines (VPN, ethernet + wifi, ...) still find devices.
            let ifaces = local_ipv4_interfaces();
            let sock = self.socket();
            if ifaces.is_empty() {
                match sock.send_to(bytes, target).await {
                    Ok(_) => any_multicast_ok = true,
                    Err(e) => debug!(error = %e, "multicast send without interface list failed"),
                }
            } else {
                let sref = socket2::SockRef::from(&*sock);
                for ip in &ifaces {
                    if let Err(e) = sref.set_multicast_if_v4(ip) {
                        debug!(%ip, error = %e, "cannot use interface for multicast");
                        continue;
                    }
                    match sock.send_to(bytes, target).await {
                        Ok(_) => any_multicast_ok = true,
                        Err(e) => debug!(%ip, error = %e, "multicast send failed"),
                    }
                }
            }
            if any_multicast_ok {
                self.log("tx", target.to_string(), None, json.clone(), false).await;
            } else {
                debug!(%target, "multicast scan unavailable on all interfaces");
            }
        }

        // Sweep private LAN `/24` hosts via unicast UDP when configured (default on iOS,
        // where Apple gates `239.255.255.250` behind a paid multicast entitlement) or
        // when multicast failed on every interface.
        let run_subnet_sweep = self.cfg.subnet_scan
            || (had_multicast_target && !any_multicast_ok && explicit_unicast_ips.is_empty());
        if run_subnet_sweep {
            let mut targets: Vec<Ipv4Addr> = Vec::new();
            let mut seen: HashSet<Ipv4Addr> = explicit_unicast_ips;
            {
                let devices = self.devices.read().await;
                for e in devices.values() {
                    if seen.insert(e.device.ip) {
                        targets.push(e.device.ip);
                    }
                }
            }
            for ip in lan_ipv4_subnet_hosts() {
                if targets.len() >= MAX_SUBNET_HOSTS {
                    break;
                }
                if seen.insert(ip) {
                    targets.push(ip);
                }
            }

            if !targets.is_empty() {
                let mut sock = self.socket();
                let mut sent_ok = 0usize;
                for (idx, ip) in targets.iter().copied().enumerate() {
                    if idx > 0 && idx % UNICAST_SCAN_BATCH == 0 {
                        tokio::time::sleep(UNICAST_SCAN_PACE).await;
                    }
                    let addr = SocketAddr::from((ip, self.cfg.scan_port));
                    match sock.send_to(bytes, addr).await {
                        Ok(_) => sent_ok += 1,
                        Err(e) if is_fatal_socket_error(&e) => {
                            self.sock_dead.store(true, Ordering::Release);
                            if let Ok(rebound) = self.rebind_socket("subnet scan socket recovery").await {
                                sock = rebound;
                                if sock.send_to(bytes, addr).await.is_ok() {
                                    sent_ok += 1;
                                }
                            }
                        }
                        Err(e) => {
                            debug!(%ip, error = %e, "unicast scan send failed");
                        }
                    }
                }
                self.log(
                    "tx",
                    format!("lan-unicast:{}", self.cfg.scan_port),
                    None,
                    format!("{json} (unicast scan to {sent_ok}/{} LAN hosts)", targets.len()),
                    false,
                )
                .await;
            }
        }
        Ok(())
    }

    async fn refresh_all(&self) {
        let mut ips: Vec<Ipv4Addr> = self.devices.read().await.values().map(|e| e.device.ip).collect();
        ips.sort();
        ips.dedup();
        self.poll_ips(ips).await;
    }

    /// Used by the background poll loop and focus sync: skip devices whose own
    /// post-command `refresh_delay` check is still pending or that are actively streaming Screen Sync.
    async fn refresh_idle(&self) {
        let synced_ids: HashSet<String> = {
            let rt = self.sync_rt.lock().await;
            let mut set: HashSet<String> = rt.sessions.keys().cloned().collect();
            if let Some(id_pulse) = &rt.identify {
                set.insert(id_pulse.device_id.clone());
            }
            set
        };
        let settle = self.cfg.refresh_delay + Duration::from_millis(250);
        let mut ips: Vec<Ipv4Addr> = self
            .devices
            .read()
            .await
            .values()
            .filter(|e| !synced_ids.contains(&e.device.id))
            .filter(|e| e.last_command.map(|t| t.elapsed() >= settle).unwrap_or(true))
            .map(|e| e.device.ip)
            .collect();
        ips.sort();
        ips.dedup();
        self.poll_ips(ips).await;
    }

    async fn poll_ips(&self, ips: Vec<Ipv4Addr>) {
        if ips.is_empty() {
            return;
        }
        *self.last_refresh.lock().await = Some(Instant::now());
        {
            let mut devices = self.devices.write().await;
            for e in devices.values_mut() {
                if ips.contains(&e.device.ip) {
                    e.missed_polls = e.missed_polls.saturating_add(1);
                }
            }
        }
        for ip in ips {
            let _ = self.send(Command::DevStatus, SocketAddr::from((ip, self.cfg.command_port))).await;
        }
    }

    async fn send_action(self: &Arc<Self>, id: &str, action: Action) -> Result<()> {
        let cmd = match action {
            Action::Power(on) => Command::Turn(on),
            Action::Brightness(b) => Command::Brightness(b.clamp(1, 100)),
            Action::Color(c) => Command::Color(c),
            Action::Kelvin(k) => Command::ColorTemp(k.clamp(KELVIN_MIN, KELVIN_MAX)),
            Action::Toggle => {
                let on = self
                    .devices
                    .read()
                    .await
                    .get(id)
                    .and_then(|e| e.device.state)
                    .map(|s| s.on)
                    .unwrap_or(false);
                Command::Turn(!on)
            }
        };

        // Optimistic update so the UI feels instant.
        let (ip, generation, dev) = {
            let mut devices = self.devices.write().await;
            let e = devices.get_mut(id).ok_or_else(|| Error::UnknownDevice(id.into()))?;
            let mut s = e.device.state.unwrap_or_default();
            match &cmd {
                Command::Turn(on) => s.on = *on,
                Command::Brightness(b) => s.brightness = *b,
                Command::Color(c) => {
                    s.color = *c;
                    s.kelvin = 0;
                    e.device.last_color = Some(*c);
                }
                Command::ColorTemp(k) => {
                    s.kelvin = *k;
                    e.device.last_kelvin = Some(*k);
                }
                _ => {}
            }
            e.device.state = Some(s);
            e.last_command = Some(Instant::now());
            e.generation += 1;
            (e.device.ip, e.generation, e.device.clone())
        };

        self.send(cmd, SocketAddr::from((ip, self.cfg.command_port))).await?;
        self.emit(Event::DeviceUpdated { device: dev });

        let is_synced = self.sync_rt.lock().await.sessions.contains_key(id);
        if !is_synced {
            // Confirm the real state once the user has stopped fiddling.
            let me = self.clone();
            let id = id.to_string();
            tokio::spawn(async move {
                tokio::time::sleep(me.cfg.refresh_delay).await;
                let still_unsynced = !me.sync_rt.lock().await.sessions.contains_key(&id);
                if !still_unsynced {
                    return;
                }
                let current = me.devices.read().await.get(&id).map(|e| (e.generation, e.device.ip));
                if let Some((g, ip)) = current {
                    if g == generation {
                        let _ = me.send(Command::DevStatus, SocketAddr::from((ip, me.cfg.command_port))).await;
                    }
                }
            });
        }
        Ok(())
    }

    // ---- screen sync internals -------------------------------------------

    async fn reconciled_sync_config(&self) -> SyncConfig {
        let dev_pairs: Vec<(String, String)> = self
            .devices
            .read()
            .await
            .values()
            .map(|e| (e.device.id.clone(), e.device.sku.clone()))
            .collect();
        let mut cfg = self.store.lock().await.data.sync.clone();
        reconcile_placements(
            &mut cfg,
            dev_pairs.iter().map(|(id, sku)| (id.as_str(), sku.as_str())),
        );
        cfg
    }

    async fn current_sync_status(&self) -> SyncStatus {
        let cfg = self.reconciled_sync_config().await;
        let (active, mut active_devices) = {
            let rt = self.sync_rt.lock().await;
            (rt.active, rt.sessions.keys().cloned().collect::<Vec<_>>())
        };
        active_devices.sort();
        SyncStatus { active, active_devices, config: cfg }
    }

    async fn update_sync_config(&self, config: SyncConfig) -> Result<SyncStatus> {
        let dev_pairs: Vec<(String, String)> = self
            .devices
            .read()
            .await
            .values()
            .map(|e| (e.device.id.clone(), e.device.sku.clone()))
            .collect();

        let mut sanitized = config.sanitized();
        reconcile_placements(
            &mut sanitized,
            dev_pairs.iter().map(|(id, sku)| (id.as_str(), sku.as_str())),
        );

        {
            let mut store = self.store.lock().await;
            store.data.sync = sanitized.clone();
            store.save()?;
        }
        self.emit(Event::ConfigChanged);

        // If Screen Sync is actively streaming, reconcile newly excluded/included or mode-changed lights live.
        let is_active = {
            let mut rt = self.sync_rt.lock().await;
            rt.preview_dirty = true;
            rt.active
        };
        if is_active {
            self.reconcile_live_sync_sessions(&sanitized).await;
        }

        let status = self.current_sync_status().await;
        self.emit(Event::SyncChanged { status: status.clone() });
        self.sync_notify.notify_one();
        Ok(status)
    }

    async fn start_sync(&self) -> Result<SyncStatus> {
        let cfg = self.reconciled_sync_config().await;
        let enabled_ids: HashSet<String> = cfg
            .placements
            .iter()
            .filter(|p| !p.excluded)
            .map(|p| p.device_id.clone())
            .collect();
        if enabled_ids.is_empty() {
            return Err(Error::NoSyncDevices);
        }

        let candidates: Vec<(Device, DeviceState, bool)> = {
            let devices = self.devices.read().await;
            let mut out = Vec::new();
            for id in &enabled_ids {
                let Some(e) = devices.get(id) else { continue };
                if !e.device.online {
                    continue;
                }
                let snap = snapshot_for_device(&e.device);
                let placements: Vec<&SyncPlacement> = cfg
                    .placements
                    .iter()
                    .filter(|p| &p.device_id == id && !p.excluded)
                    .collect();
                let uses_razer = device_uses_razer(&e.device.sku, &placements);
                out.push((e.device.clone(), snap, uses_razer));
            }
            out
        };

        if candidates.is_empty() {
            return Err(Error::NoOnlineSyncDevices);
        }

        // Wake any sleeping lights before entering Razer streaming mode.
        let mut woke_any = false;
        for (dev, snap, _) in &candidates {
            if !snap.on {
                woke_any = true;
                let addr = SocketAddr::from((dev.ip, self.cfg.command_port));
                let _ = self.send(Command::Turn(true), addr).await;
                let updated = {
                    let mut devices = self.devices.write().await;
                    devices.get_mut(&dev.id).map(|e| {
                        let mut s = e.device.state.unwrap_or(*snap);
                        s.on = true;
                        e.device.state = Some(s);
                        e.last_command = Some(Instant::now());
                        e.generation += 1;
                        e.device.clone()
                    })
                };
                if let Some(d) = updated {
                    self.emit(Event::DeviceUpdated { device: d });
                }
            }
        }
        if woke_any {
            tokio::time::sleep(Duration::from_millis(85)).await;
        }

        let now = Instant::now();
        let mut new_sessions = HashMap::new();
        for (dev, snap, uses_razer) in &candidates {
            if *uses_razer {
                let addr = SocketAddr::from((dev.ip, self.cfg.command_port));
                let _ = self.send(Command::RazerEnable(true), addr).await;
            }
            new_sessions.insert(
                dev.id.clone(),
                SyncDeviceSession {
                    ip: dev.ip,
                    sku: dev.sku.clone(),
                    snapshot: *snap,
                    uses_razer: *uses_razer,
                    last_razer: None,
                    last_solid: None,
                    last_sent_at: None,
                    last_razer_enable_at: now,
                },
            );
        }

        let count = new_sessions.len();
        {
            let mut rt = self.sync_rt.lock().await;
            // Preserve pre-sync snapshots for any device that was already in an active session.
            for (id, prev) in std::mem::take(&mut rt.sessions) {
                if let Some(next) = new_sessions.get_mut(&id) {
                    next.snapshot = prev.snapshot;
                }
            }
            rt.active = true;
            rt.detached.clear();
            rt.processor.reset();
            rt.sessions = new_sessions;
        }

        self.log(
            "action",
            "",
            None,
            format!("Screen Sync started ({count} {})", if count == 1 { "light" } else { "lights" }),
            false,
        )
        .await;

        let status = self.current_sync_status().await;
        self.emit(Event::SyncChanged { status: status.clone() });
        self.sync_notify.notify_one();
        Ok(status)
    }

    async fn stop_sync(&self, reason: Option<&str>) -> SyncStatus {
        let to_restore: Vec<(String, SyncDeviceSession)> = {
            let mut rt = self.sync_rt.lock().await;
            if !rt.active && rt.sessions.is_empty() {
                drop(rt);
                return self.current_sync_status().await;
            }
            rt.active = false;
            rt.detached.clear();
            rt.processor.reset();
            rt.sessions.drain().collect()
        };

        let status = self.current_sync_status().await;
        let msg = match reason {
            Some(r) if !r.is_empty() => format!("Screen Sync stopped ({r})"),
            _ => "Screen Sync stopped".to_string(),
        };
        self.log("action", "", None, msg, false).await;
        self.emit(Event::SyncChanged { status: status.clone() });

        for (id, sess) in to_restore {
            self.restore_device_snapshot(&id, sess.ip, sess.uses_razer, sess.snapshot).await;
        }
        status
    }

    async fn abort_sync_without_restore(&self, reason: &str) {
        let to_disable: Vec<(Ipv4Addr, bool)> = {
            let mut rt = self.sync_rt.lock().await;
            if !rt.active && rt.sessions.is_empty() {
                return;
            }
            rt.active = false;
            rt.detached.clear();
            rt.processor.reset();
            rt.sessions.drain().map(|(_, s)| (s.ip, s.uses_razer)).collect()
        };
        self.log("action", "", None, format!("Screen Sync stopped ({reason})"), false).await;
        for (ip, uses_razer) in to_disable {
            if uses_razer {
                let _ = self.send(Command::RazerEnable(false), SocketAddr::from((ip, self.cfg.command_port))).await;
            }
        }
        let status = self.current_sync_status().await;
        self.emit(Event::SyncChanged { status });
    }

    async fn detach_synced_devices(&self, ids: &[String]) {
        let mut detached_razer = Vec::new();
        let mut stopped_all = false;
        let mut any_removed = false;
        {
            let mut rt = self.sync_rt.lock().await;
            if !rt.active {
                return;
            }
            for id in ids {
                if let Some(sess) = rt.sessions.remove(id) {
                    any_removed = true;
                    rt.detached.insert(id.clone());
                    if sess.uses_razer {
                        detached_razer.push(sess.ip);
                    }
                }
            }
            if any_removed && rt.sessions.is_empty() {
                rt.active = false;
                rt.detached.clear();
                rt.processor.reset();
                stopped_all = true;
            }
        }
        if !any_removed {
            return;
        }
        for ip in detached_razer {
            let _ = self.send(Command::RazerEnable(false), SocketAddr::from((ip, self.cfg.command_port))).await;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        if stopped_all {
            self.log("action", "", None, "Screen Sync stopped (all synced lights detached)", false).await;
        }
        let status = self.current_sync_status().await;
        self.emit(Event::SyncChanged { status });
    }

    async fn reconcile_live_sync_sessions(&self, cfg: &SyncConfig) {
        let enabled_by_dev: HashMap<String, Vec<&SyncPlacement>> = {
            let mut m: HashMap<String, Vec<&SyncPlacement>> = HashMap::new();
            for p in cfg.placements.iter().filter(|p| !p.excluded) {
                m.entry(p.device_id.clone()).or_default().push(p);
            }
            m
        };

        let online_devs: HashMap<String, Device> = self
            .devices
            .read()
            .await
            .values()
            .filter(|e| e.device.online)
            .map(|e| (e.device.id.clone(), e.device.clone()))
            .collect();

        let mut to_restore = Vec::new();
        let mut razer_toggles: Vec<(Ipv4Addr, bool)> = Vec::new();
        let mut to_wake: Vec<(Device, DeviceState, bool)> = Vec::new();
        let mut auto_stopped = false;

        {
            let mut rt = self.sync_rt.lock().await;
            if !rt.active {
                return;
            }
            // Remove devices that were excluded in config.
            let existing_ids: Vec<String> = rt.sessions.keys().cloned().collect();
            for id in existing_ids {
                match enabled_by_dev.get(&id) {
                    None => {
                        if let Some(sess) = rt.sessions.remove(&id) {
                            to_restore.push((id, sess));
                        }
                    }
                    Some(placements) => {
                        if let Some(sess) = rt.sessions.get_mut(&id) {
                            let next_razer = device_uses_razer(&sess.sku, placements);
                            if sess.uses_razer != next_razer {
                                sess.uses_razer = next_razer;
                                sess.last_razer = None;
                                sess.last_solid = None;
                                sess.last_sent_at = None;
                                sess.last_razer_enable_at = Instant::now();
                                razer_toggles.push((sess.ip, next_razer));
                            }
                        }
                    }
                }
            }
            // Attach newly un-excluded online devices.
            for (id, placements) in &enabled_by_dev {
                if !rt.sessions.contains_key(id) && !rt.detached.contains(id) {
                    if let Some(dev) = online_devs.get(id) {
                        let snap = snapshot_for_device(dev);
                        let uses_razer = device_uses_razer(&dev.sku, placements);
                        to_wake.push((dev.clone(), snap, uses_razer));
                    }
                }
            }
            if rt.sessions.is_empty() && to_wake.is_empty() {
                rt.active = false;
                rt.processor.reset();
                auto_stopped = true;
            }
        }

        for (id, sess) in to_restore {
            self.restore_device_snapshot(&id, sess.ip, sess.uses_razer, sess.snapshot).await;
        }
        for (ip, en) in razer_toggles {
            let _ = self.send(Command::RazerEnable(en), SocketAddr::from((ip, self.cfg.command_port))).await;
        }
        for (dev, snap, uses_razer) in to_wake {
            let addr = SocketAddr::from((dev.ip, self.cfg.command_port));
            if !snap.on {
                let _ = self.send(Command::Turn(true), addr).await;
                tokio::time::sleep(Duration::from_millis(85)).await;
            }
            if uses_razer {
                let _ = self.send(Command::RazerEnable(true), addr).await;
            }
            let mut rt = self.sync_rt.lock().await;
            if rt.active {
                rt.sessions.insert(
                    dev.id.clone(),
                    SyncDeviceSession {
                        ip: dev.ip,
                        sku: dev.sku.clone(),
                        snapshot: snap,
                        uses_razer,
                        last_razer: None,
                        last_solid: None,
                        last_sent_at: None,
                        last_razer_enable_at: Instant::now(),
                    },
                );
            }
        }
        if auto_stopped {
            self.log("action", "", None, "Screen Sync stopped (no active lights remaining)", false).await;
        }
    }

    async fn restore_device_snapshot(
        &self,
        id: &str,
        ip: Ipv4Addr,
        uses_razer: bool,
        snap: DeviceState,
    ) {
        let addr = SocketAddr::from((ip, self.cfg.command_port));
        if uses_razer {
            let _ = self.send(Command::RazerEnable(false), addr).await;
            tokio::time::sleep(Duration::from_millis(35)).await;
        }
        if !snap.on {
            let _ = self.send(Command::Turn(false), addr).await;
        } else {
            let _ = self.send(Command::Turn(true), addr).await;
            tokio::time::sleep(Duration::from_millis(25)).await;
            let _ = self.send(Command::Brightness(snap.brightness.clamp(1, 100)), addr).await;
            tokio::time::sleep(Duration::from_millis(25)).await;
            if snap.kelvin > 0 {
                let _ = self.send(Command::ColorTemp(snap.kelvin), addr).await;
            } else if snap.color != Rgb::new(0, 0, 0) {
                let _ = self.send(Command::Color(snap.color), addr).await;
            }
        }

        let updated = {
            let mut devices = self.devices.write().await;
            devices.get_mut(id).map(|e| {
                e.device.state = Some(snap);
                e.last_command = Some(Instant::now());
                e.generation += 1;
                e.device.clone()
            })
        };
        if let Some(device) = updated {
            self.emit(Event::DeviceUpdated { device });
        }
    }

    async fn identify_sync_placement(&self, placement_id: &str) -> Result<()> {
        let cfg = self.reconciled_sync_config().await;
        let placement = cfg
            .placements
            .iter()
            .find(|p| p.id == placement_id)
            .cloned()
            .ok_or_else(|| Error::UnknownPlacement(placement_id.to_string()))?;

        let (dev, snap) = {
            let devices = self.devices.read().await;
            let e = devices
                .get(&placement.device_id)
                .ok_or_else(|| Error::UnknownDevice(placement.device_id.clone()))?;
            (e.device.clone(), snapshot_for_device(&e.device))
        };

        let dev_placements: Vec<&SyncPlacement> = cfg
            .placements
            .iter()
            .filter(|p| p.device_id == dev.id)
            .collect();
        let uses_razer = device_uses_razer(&dev.sku, &dev_placements);

        let already_synced = {
            let rt = self.sync_rt.lock().await;
            rt.active && rt.sessions.contains_key(&dev.id)
        };

        let addr = SocketAddr::from((dev.ip, self.cfg.command_port));
        if !already_synced {
            if !snap.on {
                let _ = self.send(Command::Turn(true), addr).await;
                tokio::time::sleep(Duration::from_millis(85)).await;
            }
            if uses_razer {
                let _ = self.send(Command::RazerEnable(true), addr).await;
            }
        }

        {
            let mut rt = self.sync_rt.lock().await;
            // Preserve an earlier `restore_after` if the user clicks Identify twice in a row.
            let prev_restore = rt
                .identify
                .as_ref()
                .filter(|prev| prev.device_id == dev.id)
                .and_then(|prev| prev.restore_after);
            rt.identify = Some(IdentifyPulse {
                placement_id: placement.id,
                device_id: dev.id,
                ip: dev.ip,
                sku: dev.sku,
                uses_razer,
                started_at: Instant::now(),
                duration: Duration::from_millis(850),
                restore_after: if already_synced { None } else { prev_restore.or(Some(snap)) },
            });
        }
        self.sync_notify.notify_one();
        Ok(())
    }

    async fn sync_tick(&self, dt_secs: f32, pkt_buf: &mut Vec<u8>) {
        let cfg = self.reconciled_sync_config().await;
        let now = Instant::now();

        let mut raw_sends: Vec<(SocketAddr, Vec<u8>)> = Vec::new();
        let mut finished_identify: Option<IdentifyPulse> = None;
        let mut preview_to_emit = None;

        {
            let mut rt = self.sync_rt.lock().await;
            let identify_dev_id = rt.identify.as_ref().map(|p| p.device_id.clone());

            if let Some(pulse) = &rt.identify {
                let elapsed = now.saturating_duration_since(pulse.started_at);
                if elapsed >= pulse.duration {
                    finished_identify = rt.identify.take();
                } else {
                    let progress = (elapsed.as_secs_f32() / pulse.duration.as_secs_f32()).clamp(0.0, 1.0);
                    let addr = SocketAddr::from((pulse.ip, self.cfg.command_port));
                    if pulse.uses_razer {
                        let colors = build_identify_razer_colors(
                            &cfg,
                            &pulse.device_id,
                            &pulse.sku,
                            &pulse.placement_id,
                            progress,
                        );
                        Command::write_razer_frame_bytes(true, &colors, pkt_buf);
                        raw_sends.push((addr, pkt_buf.clone()));
                    } else if elapsed.as_millis() < 120 {
                        raw_sends.push((addr, Command::Color(Rgb::new(255, 176, 32)).to_bytes()));
                    }
                }
            }

            if let Some(frame) = rt.frame.clone() {
                let active_skus: HashMap<String, String> = if rt.active {
                    rt.sessions
                        .iter()
                        .filter(|(id, _)| Some(id.as_str()) != identify_dev_id.as_deref())
                        .map(|(id, s)| (id.clone(), s.sku.clone()))
                        .collect()
                } else {
                    HashMap::new()
                };

                if rt.active || rt.preview_enabled {
                    let out = rt.processor.step(&frame, &cfg, &active_skus, dt_secs);

                    for (dev_id, dev_cmd) in out.commands {
                        let Some(sess) = rt.sessions.get_mut(&dev_id) else { continue };
                        let addr = SocketAddr::from((sess.ip, self.cfg.command_port));

                        if sess.uses_razer && now.saturating_duration_since(sess.last_razer_enable_at) >= RAZER_REASSERT {
                            sess.last_razer_enable_at = now;
                            raw_sends.push((addr, Command::RazerEnable(true).to_bytes()));
                        }

                        match dev_cmd {
                            DeviceSyncCommand::Razer { colors } => {
                                let keepalive_due = sess
                                    .last_sent_at
                                    .map(|t| now.saturating_duration_since(t) >= RAZER_KEEPALIVE)
                                    .unwrap_or(true);
                                let changed = sess
                                    .last_razer
                                    .as_ref()
                                    .map(|prev| slice_color_diff(prev, &colors) >= 2)
                                    .unwrap_or(true);
                                if changed || keepalive_due {
                                    Command::write_razer_frame_bytes(true, &colors, pkt_buf);
                                    raw_sends.push((addr, pkt_buf.clone()));
                                    sess.last_razer = Some(colors);
                                    sess.last_sent_at = Some(now);
                                }
                            }
                            DeviceSyncCommand::Solid { color } => {
                                let interval_ok = sess
                                    .last_sent_at
                                    .map(|t| now.saturating_duration_since(t) >= SOLID_MIN_INTERVAL)
                                    .unwrap_or(true);
                                let changed = sess
                                    .last_solid
                                    .map(|prev| rgb_max_diff(prev, color) >= 2)
                                    .unwrap_or(true);
                                if interval_ok && changed {
                                    raw_sends.push((addr, Command::Color(color).to_bytes()));
                                    sess.last_solid = Some(color);
                                    sess.last_sent_at = Some(now);
                                }
                            }
                        }
                    }

                    if rt.preview_enabled {
                        let due = rt
                            .last_preview_at
                            .map(|t| now.saturating_duration_since(t) >= PREVIEW_MIN_INTERVAL)
                            .unwrap_or(true);
                        if due {
                            rt.last_preview_at = Some(now);
                            rt.preview_dirty = false;
                            preview_to_emit = Some(out.preview);
                        }
                    }
                }
            }
        }

        let sock = self.socket();
        for (addr, payload) in raw_sends {
            let _ = sock.send_to(&payload, addr).await;
        }

        if let Some(preview) = preview_to_emit {
            self.emit(Event::SyncPreview { preview });
        }

        if let Some(done) = finished_identify {
            if let Some(snap) = done.restore_after {
                self.restore_device_snapshot(&done.device_id, done.ip, done.uses_razer, snap)
                    .await;
            }
        }
    }

    async fn handle(&self, bytes: &[u8], src: SocketAddr) {
        let raw = String::from_utf8_lossy(bytes).trim().to_string();
        match parse_incoming(bytes) {
            Ok(Incoming::Scan(info)) => self.on_scan(info, src, raw).await,
            Ok(Incoming::Status(st)) => self.on_status(st, src, raw).await,
            Ok(Incoming::Other { cmd, .. }) => {
                debug!(%src, cmd, "ignoring message");
                let label = self.device_label_for_ip(src.ip()).await;
                self.log("rx", src.to_string(), label, raw, false).await;
            }
            Err(e) => {
                // Ignore our own multicast scan request echoed back to us.
                if matches!(parse_command(bytes), Ok(Command::Scan)) {
                    return;
                }
                debug!(%src, error = %e, "unparseable datagram");
                let label = self.device_label_for_ip(src.ip()).await;
                self.log("rx", src.to_string(), label, raw, false).await;
            }
        }
    }

    async fn on_scan(&self, info: ScanInfo, src: SocketAddr, raw: String) {
        let ip: Ipv4Addr = match info.ip.parse() {
            Ok(ip) => ip,
            Err(_) => match src.ip() {
                IpAddr::V4(v4) => v4,
                IpAddr::V6(_) => return,
            },
        };
        let fw = format_firmware(&info);
        let mut store = self.store.lock().await;
        let (dev, needs_status) = {
            let mut devices = self.devices.write().await;
            let e = devices.entry(info.device.clone()).or_insert_with(|| {
                info!(id = %info.device, sku = %info.sku, %ip, "discovered device");
                Entry {
                    device: Device {
                        id: info.device.clone(),
                        ip,
                        sku: info.sku.clone(),
                        name: String::new(),
                        online: false,
                        state: None,
                        firmware: None,
                        last_color: None,
                        last_kelvin: None,
                    },
                    last_seen: None,
                    last_command: None,
                    missed_polls: 0,
                    generation: 0,
                }
            });
            let needs_status = !e.device.online || e.device.state.is_none();
            e.device.ip = ip;
            e.device.sku = info.sku.clone();
            e.device.name = display_name(&store, &info.device, &info.sku);
            if fw.is_some() {
                e.device.firmware = fw;
            }
            e.device.online = true;
            e.last_seen = Some(Instant::now());
            e.missed_polls = 0;
            (e.device.clone(), needs_status)
        };
        let is_new_known = store.upsert_known(KnownDevice { id: dev.id.clone(), ip, sku: dev.sku.clone() });
        if is_new_known {
            if let Err(e) = store.save() {
                warn!(error = %e, "failed to save config");
            }
        }
        drop(store);
        let label = Some(format!("{} ({})", dev.name, dev.sku));
        self.log("rx", src.to_string(), label, raw, false).await;
        self.emit(Event::DeviceUpdated { device: dev.clone() });
        if is_new_known {
            let status = self.current_sync_status().await;
            self.emit(Event::SyncChanged { status });
        }
        if needs_status {
            let is_synced = self.sync_rt.lock().await.sessions.contains_key(&dev.id);
            if !is_synced {
                let _ = self.send(Command::DevStatus, SocketAddr::from((ip, self.cfg.command_port))).await;
            }
        }
    }

    async fn on_status(&self, st: StatusInfo, src: SocketAddr, raw: String) {
        let IpAddr::V4(ip) = src.ip() else { return };
        // Ignore color=(0,0,0) status replies from lights currently in Razer streaming mode.
        {
            let synced_match = {
                let rt = self.sync_rt.lock().await;
                rt.sessions.values().any(|s| s.ip == ip)
            };
            if synced_match {
                let mut devices = self.devices.write().await;
                for e in devices.values_mut().filter(|e| e.device.ip == ip) {
                    e.device.online = true;
                    e.last_seen = Some(Instant::now());
                    e.missed_polls = 0;
                }
                return;
            }
        }
        let next = DeviceState {
            on: st.on_off == 1,
            brightness: st.brightness,
            color: st.color,
            kelvin: st.color_tem_in_kelvin,
        };
        let guard = self.cfg.refresh_delay * 3 / 4;
        enum StatusOutcome {
            Unknown,
            Settling(String),
            Updated { prev: Option<DeviceState>, was_online: bool, memo_changed: bool, dev: Device },
        }
        let outcome = {
            let mut devices = self.devices.write().await;
            let key = devices
                .iter()
                .filter(|(_, e)| e.device.ip == ip)
                .max_by_key(|(_, e)| (e.device.online, e.last_seen))
                .map(|(id, _)| id.clone());
            match key.and_then(|id| devices.get_mut(&id)) {
                None => StatusOutcome::Unknown,
                Some(e) => {
                    let prev = e.device.state;
                    let was_online = e.device.online;
                    e.device.online = true;
                    e.last_seen = Some(Instant::now());
                    e.missed_polls = 0;
                    // Govee lights fade over ~1s and report their old `onOff` if a
                    // background poll lands right after a command. Ignore contradictory
                    // status replies until the command's settle window has elapsed.
                    let in_flight = e.last_command.map(|t| t.elapsed() < guard).unwrap_or(false);
                    if in_flight && prev.map(|p| !same_logical_state(&p, &next)).unwrap_or(false) {
                        StatusOutcome::Settling(format!("{} ({})", e.device.name, e.device.sku))
                    } else {
                        let prev_memo = (e.device.last_color, e.device.last_kelvin);
                        // In Kelvin mode, Govee hardware reports `color: (255,255,255)`; only
                        // record the active mode's value so the other mode's memory survives.
                        if next.kelvin > 0 {
                            e.device.last_kelvin = Some(next.kelvin);
                        } else if next.color != Rgb::new(0, 0, 0) {
                            e.device.last_color = Some(next.color);
                        }
                        let memo_changed = (e.device.last_color, e.device.last_kelvin) != prev_memo;
                        e.device.state = Some(next);
                        StatusOutcome::Updated { prev, was_online, memo_changed, dev: e.device.clone() }
                    }
                }
            }
        };
        match outcome {
            StatusOutcome::Unknown => {
                self.log("rx", src.to_string(), None, raw, false).await;
                // A device we haven't identified yet: ask who it is.
                let _ = self.send(Command::Scan, SocketAddr::from((ip, self.cfg.scan_port))).await;
            }
            StatusOutcome::Settling(label) => {
                self.log("rx", src.to_string(), Some(label), format!("{raw} (ignored mid-transition)"), true)
                    .await;
            }
            StatusOutcome::Updated { prev, was_online, memo_changed, dev } => {
                let label = Some(format!("{} ({})", dev.name, dev.sku));
                self.log("rx", src.to_string(), label.clone(), raw, true).await;
                match prev {
                    None => {
                        self.log("state", ip.to_string(), label, format!("initial state: {}", describe_state(&next)), false)
                            .await;
                    }
                    Some(p) if !same_logical_state(&p, &next) => {
                        self.log(
                            "state",
                            ip.to_string(),
                            label,
                            format!("external/reported change: {} -> {}", describe_state(&p), describe_state(&next)),
                            false,
                        )
                        .await;
                    }
                    Some(_) if !was_online => {
                        self.log("state", ip.to_string(), label, format!("back online: {}", describe_state(&next)), false)
                            .await;
                    }
                    _ => {}
                }
                if !was_online || prev != Some(next) || memo_changed {
                    self.emit(Event::DeviceUpdated { device: dev });
                }
            }
        }
    }

    /// Mark devices offline if they've been silent for several poll cycles.
    async fn sweep_offline(&self, interval: Duration) {
        let cutoff = interval * 3 + Duration::from_secs(1);
        let mut changed = vec![];
        {
            let mut devices = self.devices.write().await;
            for e in devices.values_mut() {
                let stale = e.last_seen.map(|t| t.elapsed() > cutoff).unwrap_or(false);
                if e.device.online && e.missed_polls > 3 && stale {
                    e.device.online = false;
                    changed.push(e.device.clone());
                }
            }
        }
        for d in changed {
            let label = Some(format!("{} ({})", d.name, d.sku));
            self.log("state", d.ip.to_string(), label, "went offline (no status reply)", false).await;
            self.emit(Event::DeviceUpdated { device: d });
        }
    }
}

async fn recv_loop(inner: Arc<Inner>) {
    let mut buf = vec![0u8; 8192];
    loop {
        let sock = inner.socket();
        let res = tokio::select! {
            r = sock.recv_from(&mut buf) => r,
            _ = inner.sock_rebound.notified() => continue,
        };
        match res {
            Ok((n, src)) => inner.handle(&buf[..n], src).await,
            Err(e) if is_fatal_socket_error(&e) => {
                warn!(error = %e, "UDP listener invalidated; attempting rebind");
                inner.sock_dead.store(true, Ordering::Release);
                drop(sock);
                if let Err(rebind_err) = inner.rebind_socket("socket invalidated by OS").await {
                    debug!(error = %rebind_err, "UDP socket rebind deferred");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
            Err(e) => {
                // Windows reports ICMP "port unreachable" as ConnectionReset on
                // the next recv; just keep going.
                debug!(error = %e, "recv error");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

async fn poll_loop(inner: Arc<Inner>) {
    let mut rx = inner.poll_tx.subscribe();
    let mut interval = *rx.borrow_and_update();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                inner.refresh_idle().await;
                inner.sweep_offline(interval).await;
            }
            res = rx.changed() => {
                if res.is_err() {
                    break;
                }
                interval = *rx.borrow_and_update();
            }
        }
    }
}

async fn rescan_loop(inner: Arc<Inner>) {
    let mut tick = tokio::time::interval(inner.cfg.rescan_interval);
    tick.tick().await;
    loop {
        tick.tick().await;
        if let Err(e) = inner.send_scan().await {
            debug!(error = %e, "rescan failed");
        }
    }
}

async fn sync_loop(inner: Arc<Inner>) {
    let mut pkt_buf = Vec::with_capacity(256);
    loop {
        // Sleep completely idle until Screen Sync or an Identify sweep is active.
        let running = {
            let rt = inner.sync_rt.lock().await;
            rt.active || rt.identify.is_some() || (rt.preview_enabled && rt.preview_dirty && rt.frame.is_some())
        };
        if !running {
            inner.sync_notify.notified().await;
            continue;
        }

        let mut tick = tokio::time::interval(SYNC_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last = Instant::now();
        loop {
            tick.tick().await;
            let keep_going = {
                let rt = inner.sync_rt.lock().await;
                rt.active || rt.identify.is_some()
            };
            let now = Instant::now();
            let dt = now.saturating_duration_since(last).as_secs_f32().clamp(0.005, 0.25);
            last = now;
            inner.sync_tick(dt, &mut pkt_buf).await;
            if !keep_going {
                break;
            }
        }
    }
}

fn snapshot_for_device(dev: &Device) -> DeviceState {
    if let Some(s) = dev.state {
        if s.kelvin == 0 && s.color == Rgb::new(0, 0, 0) {
            // If the light was running a built-in scene, fall back to its remembered colour or warm white.
            if let Some(c) = dev.last_color {
                return DeviceState { color: c, ..s };
            }
            if let Some(k) = dev.last_kelvin {
                return DeviceState { kelvin: k, ..s };
            }
        }
        return s;
    }
    DeviceState {
        on: false,
        brightness: 80,
        color: dev.last_color.unwrap_or(Rgb::new(255, 180, 100)),
        kelvin: dev.last_kelvin.unwrap_or(0),
    }
}

fn rgb_max_diff(a: Rgb, b: Rgb) -> u8 {
    a.r.abs_diff(b.r).max(a.g.abs_diff(b.g)).max(a.b.abs_diff(b.b))
}

fn slice_color_diff(a: &[Rgb], b: &[Rgb]) -> u8 {
    if a.len() != b.len() {
        return u8::MAX;
    }
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| rgb_max_diff(x, y))
        .max()
        .unwrap_or(0)
}

fn is_fatal_socket_error(e: &io::Error) -> bool {
    if matches!(
        e.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::NotConnected | io::ErrorKind::InvalidInput
    ) {
        return true;
    }
    match e.raw_os_error() {
        // POSIX EBADF (9) or EINVAL (22)
        Some(9 | 22) => true,
        // iOS/macOS ENETDOWN (50) or ENOTCONN (57) after background suspension
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        Some(50 | 57) => true,
        #[cfg(target_os = "linux")]
        Some(100 | 107) => true,
        _ => false,
    }
}

fn bind_socket(addr: SocketAddr) -> io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    // On iOS, allow immediate rebinding on `:4002` when recovering from background
    // suspension; on desktop, keep exclusive port-4002 conflict detection intact.
    #[cfg(target_os = "ios")]
    s.set_reuse_address(true)?;
    s.set_multicast_ttl_v4(2)?;
    s.set_multicast_loop_v4(true)?;
    s.set_nonblocking(true)?;
    s.bind(&addr.into()).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("cannot listen on {addr} ({e}). Is another Govee app (Govee Desktop, SignalRGB, ...) using it?"),
        )
    })?;
    UdpSocket::from_std(s.into())
}

fn local_ipv4_interfaces() -> Vec<Ipv4Addr> {
    match if_addrs::get_if_addrs() {
        Ok(ifaces) => ifaces
            .into_iter()
            .filter(|i| !i.is_loopback())
            .filter_map(|i| match i.ip() {
                IpAddr::V4(v4) if !v4.is_link_local() => Some(v4),
                _ => None,
            })
            .collect(),
        Err(_) => vec![],
    }
}

/// Skip VPN tunnels (e.g. SideStore WireGuard `utun*`), iOS cellular (`pdp_ip*`),
/// Apple peer-to-peer Wi-Fi (`awdl*`, `llw*`), and container bridges when sweeping
/// private `/24` subnets for Govee lights.
fn is_lan_interface_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    const SKIP_PREFIXES: &[&str] = &[
        "lo",
        "utun",
        "tun",
        "tap",
        "wg",
        "tailscale",
        "zt",
        "ipsec",
        "ppp",
        "pdp_ip",
        "llw",
        "awdl",
        "anpi",
        "docker",
        "br-",
        "veth",
        "virbr",
        "vmnet",
        "vboxnet",
    ];
    !lower.is_empty() && !SKIP_PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// Enumerate usable host IPv4 addresses on a private RFC1918 LAN subnet, clamping
/// masks wider than `/24` (or `/32` point-to-point masks) to the local `/24`.
fn subnet_hosts_v4(local_ip: Ipv4Addr, netmask: Ipv4Addr) -> Vec<Ipv4Addr> {
    if !local_ip.is_private() {
        return Vec::new();
    }
    let ip_u = u32::from(local_ip);
    let mask_u = u32::from(netmask);
    // Clamp wide masks (< /24) and degenerate masks (/31, /32) to /24 so we scan
    // at most 253 neighbours per Wi-Fi/LAN interface.
    let eff_mask = if (0xffff_ff00..=0xffff_fffc).contains(&mask_u) {
        mask_u
    } else {
        0xffff_ff00
    };
    let net = ip_u & eff_mask;
    let bcast = net | !eff_mask;
    if bcast <= net + 1 {
        return Vec::new();
    }
    ((net + 1)..bcast)
        .map(Ipv4Addr::from)
        .filter(|&ip| ip != local_ip)
        .collect()
}

fn lan_ipv4_subnet_hosts() -> Vec<Ipv4Addr> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for iface in ifaces {
        if iface.is_loopback() || !is_lan_interface_name(&iface.name) {
            continue;
        }
        if let if_addrs::IfAddr::V4(v4) = iface.addr {
            for host in subnet_hosts_v4(v4.ip, v4.netmask) {
                if seen.insert(host) {
                    out.push(host);
                    if out.len() >= MAX_SUBNET_HOSTS {
                        return out;
                    }
                }
            }
        }
    }
    out
}

fn display_name(store: &Store, id: &str, sku: &str) -> String {
    store.data.nicknames.get(id).cloned().unwrap_or_else(|| friendly_sku_name(sku))
}

fn format_firmware(info: &ScanInfo) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(w) = info.wifi_version_soft.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("wifi {w}"));
    }
    if let Some(b) = info.ble_version_soft.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("ble {b}"));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn same_logical_state(a: &DeviceState, b: &DeviceState) -> bool {
    a.on == b.on
        && a.brightness == b.brightness
        && a.kelvin == b.kelvin
        && (a.kelvin > 0 || a.color == b.color)
}

fn describe_state(s: &DeviceState) -> String {
    let mode = if s.kelvin > 0 {
        format!("{}K", s.kelvin)
    } else if s.color.r == 0 && s.color.g == 0 && s.color.b == 0 {
        "scene".to_string()
    } else {
        s.color.to_hex()
    };
    format!("{} · {}% · {mode}", if s.on { "on" } else { "off" }, s.brightness)
}

fn new_id(prefix: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{prefix}{:x}", nanos & 0xffff_ffff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_vpn_cellular_and_virtual_interfaces() {
        assert!(is_lan_interface_name("en0"));
        assert!(is_lan_interface_name("en1"));
        assert!(is_lan_interface_name("wlan0"));
        assert!(is_lan_interface_name("wlp3s0"));
        assert!(is_lan_interface_name("eth0"));

        // SideStore WireGuard loopback VPN, iOS cellular, AWDL, loopback, Docker
        assert!(!is_lan_interface_name("utun0"));
        assert!(!is_lan_interface_name("utun4"));
        assert!(!is_lan_interface_name("pdp_ip0"));
        assert!(!is_lan_interface_name("awdl0"));
        assert!(!is_lan_interface_name("llw0"));
        assert!(!is_lan_interface_name("ipsec0"));
        assert!(!is_lan_interface_name("wg0"));
        assert!(!is_lan_interface_name("tun0"));
        assert!(!is_lan_interface_name("lo0"));
        assert!(!is_lan_interface_name("docker0"));
    }

    #[test]
    fn enumerates_private_lan_hosts_and_clamps_wide_masks() {
        // Standard /24 Wi-Fi subnet -> 253 neighbours (excludes .0, .255, and self)
        let hosts = subnet_hosts_v4(
            Ipv4Addr::new(192, 168, 1, 25),
            Ipv4Addr::new(255, 255, 255, 0),
        );
        assert_eq!(hosts.len(), 253);
        assert_eq!(hosts.first().copied(), Some(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(hosts.last().copied(), Some(Ipv4Addr::new(192, 168, 1, 254)));
        assert!(!hosts.contains(&Ipv4Addr::new(192, 168, 1, 25)));

        // Wide /8 mask on 10.x -> clamped to 10.0.5.0/24 (253 hosts, not 16M)
        let clamped = subnet_hosts_v4(
            Ipv4Addr::new(10, 0, 5, 10),
            Ipv4Addr::new(255, 0, 0, 0),
        );
        assert_eq!(clamped.len(), 253);
        assert_eq!(clamped.first().copied(), Some(Ipv4Addr::new(10, 0, 5, 1)));
        assert_eq!(clamped.last().copied(), Some(Ipv4Addr::new(10, 0, 5, 254)));

        // Narrow /28 mask -> 13 usable neighbours
        let narrow = subnet_hosts_v4(
            Ipv4Addr::new(172, 16, 0, 2),
            Ipv4Addr::new(255, 255, 255, 240),
        );
        assert_eq!(narrow.len(), 13);
        assert_eq!(narrow.first().copied(), Some(Ipv4Addr::new(172, 16, 0, 1)));
        assert_eq!(narrow.last().copied(), Some(Ipv4Addr::new(172, 16, 0, 14)));

        // CGNAT (100.64.0.0/10 cellular / Tailscale), link-local, and loopback are rejected
        assert!(subnet_hosts_v4(Ipv4Addr::new(100, 64, 12, 34), Ipv4Addr::new(255, 255, 255, 0)).is_empty());
        assert!(subnet_hosts_v4(Ipv4Addr::new(169, 254, 10, 20), Ipv4Addr::new(255, 255, 0, 0)).is_empty());
        assert!(subnet_hosts_v4(Ipv4Addr::LOCALHOST, Ipv4Addr::new(255, 0, 0, 0)).is_empty());
    }
}
