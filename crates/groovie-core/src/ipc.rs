//! Local loopback IPC (`127.0.0.1`) so `grooviectl` works seamlessly and
//! instantaneously alongside the running Groovie desktop tray app without
//! competing for UDP port `4002`.

use crate::model::{Action, Device, Group, Scene, Target};
use crate::store::default_ipc_path;
use crate::sync::{SyncConfig, SyncStatus};
use crate::Groovie;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::collections::hash_map::RandomState;
use std::future::Future;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(150);
const CALL_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_LINE_BYTES: u64 = 65_536;

/// Optional host hook allowing the desktop shell to start/stop OS screen capture
/// and update tray checkmarks when `grooviectl sync` is invoked over IPC.
pub trait SyncController: Send + Sync + 'static {
    fn is_supported(&self) -> bool;

    fn set_or_toggle<'a>(
        &'a self,
        desired: Option<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<SyncStatus, String>> + Send + 'a>>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusInfo {
    pub supported: bool,
    pub active: bool,
    pub active_devices: Vec<String>,
    pub config: SyncConfig,
}

impl SyncStatusInfo {
    pub fn from_status(supported: bool, st: SyncStatus) -> Self {
        Self {
            supported,
            active: st.active,
            active_devices: st.active_devices,
            config: st.config,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgottenDevice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum IpcRequest {
    Ping,
    ListDevices {
        #[serde(default)]
        scan: bool,
        #[serde(default)]
        wait_ms: u64,
    },
    Apply {
        target: String,
        action: Action,
    },
    Rename {
        target: String,
        name: String,
    },
    Forget {
        target: String,
    },
    AddIp {
        ip: Ipv4Addr,
        wait_ms: u64,
    },
    GroupList,
    GroupCreate {
        name: String,
        lights: Vec<String>,
    },
    GroupAdd {
        group: String,
        lights: Vec<String>,
    },
    GroupRemove {
        group: String,
        lights: Vec<String>,
    },
    GroupDelete {
        group: String,
    },
    SceneList,
    SceneSave {
        name: String,
        lights: Vec<String>,
    },
    SceneRename {
        scene: String,
        name: String,
    },
    SceneUpdate {
        scene: String,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        lights: Vec<String>,
    },
    SceneApply {
        scene: String,
    },
    SceneDelete {
        scene: String,
    },
    SyncStatus,
    SyncSet {
        enabled: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DaemonDescriptor {
    port: u16,
    pid: u32,
    token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IpcEnvelope {
    token: String,
    req: IpcRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IpcResponse {
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Result of attempting to forward a CLI operation to a running local daemon.
#[derive(Debug)]
pub enum IpcOutcome<T> {
    /// Handled by the running daemon.
    Ok(T),
    /// No active daemon was reachable on loopback; caller should run standalone.
    Unavailable,
    /// Daemon was reachable and rejected the request (e.g. unknown target name).
    DaemonErr(String),
}

pub struct IpcServer {
    path: PathBuf,
    port: u16,
    token: String,
    task: JoinHandle<()>,
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.task.abort();
        remove_descriptor_if_owned(&self.path, &self.token);
    }
}

impl IpcServer {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn start_default(
        groovie: Groovie,
        sync_ctl: Option<Arc<dyn SyncController>>,
    ) -> io::Result<Option<Self>> {
        let Some(path) = default_ipc_path() else {
            return Ok(None);
        };
        Self::start(groovie, path, sync_ctl).await.map(Some)
    }

    pub async fn start(
        groovie: Groovie,
        path: PathBuf,
        sync_ctl: Option<Arc<dyn SyncController>>,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let token = generate_token();
        let desc = DaemonDescriptor { port, pid: std::process::id(), token: token.clone() };
        write_descriptor(&path, &desc)?;

        let server_token = token.clone();
        let task = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let g = groovie.clone();
                        let ctl = sync_ctl.clone();
                        let expected = server_token.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_conn(stream, g, ctl, &expected).await {
                                debug!(error = %e, "ipc connection closed");
                            }
                        });
                    }
                    Err(e) => {
                        warn!(error = %e, "ipc accept error");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        });

        Ok(Self { path, port, token, task })
    }
}

async fn handle_conn(
    stream: TcpStream,
    g: Groovie,
    sync_ctl: Option<Arc<dyn SyncController>>,
    expected_token: &str,
) -> io::Result<()> {
    let _ = stream.set_nodelay(true);
    let (rd, mut wr) = stream.into_split();
    let mut reader = BufReader::new(rd.take(MAX_LINE_BYTES));
    let mut line = String::new();
    let n = tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "ipc read timeout"))??;
    if n == 0 {
        return Ok(());
    }

    let env: IpcEnvelope = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(e) => {
            write_resp(&mut wr, IpcResponse { ok: false, data: None, error: Some(format!("bad ipc request: {e}")) }).await?;
            return Ok(());
        }
    };

