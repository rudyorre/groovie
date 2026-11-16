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
        subnet_scan: false,
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

    // Wait past the command settle guard, then simulate an external controller toggling the lamp.
    tokio::time::sleep(Duration::from_millis(90)).await;
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

#[tokio::test]
async fn ignores_mid_fade_status_poll_after_command() {
    let (net, base) = (10, 41090);
    let sims = spawn_sims(net, base, 1).await;
    let mut cfg = config(net, base, 1, None);
    cfg.refresh_delay = Duration::from_millis(200);
    let g = Groovie::start(cfg).await.unwrap();
    let g2 = g.clone();
    eventually("discovered", || {
        let g = g2.clone();
        async move { g.devices().await.first().and_then(|d| d.state).is_some() }
    })
    .await;

    let id = sims[0].config.id.clone();
    g.apply(&Target::Device(id.clone()), Action::Power(true)).await.unwrap();
    eventually("sim received turn(true)", || async { sims[0].state().on }).await;

    // Simulate slow-fading hardware (like H6072) still reporting `onOff: 0` 30ms after `turn(1)`.
    sims[0].state.lock().unwrap().on = false;
    g.refresh_all().await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // Optimistic `on: true` must not be reverted by the mid-fade poll.
    assert!(g.device(&id).await.unwrap().state.unwrap().on);

    // Once the hardware finishes fading before `refresh_delay` (200ms) fires, state stays `on: true`.
    sims[0].state.lock().unwrap().on = true;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(g.device(&id).await.unwrap().state.unwrap().on);
}

#[tokio::test]
async fn syncs_external_changes_and_adapts_poll_interval() {
    // Start with a 60s poll interval (like idle tray mode), then switch to fast active polling.
    let r = rig(11, 41100, 1).await;
    let id = r.sims[0].config.id.clone();

    // Simulate toggling on + setting 4000K at 55% from the Govee mobile app.
    {
        let mut st = r.sims[0].state.lock().unwrap();
        st.on = true;
        st.brightness = 55;
        st.kelvin = 4000;
    }

    assert!(r.g.set_poll_interval(Duration::from_millis(60)));
    assert!(!r.g.set_poll_interval(Duration::from_millis(60)));

    let g2 = r.g.clone();
    let id2 = id.clone();
    eventually("external Govee app change synced via active poll", || {
        let g = g2.clone();
        let id = id2.clone();
        async move {
            g.device(&id)
                .await
                .and_then(|d| d.state)
                .map(|s| s.on && s.brightness == 55 && s.kelvin == 4000)
                .unwrap_or(false)
        }
    })
    .await;

    // Switch back to quiet tray polling, wait past the 400ms debounce, and test `sync_now()` on focus.
    assert!(r.g.set_poll_interval(Duration::from_secs(60)));
    tokio::time::sleep(Duration::from_millis(430)).await;
    r.sims[0].state.lock().unwrap().on = false;
    r.g.sync_now().await;

    let g3 = r.g.clone();
    eventually("focus sync_now picked up external off", || {
        let g = g3.clone();
        let id = id.clone();
        async move { g.device(&id).await.and_then(|d| d.state).map(|s| !s.on).unwrap_or(false) }
    })
    .await;
}

