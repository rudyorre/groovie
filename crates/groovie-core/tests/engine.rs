//! End-to-end tests: the real engine talking UDP to simulated devices on
//! loopback addresses. Linux-only because they bind 127.0.x.y (macOS needs
//! loopback aliases for that).
#![cfg(target_os = "linux")]

use groovie_core::protocol::Command;
use groovie_core::{Action, Config, Groovie, Rgb, Target};
use groovie_sim::{SimConfig, SimDevice};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

struct Rig {
    g: Groovie,
    sims: Vec<SimDevice>,
}

fn sim_ip(net: u8, i: u8) -> Ipv4Addr {
    Ipv4Addr::new(127, 0, net, 10 + i)
}

async fn spawn_sims(net: u8, base: u16, n: u8) -> Vec<SimDevice> {
    let mut sims = vec![];
    for i in 0..n {
        let mut c = SimConfig::new(sim_ip(net, i), if i == 0 { "H6076" } else { "H6046" });
        c.scan_port = base;
        c.command_port = base + 1;
        c.reply_port = base + 2;
        sims.push(SimDevice::spawn(c).await.unwrap());
    }
    sims
}

fn config(net: u8, base: u16, n: u8, store: Option<PathBuf>) -> Config {
    Config {
        listen: SocketAddr::from((Ipv4Addr::new(127, 0, net, 1), base + 2)),
        scan_targets: (0..n).map(|i| SocketAddr::from((sim_ip(net, i), base))).collect(),
        scan_port: base,
        command_port: base + 1,
        store_path: store,
        poll_interval: Duration::from_secs(60),
        rescan_interval: Duration::from_secs(60),
        refresh_delay: Duration::from_millis(100),
    }
}

async fn eventually<F, Fut>(what: &str, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..100 {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for: {what}");
}

async fn rig(net: u8, base: u16, n: u8) -> Rig {
    let sims = spawn_sims(net, base, n).await;
    let g = Groovie::start(config(net, base, n, None)).await.unwrap();
    let g2 = g.clone();
    eventually("all devices discovered with state", || {
        let g = g2.clone();
        async move {
            let d = g.devices().await;
            d.len() == n as usize && d.iter().all(|d| d.online && d.state.is_some())
        }
    })
    .await;
    Rig { g, sims }
}

#[tokio::test]
async fn discovers_devices_and_reads_status() {
    let r = rig(1, 41000, 3).await;
    let devices = r.g.devices().await;
    assert!(devices.iter().any(|d| d.sku == "H6076" && d.name == "Floor Lamp"));
    assert_eq!(devices.iter().filter(|d| d.name == "Light Bars").count(), 2);
    assert!(devices.iter().all(|d| d.state.unwrap().brightness == 100));
}

#[tokio::test]
async fn commands_reach_the_device() {
    let r = rig(2, 41010, 1).await;
    let id = r.sims[0].config.id.clone();
    let t = Target::Device(id.clone());

    r.g.apply(&t, Action::Power(true)).await.unwrap();
    r.g.apply(&t, Action::Brightness(37)).await.unwrap();
    r.g.apply(&t, Action::Color(Rgb::new(255, 0, 128))).await.unwrap();

    // Optimistic state is immediate.
    let s = r.g.device(&id).await.unwrap().state.unwrap();
    assert!(s.on);
    assert_eq!(s.brightness, 37);
    assert_eq!(s.color, Rgb::new(255, 0, 128));

    let sim = &r.sims[0];
    eventually("device applied commands", || async {
        let s = sim.state();
        s.on && s.brightness == 37 && s.color == Rgb::new(255, 0, 128)
    })
    .await;

    r.g.apply(&t, Action::Kelvin(3000)).await.unwrap();
    eventually("kelvin applied", || async { sim.state().kelvin == 3000 }).await;
    assert!(sim.received().contains(&Command::ColorTemp(3000)));
}

#[tokio::test]
async fn groups_and_toggle() {
    let r = rig(3, 41020, 3).await;
    let a = r.sims[0].config.id.clone();
    let b = r.sims[1].config.id.clone();
    let group = r.g.create_group("Desk", vec![a.clone(), b.clone()]).await.unwrap();
    assert_eq!(r.g.groups().await.len(), 1);

    // All off initially -> toggle turns the group on, leaves the third alone.
    r.g.apply(&Target::Group(group.id.clone()), Action::Toggle).await.unwrap();
    eventually("group on", || async { r.sims[0].state().on && r.sims[1].state().on }).await;
    assert!(!r.sims[2].state().on);

    // Some on -> toggle turns all off.
    r.g.apply(&Target::Group(group.id.clone()), Action::Toggle).await.unwrap();
    eventually("group off", || async { !r.sims[0].state().on && !r.sims[1].state().on }).await;

    r.g.apply(&Target::All, Action::Power(true)).await.unwrap();
    eventually("all on", || async { r.sims.iter().all(|s| s.state().on) }).await;

    r.g.delete_group(&group.id).await.unwrap();
    assert!(r.g.groups().await.is_empty());
}

#[tokio::test]
async fn scenes_capture_and_restore() {
    let r = rig(4, 41030, 2).await;
    let a = r.sims[0].config.id.clone();
    let b = r.sims[1].config.id.clone();
    r.g.apply(&Target::Device(a.clone()), Action::Power(true)).await.unwrap();
    r.g.apply(&Target::Device(a.clone()), Action::Color(Rgb::new(10, 20, 30))).await.unwrap();
    r.g.apply(&Target::Device(b.clone()), Action::Power(true)).await.unwrap();
    r.g.apply(&Target::Device(b.clone()), Action::Kelvin(2700)).await.unwrap();

    let scene = r.g.save_scene("Cozy", None).await.unwrap();
    assert_eq!(scene.entries.len(), 2);

    r.g.apply(&Target::All, Action::Power(false)).await.unwrap();
    r.g.apply(&Target::All, Action::Color(Rgb::new(255, 255, 255))).await.unwrap();
    eventually("reset", || async { !r.sims[0].state().on && r.sims[1].state().kelvin == 0 }).await;

    r.g.apply_scene("cozy").await.unwrap();
    eventually("scene restored", || async {
        let (sa, sb) = (r.sims[0].state(), r.sims[1].state());
        sa.on && sa.color == Rgb::new(10, 20, 30) && sb.on && sb.kelvin == 2700
    })
    .await;
}

#[tokio::test]
async fn resolves_human_queries() {
    let r = rig(5, 41040, 2).await;
    let lamp = r.sims[0].config.id.clone();
    assert_eq!(r.g.resolve("all").await.unwrap(), Target::All);
    assert_eq!(r.g.resolve("floor lamp").await.unwrap(), Target::Device(lamp.clone()));
    assert_eq!(r.g.resolve(&sim_ip(5, 0).to_string()).await.unwrap(), Target::Device(lamp.clone()));
    r.g.rename_device(&lamp, "Big Lamp").await.unwrap();
    assert_eq!(r.g.resolve("big lamp").await.unwrap(), Target::Device(lamp.clone()));
    let g = r.g.create_group("Living Room", vec![lamp]).await.unwrap();
    assert_eq!(r.g.resolve("living room").await.unwrap(), Target::Group(g.id));
    assert!(r.g.resolve("nope").await.is_err());
}

#[tokio::test]
async fn persists_config_and_known_devices() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("groovie.json");
    let (net, base) = (6, 41050);
    let lamp;
    {
        let sims = spawn_sims(net, base, 1).await;
        lamp = sims[0].config.id.clone();
        let g = Groovie::start(config(net, base, 1, Some(path.clone()))).await.unwrap();
        let g2 = g.clone();
        eventually("discovered", || {
            let g = g2.clone();
            async move { g.devices().await.len() == 1 }
        })
        .await;
        g.rename_device(&lamp, "Corner").await.unwrap();
        g.create_group("Room", vec![lamp.clone()]).await.unwrap();
    }
    // Let the dropped engine release its socket.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Devices are gone, but we remember them (offline) and our config.
    let g = Groovie::start(config(net, base, 1, Some(path))).await.unwrap();
    let devices = g.devices().await;
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Corner");
    assert!(!devices[0].online);
    assert_eq!(g.groups().await[0].name, "Room");
}