    if !constant_time_eq(env.token.as_bytes(), expected_token.as_bytes()) {
        write_resp(&mut wr, IpcResponse { ok: false, data: None, error: Some("unauthorized".into()) }).await?;
        return Ok(());
    }

    let resp = match dispatch(&g, sync_ctl.as_deref(), env.req).await {
        Ok(val) => IpcResponse { ok: true, data: Some(val), error: None },
        Err(msg) => IpcResponse { ok: false, data: None, error: Some(msg) },
    };
    write_resp(&mut wr, resp).await
}

async fn write_resp(wr: &mut tokio::net::tcp::OwnedWriteHalf, resp: IpcResponse) -> io::Result<()> {
    let mut out = serde_json::to_vec(&resp).map_err(io::Error::other)?;
    out.push(b'\n');
    wr.write_all(&out).await?;
    wr.flush().await
}

async fn resolve_single_device(g: &Groovie, query: &str) -> Result<String, String> {
    match g.resolve(query).await.map_err(|e| e.to_string())? {
        Target::Device(id) => Ok(id),
        _ => Err(format!("'{query}' is not a single light")),
    }
}

async fn resolve_device_list(g: &Groovie, queries: &[String]) -> Result<Vec<String>, String> {
    let mut ids = Vec::with_capacity(queries.len());
    for q in queries {
        ids.push(resolve_single_device(g, q).await?);
    }
    Ok(ids)
}

async fn find_group(g: &Groovie, query: &str) -> Result<Group, String> {
    g.groups()
        .await
        .into_iter()
        .find(|x| x.id == query || x.name.eq_ignore_ascii_case(query))
        .ok_or_else(|| format!("no group called '{query}'"))
}