#[tokio::test]
async fn reloads_external_dotfiles_edits_and_switches_path() {
    let dir = tempfile::tempdir().unwrap();
    let initial_path = dir.path().join("app/groovie.json");
    let dotfiles_path = dir.path().join("dotfiles/groovie.json");

    let (net, base) = (12, 41110);
    let sims = spawn_sims(net, base, 1).await;
    let id = sims[0].config.id.clone();
    let g = Groovie::start(config(net, base, 1, Some(initial_path))).await.unwrap();
    let g2 = g.clone();
    eventually("discovered", || {
        let g = g2.clone();
        async move { g.devices().await.len() == 1 }
    })
    .await;

    g.rename_device(&id, "Desk Lamp").await.unwrap();
    g.create_group("Office", vec![id.clone()]).await.unwrap();

    // Pointing at a brand-new dotfiles path copies current config there.
    let info = g.switch_store_path(dotfiles_path.clone()).await.unwrap();
    assert_eq!(info.path.as_deref(), dotfiles_path.to_str());
    assert!(dotfiles_path.is_file());
    assert!(info.json.contains("Desk Lamp"));

    // Simulate `git pull` / editor modifying `dotfiles/groovie.json` externally.
    tokio::time::sleep(Duration::from_millis(20)).await;
    let edited = info.json.replace("Desk Lamp", "Studio Lamp").replace("Office", "Studio");
    std::fs::write(&dotfiles_path, edited).unwrap();

    // Focusing the window (`sync_now`) picks up the dotfile edit immediately.
    g.sync_now().await;
    assert_eq!(g.device(&id).await.unwrap().name, "Studio Lamp");
    assert_eq!(g.groups().await[0].name, "Studio");
}

#[tokio::test]
async fn remembers_last_kelvin_and_color_across_mode_switches() {
    let r = rig(13, 41120, 1).await;
    let id = r.sims[0].config.id.clone();
    let t = Target::Device(id.clone());
    let indigo = Rgb::new(94, 92, 230);

    r.g.apply(&t, Action::Power(true)).await.unwrap();
    r.g.apply(&t, Action::Color(indigo)).await.unwrap();
    r.g.apply(&t, Action::Kelvin(2700)).await.unwrap();

    // Wait past `refresh_delay` (100ms) so the simulated Govee device sends back
    // `devStatus` with `color: (255, 255, 255), colorTemInKelvin: 2700`.
    tokio::time::sleep(Duration::from_millis(160)).await;
    let dev = r.g.device(&id).await.unwrap();
    assert_eq!(dev.state.unwrap().kelvin, 2700);
    assert_eq!(dev.state.unwrap().color, Rgb::new(255, 255, 255));
    assert_eq!(dev.last_kelvin, Some(2700));
    assert_eq!(dev.last_color, Some(indigo), "Kelvin devStatus must not overwrite remembered RGB colour");

    // Switch back to Colour, wait for `devStatus` (`colorTemInKelvin: 0`), and verify
    // `last_kelvin` still remembers 2700K for when the user returns to White.
    r.g.apply(&t, Action::Color(indigo)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(160)).await;
    let dev = r.g.device(&id).await.unwrap();
    assert_eq!(dev.state.unwrap().kelvin, 0);
    assert_eq!(dev.state.unwrap().color, indigo);
    assert_eq!(dev.last_color, Some(indigo));
    assert_eq!(dev.last_kelvin, Some(2700), "Colour mode must not clear remembered Kelvin");
}

#[tokio::test]
async fn dropping_engine_releases_udp_port_immediately() {
    let mut cfg = config(14, 41130, 0, None);
    cfg.scan_targets.clear();

    let first = Groovie::start(cfg.clone()).await.unwrap();
    assert!(Groovie::start(cfg.clone()).await.is_err());

    drop(first);
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(25)).await;

    let _restarted = Groovie::start(cfg).await.expect("dropped engine must release UDP port for relaunch");
}

async fn spawn_custom_sims(net: u8, base: u16, skus: &[&str]) -> Rig {
    let mut sims = Vec::with_capacity(skus.len());
    for (i, sku) in skus.iter().enumerate() {
        let mut c = SimConfig::new(sim_ip(net, i as u8), sku);
        c.scan_port = base;
        c.command_port = base + 1;
        c.reply_port = base + 2;
        sims.push(SimDevice::spawn(c).await.unwrap());
    }
    let g = Groovie::start(config(net, base, skus.len() as u8, None)).await.unwrap();
    let g2 = g.clone();
    let expected = skus.len();
    eventually("custom sims discovered with state", || {
        let g = g2.clone();
        async move {
            let d = g.devices().await;
            d.len() == expected && d.iter().all(|d| d.online && d.state.is_some())
        }
    })
    .await;
    Rig { g, sims }
}

