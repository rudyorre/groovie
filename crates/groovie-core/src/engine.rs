//! The engine: owns the UDP socket, discovers devices, tracks their state,
//! sends commands and persists user config.

use crate::model::*;
use crate::protocol::*;
use crate::store::{default_store_path, KnownDevice, Store};
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

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
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub struct Config {
    /// Where device replies arrive. Govee always replies to port 4002.
    pub listen: SocketAddr,
    /// Where `scan` requests go. Defaults to the Govee multicast group.
    pub scan_targets: Vec<SocketAddr>,
    /// Port used for unicast scans when adding a device by IP.
    pub scan_port: u16,
    /// Port devices accept commands on.
    pub command_port: u16,
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
            store_path: default_store_path(),
            poll_interval: Duration::from_secs(15),
            rescan_interval: Duration::from_secs(60),
            refresh_delay: Duration::from_millis(1500),
        }
    }
}

struct Entry {
    device: Device,
    last_seen: Option<Instant>,
    /// Bumped on every command so stale delayed refreshes can be skipped.
    generation: u64,
}

struct Inner {
    cfg: Config,
    sock: UdpSocket,
    scan_lock: Mutex<()>,
    devices: RwLock<HashMap<String, Entry>>,
    store: Mutex<Store>,
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
        let sock = bind_socket(cfg.listen)?;
        let store = match &cfg.store_path {
            Some(p) => Store::open(p)?,
            None => Store::in_memory(),
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
                    },
                    last_seen: None,
                    generation: 0,
                },
            );
        }

        let (events, _) = broadcast::channel(256);
        let inner = Arc::new(Inner {
            cfg,
            sock,
            scan_lock: Mutex::new(()),
            devices: RwLock::new(devices),
            store: Mutex::new(store),
            events,
        });

        let tasks = vec![
            tokio::spawn(recv_loop(inner.clone())),
            tokio::spawn(poll_loop(inner.clone())),
            tokio::spawn(rescan_loop(inner.clone())),
        ];

        let me = Self { inner, _tasks: Arc::new(TaskGuard(tasks)) };
        me.discover().await?;
        me.refresh_all().await;
        Ok(me)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// Multicast a scan request; replies are handled in the background.
    pub async fn discover(&self) -> Result<()> {
        self.inner.send_scan().await
    }

    /// Ask every known device for its status.
    pub async fn refresh_all(&self) {
        self.inner.refresh_all().await
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
}

impl Inner {
    fn emit(&self, ev: Event) {
        // No subscribers is fine.
        let _ = self.events.send(ev);
    }

    async fn mutate_store(&self, f: impl FnOnce(&mut Store) -> Result<()>) -> Result<()> {
        let mut store = self.store.lock().await;
        f(&mut store)?;
        store.save()?;
        drop(store);
        self.emit(Event::ConfigChanged);
        Ok(())
    }

    async fn send(&self, cmd: Command, to: SocketAddr) -> Result<()> {
        debug!(?cmd, %to, "send");
        self.sock.send_to(&cmd.to_bytes(), to).await?;
        Ok(())
    }

    async fn send_scan(&self) -> Result<()> {
        let _guard = self.scan_lock.lock().await;
        let bytes = Command::Scan.to_bytes();
        for target in &self.cfg.scan_targets {
            let multicast = matches!(target, SocketAddr::V4(v4) if v4.ip().is_multicast());
            if !multicast {
                self.sock.send_to(&bytes, target).await?;
                continue;
            }
            // Send the multicast scan out of every interface so multi-NIC
            // machines (VPN, ethernet + wifi, ...) still find devices.
            let ifaces = local_ipv4_interfaces();
            if ifaces.is_empty() {
                self.sock.send_to(&bytes, target).await?;
            }
            let sref = socket2::SockRef::from(&self.sock);
            for ip in ifaces {
                if let Err(e) = sref.set_multicast_if_v4(&ip) {
                    debug!(%ip, error = %e, "cannot use interface for multicast");
                    continue;
                }
                if let Err(e) = self.sock.send_to(&bytes, target).await {
                    debug!(%ip, error = %e, "multicast send failed");
                }
            }
        }
        Ok(())
    }