#[tokio::test]
async fn add_device_by_ip_without_discovery() {
    let (net, base) = (7, 41060);
    let sims = spawn_sims(net, base, 1).await;
    let mut cfg = config(net, base, 1, None);
    cfg.scan_targets.clear();
    let g = Groovie::start(cfg).await.unwrap();
    assert!(g.devices().await.is_empty());
    g.add_device_by_ip(sim_ip(net, 0)).await.unwrap();
    let g2 = g.clone();
    eventually("manually added", || {
        let g = g2.clone();
        async move { g.devices().await.first().map(|d| d.state.is_some()).unwrap_or(false) }
    })
    .await;
    assert_eq!(g.devices().await[0].id, sims[0].config.id);
}

#[tokio::test]
async fn second_instance_on_same_port_fails_cleanly() {
    let mut cfg = config(8, 41070, 0, None);
    cfg.scan_targets.clear();
    let _first = Groovie::start(cfg.clone()).await.unwrap();
    let err = Groovie::start(cfg).await.err().expect("second bind on same UDP port must fail");
    assert!(err.to_string().contains("41072"), "unexpected error: {err}");
}

#[tokio::test]
async fn records_network_and_state_logs() {
    let r = rig(9, 41080, 2).await;
    let devices = r.g.devices().await;
    assert_eq!(devices[0].firmware.as_deref(), Some("wifi 1.02.03 · ble 1.03.01"));

    let lamp_id = r.sims[0].config.id.clone();
    let bars_id = r.sims[1].config.id.clone();
    let lamp_ip = r.sims[0].config.ip.to_string();
    let bars_ip = r.sims[1].config.ip.to_string();

    r.g.apply(&Target::Device(lamp_id.clone()), Action::Power(true)).await.unwrap();
    r.g.apply(&Target::Device(bars_id.clone()), Action::Power(true)).await.unwrap();
    eventually("both on", || async { r.sims[0].state().on && r.sims[1].state().on }).await;

    r.g.clear_logs().await;
    assert!(r.g.logs().await.is_empty());

    // Turn off only the light bars: no command may go to the floor lamp.
    r.g.apply(&Target::Device(bars_id), Action::Power(false)).await.unwrap();
    eventually("bars off", || async { !r.sims[1].state().on }).await;
    assert!(r.sims[0].state().on);

    let logs = r.g.logs().await;
    assert!(logs.iter().any(|l| l.dir == "action" && l.message.contains(&bars_ip) && !l.message.contains(&lamp_ip)));
    assert!(logs.iter().any(|l| l.dir == "tx" && l.addr.starts_with(&bars_ip) && l.message.contains("\"turn\"")));
    assert!(!logs.iter().any(|l| l.dir == "tx" && l.addr.starts_with(&lamp_ip)));

    // Simulate an external controller (e.g. DreamView / mobile app) toggling the lamp.
    r.sims[0].state.lock().unwrap().on = false;
    r.g.refresh_all().await;
    let g2 = r.g.clone();
    eventually("external state change logged", || {
        let g = g2.clone();
        async move {
            g.logs().await.iter().any(|l| l.dir == "state" && l.message.contains("external/reported change"))
        }
    })
    .await;
}