#[tokio::test]
async fn screen_sync_streams_razer_and_restores_snapshot_on_stop() {
    use groovie_core::{SyncFrame, SYNC_GRID_H, SYNC_GRID_W};

    let r = spawn_custom_sims(15, 41140, &["H6056", "H6072", "H6076"]).await;
    let bars_id = r.sims[0].config.id.clone();
    let lamp_id = r.sims[1].config.id.clone();
    let solid_id = r.sims[2].config.id.clone();

    // H6056 starts OFF; H6072 starts ON at 55% 2700K White; H6076 starts ON at 70% warm amber RGB.
    r.g.apply(&Target::Device(bars_id.clone()), Action::Power(false)).await.unwrap();
    r.g.apply(&Target::Device(lamp_id.clone()), Action::Power(true)).await.unwrap();
    r.g.apply(&Target::Device(lamp_id.clone()), Action::Brightness(55)).await.unwrap();
    r.g.apply(&Target::Device(lamp_id.clone()), Action::Kelvin(2700)).await.unwrap();
    r.g.apply(&Target::Device(solid_id.clone()), Action::Power(true)).await.unwrap();
    r.g.apply(&Target::Device(solid_id.clone()), Action::Brightness(70)).await.unwrap();
    r.g.apply(&Target::Device(solid_id.clone()), Action::Color(Rgb::new(200, 100, 40))).await.unwrap();

    eventually("initial pre-sync states applied", || async {
        !r.sims[0].state().on
            && r.sims[1].state().on
            && r.sims[1].state().kelvin == 2700
            && r.sims[1].state().brightness == 55
            && r.sims[2].state().on
            && r.sims[2].state().color == Rgb::new(200, 100, 40)
    })
    .await;

    // Seed placements and set smoothing=0 so the first frame hits full target colour immediately.
    let mut sync_cfg = r.g.sync_status().await.config;
    assert_eq!(sync_cfg.placements.len(), 4, "H6056 splits into Bar A + Bar B plus H6072 + H6076");
    sync_cfg.smoothing = 0;
    r.g.update_sync_config(sync_cfg).await.unwrap();

    // Frame: left half bright red, right half bright blue.
    let mut px = vec![Rgb::new(0, 0, 0); SYNC_GRID_W * SYNC_GRID_H];
    for y in 0..SYNC_GRID_H {
        for x in 0..SYNC_GRID_W {
            px[y * SYNC_GRID_W + x] = if x < SYNC_GRID_W / 2 {
                Rgb::new(255, 16, 16)
            } else {
                Rgb::new(16, 40, 255)
            };
        }
    }
    r.g.push_sync_frame(SyncFrame::new(SYNC_GRID_W, SYNC_GRID_H, 16.0 / 9.0, px).unwrap()).await;

    let st = r.g.start_sync().await.unwrap();
    assert!(st.active);
    assert_eq!(st.active_devices.len(), 3);

    eventually("Razer and solid sync frames streamed", || async {
        let bars_ok = r.sims[0].state().on
            && r.sims[0].is_razer_enabled()
            && r.sims[0]
                .last_razer_frame()
                .map(|f| f.len() == 12 && f[0].r > 200 && f[6].b > 200)
                .unwrap_or(false);
        let lamp_ok = r.sims[1].state().on
            && r.sims[1].is_razer_enabled()
            && r.sims[1]
                .last_razer_frame()
                .map(|f| f.len() == 15 && f[0].b > 200)
                .unwrap_or(false);
        let solid_ok = !r.sims[2].is_razer_enabled() && r.sims[2].state().color.r > 180;
        bars_ok && lamp_ok && solid_ok
    })
    .await;

    // Live per-light brightness adjustment works without dropping Razer mode.
    r.g.apply(&Target::Device(lamp_id.clone()), Action::Brightness(35)).await.unwrap();
    eventually("live dimmer applied during sync", || async {
        r.sims[1].state().brightness == 35 && r.sims[1].is_razer_enabled()
    })
    .await;

    // Stopping Screen Sync exits Razer mode and restores each light's pre-sync snapshot.
    let stopped = r.g.stop_sync(Some("done")).await;
    assert!(!stopped.active);

    eventually("pre-sync snapshots restored", || async {
        let s0 = r.sims[0].state();
        let s1 = r.sims[1].state();
        let s2 = r.sims[2].state();
        !r.sims[0].is_razer_enabled()
            && !s0.on
            && !r.sims[1].is_razer_enabled()
            && s1.on
            && s1.brightness == 55
            && s1.kelvin == 2700
            && s2.on
            && s2.brightness == 70
            && s2.color == Rgb::new(200, 100, 40)
    })
    .await;
}