async fn dispatch(
    g: &Groovie,
    sync_ctl: Option<&dyn SyncController>,
    req: IpcRequest,
) -> Result<Value, String> {
    let to_val = |v: Result<Value, serde_json::Error>| v.map_err(|e| e.to_string());
    match req {
        IpcRequest::Ping => Ok(Value::Bool(true)),
        IpcRequest::ListDevices { scan, wait_ms } => {
            if scan {
                g.discover().await.map_err(|e| e.to_string())?;
                g.refresh_all().await;
                if wait_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(wait_ms)).await;
                }
            }
            let devices: Vec<Device> = g.devices().await;
            to_val(serde_json::to_value(devices))
        }
        IpcRequest::Apply { target, action } => {
            let t = g.resolve(&target).await.map_err(|e| e.to_string())?;
            g.apply(&t, action).await.map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        IpcRequest::Rename { target, name } => {
            let id = resolve_single_device(g, &target).await?;
            g.rename_device(&id, &name).await.map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        IpcRequest::Forget { target } => {
            let id = resolve_single_device(g, &target).await?;
            let name = g
                .device(&id)
                .await
                .map(|d| d.name)
                .unwrap_or_else(|| id.clone());
            g.remove_device(&id).await.map_err(|e| e.to_string())?;
            to_val(serde_json::to_value(ForgottenDevice { id, name }))
        }
        IpcRequest::AddIp { ip, wait_ms } => {
            let found = g
                .add_device_by_ip_and_wait(ip, Duration::from_millis(wait_ms))
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no answer from {ip}; check the IP and that LAN Control is on"))?;
            to_val(serde_json::to_value(found))
        }
        IpcRequest::GroupList => to_val(serde_json::to_value(g.groups().await)),
        IpcRequest::GroupCreate { name, lights } => {
            let ids = resolve_device_list(g, &lights).await?;
            let gr = g.create_group(&name, ids).await.map_err(|e| e.to_string())?;
            to_val(serde_json::to_value(gr))
        }
        IpcRequest::GroupAdd { group, lights } => {
            let mut gr = find_group(g, &group).await?;
            for id in resolve_device_list(g, &lights).await? {
                if !gr.members.contains(&id) {
                    gr.members.push(id);
                }
            }
            g.update_group(gr).await.map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        IpcRequest::GroupRemove { group, lights } => {
            let mut gr = find_group(g, &group).await?;
            let ids = resolve_device_list(g, &lights).await?;
            gr.members.retain(|m| !ids.contains(m));
            g.update_group(gr).await.map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        IpcRequest::GroupDelete { group } => {
            let gr = find_group(g, &group).await?;
            g.delete_group(&gr.id).await.map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        IpcRequest::SceneList => to_val(serde_json::to_value(g.scenes().await)),
        IpcRequest::SceneSave { name, lights } => {
            let ids = if lights.is_empty() {
                None
            } else {
                Some(resolve_device_list(g, &lights).await?)
            };
            let sc: Scene = g.save_scene(&name, ids).await.map_err(|e| e.to_string())?;
            to_val(serde_json::to_value(sc))
        }
        IpcRequest::SceneRename { scene, name } => {
            let sc = g.rename_scene(&scene, &name).await.map_err(|e| e.to_string())?;
            to_val(serde_json::to_value(sc))
        }
        IpcRequest::SceneUpdate { scene, all, lights } => {
            let existing = g
                .scenes()
                .await
                .into_iter()
                .find(|s| s.id == scene || s.name.eq_ignore_ascii_case(&scene))
                .ok_or_else(|| format!("no scene called '{scene}'"))?;
            let ids = if all {
                None
            } else if !lights.is_empty() {
                Some(resolve_device_list(g, &lights).await?)
            } else if !existing.entries.is_empty() {
                Some(existing.entries.into_iter().map(|e| e.device).collect())
            } else {
                None
            };
            let sc = g
                .update_scene(&existing.id, None, ids)
                .await
                .map_err(|e| e.to_string())?;
            to_val(serde_json::to_value(sc))
        }
        IpcRequest::SceneApply { scene } => {
            g.apply_scene(&scene).await.map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        IpcRequest::SceneDelete { scene } => {
            let sc = g
                .scenes()
                .await
                .into_iter()
                .find(|s| s.id == scene || s.name.eq_ignore_ascii_case(&scene))
                .ok_or_else(|| format!("no scene called '{scene}'"))?;
            g.delete_scene(&sc.id).await.map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        IpcRequest::SyncStatus => {
            let supported = sync_ctl.map(|c| c.is_supported()).unwrap_or(true);
            let st = g.sync_status().await;
            to_val(serde_json::to_value(SyncStatusInfo::from_status(supported, st)))
        }
        IpcRequest::SyncSet { enabled } => match sync_ctl {
            Some(ctl) => {
                let supported = ctl.is_supported();
                let st = ctl.set_or_toggle(enabled).await?;
                to_val(serde_json::to_value(SyncStatusInfo::from_status(supported, st)))
            }
            None => {
                let cur = g.sync_status().await;
                let next_en = enabled.unwrap_or(!cur.active);
                let st = if next_en {
                    g.start_sync().await.map_err(|e| e.to_string())?
                } else {
                    g.stop_sync(Some("stopped via CLI")).await
                };
                to_val(serde_json::to_value(SyncStatusInfo::from_status(true, st)))
            }
        },
    }
}

/// Attempt to call the default local Groovie daemon, deserializing the response as `T`.
pub async fn try_call<T: DeserializeOwned>(req: IpcRequest) -> IpcOutcome<T> {
    let Some(path) = default_ipc_path() else {
        return IpcOutcome::Unavailable;
    };
    try_call_at(&path, req).await
}

/// Attempt to call a local Groovie daemon described by `path`.
pub async fn try_call_at<T: DeserializeOwned>(path: &Path, req: IpcRequest) -> IpcOutcome<T> {
    let Ok(bytes) = std::fs::read(path) else {
        return IpcOutcome::Unavailable;
    };
    let Ok(desc) = serde_json::from_slice::<DaemonDescriptor>(&bytes) else {
        return IpcOutcome::Unavailable;
    };

    let Ok(Ok(stream)) = tokio::time::timeout(
        CONNECT_TIMEOUT,
        TcpStream::connect((Ipv4Addr::LOCALHOST, desc.port)),
    )
    .await
    else {
        return IpcOutcome::Unavailable;
    };
    let _ = stream.set_nodelay(true);

    let env = IpcEnvelope { token: desc.token, req };
    let Ok(mut payload) = serde_json::to_vec(&env) else {
        return IpcOutcome::Unavailable;
    };
    payload.push(b'\n');

    let (rd, mut wr) = stream.into_split();
    if wr.write_all(&payload).await.is_err() || wr.flush().await.is_err() {
        return IpcOutcome::Unavailable;
    }

    let mut reader = BufReader::new(rd.take(MAX_LINE_BYTES));
    let mut line = String::new();
    let n = match tokio::time::timeout(CALL_TIMEOUT, reader.read_line(&mut line)).await {
        Ok(Ok(n)) => n,
        Ok(Err(_)) => return IpcOutcome::Unavailable,
        Err(_) => {
            return IpcOutcome::DaemonErr(
                "Groovie desktop app did not respond in time".into(),
            );
        }
    };
    if n == 0 {
        return IpcOutcome::Unavailable;
    }

    let Ok(resp) = serde_json::from_str::<IpcResponse>(line.trim()) else {
        return IpcOutcome::Unavailable;
    };

    if !resp.ok {
        let msg = resp.error.unwrap_or_else(|| "daemon error".into());
        if msg == "unauthorized" {
            return IpcOutcome::Unavailable;
        }
        return IpcOutcome::DaemonErr(msg);
    }

    let val = resp.data.unwrap_or(Value::Null);
    match serde_json::from_value::<T>(val) {
        Ok(out) => IpcOutcome::Ok(out),
        Err(e) => IpcOutcome::DaemonErr(format!("invalid daemon response: {e}")),
    }
}

fn generate_token() -> String {
    let mut out = String::with_capacity(64);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    for round in 0..4u64 {
        let s = RandomState::new();
        let mut h = s.build_hasher();
        h.write_u64(nanos.wrapping_add(round));
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (&x, &y)| acc | (x ^ y)) == 0
}

fn write_descriptor(path: &Path, desc: &DaemonDescriptor) -> io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut bytes = serde_json::to_vec_pretty(desc).map_err(io::Error::other)?;
    bytes.push(b'\n');

    let tmp_name = path
        .file_name()
        .map(|n| format!("{}.tmp", n.to_string_lossy()))
        .unwrap_or_else(|| "daemon.json.tmp".into());
    let tmp = path.with_file_name(tmp_name);

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&bytes)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp, &bytes)?;
    }

    if std::fs::rename(&tmp, path).is_err() {
        std::fs::write(path, &bytes)?;
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(())
}

fn remove_descriptor_if_owned(path: &Path, expected_token: &str) {
    let Ok(bytes) = std::fs::read(path) else { return };
    let Ok(desc) = serde_json::from_slice::<DaemonDescriptor>(&bytes) else { return };
    if desc.token == expected_token {
        let _ = std::fs::remove_file(path);
    }
}
