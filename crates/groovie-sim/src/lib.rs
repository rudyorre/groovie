//! Simulated Govee devices speaking the LAN protocol over real UDP sockets.
//!
//! Each [`SimDevice`] listens for `scan` on `ip:scan_port` (optionally joining
//! the Govee multicast group) and for commands on `ip:command_port`, and
//! replies to `<sender-ip>:reply_port` exactly like real hardware.

use groovie_core::protocol::{parse_command, Command, Rgb, MULTICAST_ADDR};
use groovie_core::DeviceState;
use serde_json::json;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub struct SimConfig {
    pub id: String,
    pub sku: String,
    /// IP the device binds to and reports in scan replies.
    pub ip: Ipv4Addr,
    pub scan_port: u16,
    pub command_port: u16,
    /// Port replies are sent to on the sender's IP (4002 on real hardware).
    pub reply_port: u16,
    /// Bind scan socket to `0.0.0.0` and join `239.255.255.250`.
    pub multicast: bool,
    /// Ignore status requests (simulates a flaky / offline device).
    pub mute: bool,
}

impl SimConfig {
    pub fn new(ip: Ipv4Addr, sku: &str) -> Self {
        let o = ip.octets();
        Self {
            id: format!("AA:BB:CC:DD:EE:{:02X}:{:02X}:{:02X}", o[1], o[2], o[3]),
            sku: sku.into(),
            ip,
            scan_port: 4001,
            command_port: 4003,
            reply_port: 4002,
            multicast: false,
            mute: false,
        }
    }
}

/// Handle to a running simulated device. Dropping it stops the device.
pub struct SimDevice {
    pub config: SimConfig,
    pub state: Arc<Mutex<DeviceState>>,
    /// Every command received, in order (excluding scans).
    pub received: Arc<Mutex<Vec<Command>>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for SimDevice {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl SimDevice {
    pub async fn spawn(config: SimConfig) -> std::io::Result<Self> {
        let state = Arc::new(Mutex::new(DeviceState::default()));
        let received = Arc::new(Mutex::new(Vec::new()));

        let scan_sock = if config.multicast {
            let s = reusable_socket(SocketAddr::from((Ipv4Addr::UNSPECIFIED, config.scan_port)))?;
            s.join_multicast_v4(MULTICAST_ADDR, Ipv4Addr::UNSPECIFIED)?;
            s
        } else {
            UdpSocket::bind((config.ip, config.scan_port)).await?
        };
        let cmd_sock = Arc::new(UdpSocket::bind((config.ip, config.command_port)).await?);

        let scan_task = {
            let cfg = config.clone();
            let reply = cmd_sock.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                loop {
                    let Ok((n, src)) = scan_sock.recv_from(&mut buf).await else { continue };
                    if matches!(parse_command(&buf[..n]), Ok(Command::Scan)) {
                        let msg = json!({"msg":{"cmd":"scan","data":{
                            "ip": cfg.ip.to_string(), "device": cfg.id, "sku": cfg.sku,
                            "bleVersionHard":"3.01.01","bleVersionSoft":"1.03.01",
                            "wifiVersionHard":"1.00.10","wifiVersionSoft":"1.02.03"
                        }}});
                        let to = SocketAddr::new(src.ip(), cfg.reply_port);
                        let _ = reply.send_to(msg.to_string().as_bytes(), to).await;
                    }
                }
            })
        };

        let cmd_task = {
            let cfg = config.clone();
            let state = state.clone();
            let received = received.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                loop {
                    let Ok((n, src)) = cmd_sock.recv_from(&mut buf).await else { continue };
                    let Ok(cmd) = parse_command(&buf[..n]) else { continue };
                    tracing::info!(device = %cfg.id, ?cmd, "received");
                    let reply = {
                        let mut s = state.lock().unwrap();
                        match &cmd {
                            Command::Turn(on) => s.on = *on,
                            Command::Brightness(b) => s.brightness = *b,
                            Command::Color(c) => {
                                s.color = *c;
                                s.kelvin = 0;
                            }
                            Command::ColorTemp(k) => {
                                s.kelvin = *k;
                                s.color = Rgb::new(255, 255, 255);
                            }
                            Command::Scan | Command::DevStatus => {}
                        }
                        (cmd == Command::DevStatus && !cfg.mute).then(|| {
                            json!({"msg":{"cmd":"devStatus","data":{
                                "onOff": u8::from(s.on), "brightness": s.brightness,
                                "color": {"r": s.color.r, "g": s.color.g, "b": s.color.b},
                                "colorTemInKelvin": s.kelvin
                            }}})
                        })
                    };
                    if cmd != Command::Scan {
                        received.lock().unwrap().push(cmd);
                    }
                    if let Some(msg) = reply {
                        let to = SocketAddr::new(src.ip(), cfg.reply_port);
                        let _ = cmd_sock.send_to(msg.to_string().as_bytes(), to).await;
                    }
                }
            })
        };

        Ok(Self { config, state, received, tasks: vec![scan_task, cmd_task] })
    }

    pub fn state(&self) -> DeviceState {
        *self.state.lock().unwrap()
    }

    pub fn received(&self) -> Vec<Command> {
        self.received.lock().unwrap().clone()
    }
}

fn reusable_socket(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    s.set_nonblocking(true)?;
    s.bind(&addr.into())?;
    UdpSocket::from_std(s.into())
}