#[tokio::test]
async fn screen_sync_detaches_light_on_power_off_and_stops_when_empty() {
    use groovie_core::{SyncFrame, SYNC_GRID_H, SYNC_GRID_W};

    let r = spawn_custom_sims(16, 41150, &["H6056", "H6072"]).await;
    let bars_id = r.sims[0].config.id.clone();
    let lamp_id = r.sims[1].config.id.clone();

    r.g.push_sync_frame(SyncFrame::solid(SYNC_GRID_W, SYNC_GRID_H, Rgb::new(30, 200, 120))).await;
    r.g.start_sync().await.unwrap();

    eventually("both devices syncing", || async {
        r.sims[0].is_razer_enabled() && r.sims[1].is_razer_enabled()
    })
    .await;

    // Powering off the floor lamp detaches just that lamp; the bars keep syncing.
    r.g.apply(&Target::Device(lamp_id), Action::Power(false)).await.unwrap();
    eventually("lamp detached and off", || async {
        !r.sims[1].is_razer_enabled() && !r.sims[1].state().on
    })
    .await;
    let mid = r.g.sync_status().await;
    assert!(mid.active);
    assert_eq!(mid.active_devices, vec![bars_id.clone()]);
    assert!(r.sims[0].is_razer_enabled());

    // Powering off the last syncing light automatically ends Screen Sync.
    r.g.apply(&Target::Device(bars_id), Action::Power(false)).await.unwrap();
    eventually("bars off and sync auto-stopped", || async {
        !r.sims[0].is_razer_enabled() && !r.sims[0].state().on
    })
    .await;
    let end = r.g.sync_status().await;
    assert!(!end.active);
    assert!(end.active_devices.is_empty());
}

#[tokio::test]
async fn screen_sync_identify_sweeps_and_restores_when_idle() {
    let r = spawn_custom_sims(17, 41160, &["H6056"]).await;
    let bars_id = r.sims[0].config.id.clone();

    r.g.apply(&Target::Device(bars_id.clone()), Action::Power(true)).await.unwrap();
    r.g.apply(&Target::Device(bars_id.clone()), Action::Brightness(65)).await.unwrap();
    r.g.apply(&Target::Device(bars_id.clone()), Action::Kelvin(3200)).await.unwrap();
    eventually("bars in 3200K white", || async {
        r.sims[0].state().on && r.sims[0].state().kelvin == 3200
    })
    .await;

    // Identify Bar A (`<id>#A`): slots 0..6 sweep warm amber while Bar B (slots 6..12) stays dark.
    r.g.identify_sync_placement(&format!("{bars_id}#A")).await.unwrap();
    eventually("identify frame received on Bar A only", || async {
        r.sims[0]
            .last_razer_frame()
            .map(|f| {
                f.len() == 12
                    && f[..6].iter().any(|c| c.r > 120)
                    && f[6..].iter().all(|c| *c == Rgb::new(0, 0, 0))
            })
            .unwrap_or(false)
    })
    .await;

    // Wait for the ~850ms identify sweep to finish and restore 3200K @ 65%.
    tokio::time::sleep(Duration::from_millis(820)).await;
    eventually("bars restored after identify sweep", || async {
        let s = r.sims[0].state();
        !r.sims[0].is_razer_enabled() && s.on && s.brightness == 65 && s.kelvin == 3200
    })
    .await;
}