    async fn refresh_all(&self) {
        let ips: Vec<Ipv4Addr> = self.devices.read().await.values().map(|e| e.device.ip).collect();
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
                }
                Command::ColorTemp(k) => s.kelvin = *k,
                _ => {}
            }
            e.device.state = Some(s);
            e.generation += 1;
            (e.device.ip, e.generation, e.device.clone())
        };

        self.send(cmd, SocketAddr::from((ip, self.cfg.command_port))).await?;
        self.emit(Event::DeviceUpdated { device: dev });

        // Confirm the real state once the user has stopped fiddling.
        let me = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(me.cfg.refresh_delay).await;
            let current = me.devices.read().await.get(&id).map(|e| (e.generation, e.device.ip));
            if let Some((g, ip)) = current {
                if g == generation {
                    let _ = me.send(Command::DevStatus, SocketAddr::from((ip, me.cfg.command_port))).await;
                }
            }
        });
        Ok(())
    }

    async fn handle(&self, bytes: &[u8], src: SocketAddr) {
        match parse_incoming(bytes) {
            Ok(Incoming::Scan(info)) => self.on_scan(info, src).await,
            Ok(Incoming::Status(st)) => self.on_status(st, src).await,
            Ok(Incoming::Other { cmd, .. }) => debug!(%src, cmd, "ignoring message"),
            // Includes our own multicast scan request echoed back to us.
            Err(e) => debug!(%src, error = %e, "unparseable datagram"),
        }
    }

    async fn on_scan(&self, info: ScanInfo, src: SocketAddr) {
        let ip: Ipv4Addr = match info.ip.parse() {
            Ok(ip) => ip,
            Err(_) => match src.ip() {
                IpAddr::V4(v4) => v4,
                IpAddr::V6(_) => return,
            },
        };
        let mut store = self.store.lock().await;
        let dev = {
            let mut devices = self.devices.write().await;
            let e = devices.entry(info.device.clone()).or_insert_with(|| {
                info!(id = %info.device, sku = %info.sku, %ip, "discovered device");
                Entry {
                    device: Device {
                        id: info.device.clone(),
                        ip,
                        sku: info.sku.clone(),
                        name: String::new(),
                        online: true,
                        state: None,
                    },
                    last_seen: None,
                    generation: 0,
                }
            });
            e.device.ip = ip;
            e.device.sku = info.sku.clone();
            e.device.name = display_name(&store, &info.device, &info.sku);
            e.device.online = true;
            e.last_seen = Some(Instant::now());
            e.device.clone()
        };
        if store.upsert_known(KnownDevice { id: dev.id.clone(), ip, sku: dev.sku.clone() }) {
            if let Err(e) = store.save() {
                warn!(error = %e, "failed to save config");
            }
        }
        drop(store);
        self.emit(Event::DeviceUpdated { device: dev });
        let _ = self.send(Command::DevStatus, SocketAddr::from((ip, self.cfg.command_port))).await;
    }

    async fn on_status(&self, st: StatusInfo, src: SocketAddr) {
        let IpAddr::V4(ip) = src.ip() else { return };
        let dev = {
            let mut devices = self.devices.write().await;
            let Some(e) = devices.values_mut().find(|e| e.device.ip == ip) else {
                drop(devices);
                // A device we haven't identified yet: ask who it is.
                let _ = self.send(Command::Scan, SocketAddr::from((ip, self.cfg.scan_port))).await;
                return;
            };
            e.device.state = Some(DeviceState {
                on: st.on_off == 1,
                brightness: st.brightness,
                color: st.color,
                kelvin: st.color_tem_in_kelvin,
            });
            e.device.online = true;
            e.last_seen = Some(Instant::now());
            e.device.clone()
        };
        self.emit(Event::DeviceUpdated { device: dev });
    }

    /// Mark devices offline if they've been silent for a few poll cycles.
    async fn sweep_offline(&self) {
        let cutoff = self.cfg.poll_interval * 3 + Duration::from_secs(1);
        let mut changed = vec![];
        {
            let mut devices = self.devices.write().await;
            for e in devices.values_mut() {
                let stale = e.last_seen.map(|t| t.elapsed() > cutoff).unwrap_or(false);
                if e.device.online && stale {
                    e.device.online = false;
                    changed.push(e.device.clone());
                }
            }
        }
        for d in changed {
            self.emit(Event::DeviceUpdated { device: d });
        }
    }
}

async fn recv_loop(inner: Arc<Inner>) {
    let mut buf = vec![0u8; 8192];
    loop {
        match inner.sock.recv_from(&mut buf).await {
            Ok((n, src)) => inner.handle(&buf[..n], src).await,
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
    let mut tick = tokio::time::interval(inner.cfg.poll_interval);
    tick.tick().await;
    loop {
        tick.tick().await;
        inner.refresh_all().await;
        inner.sweep_offline().await;
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

fn bind_socket(addr: SocketAddr) -> io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
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

fn display_name(store: &Store, id: &str, sku: &str) -> String {
    store.data.nicknames.get(id).cloned().unwrap_or_else(|| friendly_sku_name(sku))
}

fn new_id(prefix: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{prefix}{:x}", nanos & 0xffff_ffff_ffff)
}