#[tokio::test]
async fn save_config_json_imports_and_probes_devices_without_multicast() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Documents/groovie.json");
    let (net, base) = (18, 41170);
    let sims = spawn_sims(net, base, 2).await;
    let id_a = sims[0].config.id.clone();
    let id_b = sims[1].config.id.clone();
    let ip_a = sims[0].config.ip;
    let ip_b = sims[1].config.ip;

    // Start with zero scan_targets (simulating iOS without multicast).
    let mut cfg = config(net, base, 0, Some(path.clone()));
    cfg.scan_targets.clear();
    let g = Groovie::start(cfg).await.unwrap();
    assert!(g.devices().await.is_empty());

    // First manually discover light A so it is online in memory.
    g.add_device_by_ip(ip_a).await.unwrap();
    let g_wait = g.clone();
    eventually("light A online", || {
        let g = g_wait.clone();
        async move { g.devices().await.iter().any(|d| d.online && d.state.is_some()) }
    })
    .await;

    // Reject invalid JSON cleanly without dropping light A.
    assert!(g.save_config_json("{ broken").await.is_err());
    assert_eq!(g.devices().await.len(), 1);

    // Paste desktop groovie.json containing nicknames, groups, scenes, and light B
    // (while omitting light A from known_devices to verify live devices are preserved).
    let pasted = serde_json::json!({
        "version": 1,
        "nicknames": {
            id_a.clone(): "Bedside",
            id_b.clone(): "TV Bars",
        },
        "groups": [
            { "id": "g_bed", "name": "Bedroom", "members": [id_a.clone(), id_b.clone()] }
        ],
        "scenes": [
            {
                "id": "s_night",
                "name": "Night",
                "entries": [
                    { "device": id_b.clone(), "on": true, "brightness": 25, "kelvin": 2700 }
                ]
            }
        ],
        "known_devices": [
            { "id": id_b.clone(), "ip": ip_b.to_string(), "sku": "H6046" }
        ]
    })
    .to_string();

    let info = g.save_config_json(&pasted).await.unwrap();
    assert!(path.is_file());
    assert!(info.json.contains("Bedside"));
    assert!(info.json.contains("TV Bars"));
    assert_eq!(g.groups().await.len(), 1);
    assert_eq!(g.scenes().await.len(), 1);

    // Newly imported light B is probed automatically via unicast and comes online!
    let g_probe = g.clone();
    let id_b2 = id_b.clone();
    eventually("imported light B came online automatically", || {
        let g = g_probe.clone();
        let id_b = id_b2.clone();
        async move {
            g.device(&id_b)
                .await
                .map(|d| d.online && d.name == "TV Bars" && d.state.is_some())
                .unwrap_or(false)
        }
    })
    .await;
    assert_eq!(g.device(&id_a).await.unwrap().name, "Bedside");
}

#[tokio::test]
async fn resume_recovers_discovery_when_lights_appear_after_startup() {
    let (net, base) = (19, 41180);
    // Start the engine BEFORE the simulated light exists (simulating the iOS
    // Local Network permission dialog dropping the initial startup scan).
    let cfg = config(net, base, 1, None);
    let g = Groovie::start(cfg).await.unwrap();
    assert!(g.devices().await.is_empty());

    // Now the user grants permission / the light becomes reachable on the LAN,
    // and the app foregrounds (`resume()`).
    let _sims = spawn_sims(net, base, 1).await;
    g.resume().await;

    let g2 = g.clone();
    eventually("resume() discovered light and fetched status", || {
        let g = g2.clone();
        async move {
            let d = g.devices().await;
            d.len() == 1 && d[0].online && d[0].state.is_some()
        }
    })
    .await;
}
