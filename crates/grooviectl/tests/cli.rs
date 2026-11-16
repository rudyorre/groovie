//! End-to-end binary tests for `grooviectl`: verifies loopback IPC coexistence
//! alongside a running daemon, `<100ms` cached UDP fast-path when the daemon is
//! closed, and `grooviectl sync` subcommands.
#![cfg(target_os = "linux")]

use groovie_core::ipc::IpcServer;
use groovie_core::{Config, Groovie, Rgb};
use groovie_sim::{SimConfig, SimDevice};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_grooviectl");

fn run_cli(
    args: &[&str],
    store: &Path,
    ipc: Option<&Path>,
    net: u8,
    base: u16,
) -> (bool, String, String, Duration) {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .env("GROOVIE_CONFIG", store)
        .env("GROOVIE_LISTEN", format!("127.0.{net}.1:{}", base + 2))
        .env("GROOVIE_SCAN_TARGETS", format!("127.0.{net}.10:{base}"))
        .env("GROOVIE_SCAN_PORT", base.to_string())
        .env("GROOVIE_COMMAND_PORT", (base + 1).to_string());
    match ipc {
        Some(p) => {
            cmd.env("GROOVIE_IPC_FILE", p);
        }
        None => {
            cmd.env_remove("GROOVIE_IPC_FILE");
        }
    }
    let t0 = Instant::now();
    let out = cmd.output().expect("spawn grooviectl");
    let elapsed = t0.elapsed();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        elapsed,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_works_alongside_daemon_and_fast_path_when_daemon_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store_path = dir.path().join("groovie.json");
    let ipc_path = dir.path().join("daemon.json");
    let (net, base) = (31, 42100);

    let mut sc = SimConfig::new(Ipv4Addr::new(127, 0, net, 10), "H6056");
    sc.scan_port = base;
    sc.command_port = base + 1;
    sc.reply_port = base + 2;
    let sim = SimDevice::spawn(sc).await.unwrap();

    let cfg = Config {
        listen: SocketAddr::from((Ipv4Addr::new(127, 0, net, 1), base + 2)),
        scan_targets: vec![SocketAddr::from((Ipv4Addr::new(127, 0, net, 10), base))],
        scan_port: base,
        command_port: base + 1,
        store_path: Some(store_path.clone()),
        poll_interval: Duration::from_secs(60),
        rescan_interval: Duration::from_secs(60),
        refresh_delay: Duration::from_millis(100),
        ..Config::default()
    };

    let daemon = Groovie::start(cfg).await.unwrap();
    for _ in 0..80 {
        if daemon.devices().await.first().and_then(|d| d.state).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let ipc = IpcServer::start(daemon.clone(), ipc_path.clone(), None).await.unwrap();

    // 1. With the daemon holding UDP port `base + 2`, `grooviectl` succeeds instantaneously over IPC!
    let (ok, stdout, stderr, _) = run_cli(&["list", "--json"], &store_path, Some(&ipc_path), net, base);
    assert!(ok, "stderr: {stderr}");
    assert!(stdout.contains("H6056"));

    let (ok, _, stderr, _) = run_cli(&["rename", "Light Bars", "Desk Bars"], &store_path, Some(&ipc_path), net, base);
    assert!(ok, "stderr: {stderr}");

    let (ok, _, stderr, _) = run_cli(&["on", "Desk Bars"], &store_path, Some(&ipc_path), net, base);
    assert!(ok, "stderr: {stderr}");
    for _ in 0..50 {
        if sim.state().on {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    assert!(sim.state().on);

    // 2. `grooviectl scene` save/rename/update + `sync` over IPC!
    let (ok, stdout, stderr, _) = run_cli(
        &["scene", "save", "WarmDesk", "Desk Bars"],
        &store_path,
        Some(&ipc_path),
        net,
        base,
    );
    assert!(ok, "stderr: {stderr}");
    assert!(stdout.contains("Saved scene 'WarmDesk'"), "stdout: {stdout}");

    let (ok, stdout, stderr, _) = run_cli(
        &["scene", "rename", "WarmDesk", "NightDesk"],
        &store_path,
        Some(&ipc_path),
        net,
        base,
    );
    assert!(ok, "stderr: {stderr}");
    assert!(stdout.contains("Renamed scene to 'NightDesk'"), "stdout: {stdout}");

    let (ok, stdout, stderr, _) = run_cli(
        &["scene", "update", "NightDesk"],
        &store_path,
        Some(&ipc_path),
        net,
        base,
    );
    assert!(ok, "stderr: {stderr}");
    assert!(stdout.contains("Updated scene 'NightDesk'"), "stdout: {stdout}");

    let (ok, stdout, stderr, _) = run_cli(&["sync", "start"], &store_path, Some(&ipc_path), net, base);
    assert!(ok, "stderr: {stderr}");
    assert!(stdout.contains("Screen Sync: active"), "stdout: {stdout}");

    let (ok, stdout, stderr, _) = run_cli(&["sync", "toggle"], &store_path, Some(&ipc_path), net, base);
    assert!(ok, "stderr: {stderr}");
    assert!(stdout.contains("Screen Sync: off"), "stdout: {stdout}");

    // 3. Shut down the daemon; now test the offline ephemeral UDP fast-path (`--wait 1500` default,
    //    yet cached commands finish well under 350ms without waiting 1500ms!).
    drop(ipc);
    drop(daemon);
    tokio::time::sleep(Duration::from_millis(30)).await;

    let (ok, _, stderr, elapsed) = run_cli(
        &["color", "Desk Bars", "#ff4400"],
        &store_path,
        None,
        net,
        base,
    );
    assert!(ok, "stderr: {stderr}");
    assert!(
        elapsed < Duration::from_millis(500),
        "expected fast-path well below 1500ms default wait, got {elapsed:?}"
    );
    for _ in 0..50 {
        if sim.state().color == Rgb::new(255, 68, 0) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    assert_eq!(sim.state().color, Rgb::new(255, 68, 0));

    // Offline `scene rename` succeeds immediately via store edit, and `scene update` works standalone.
    let (ok, stdout, stderr, elapsed) = run_cli(
        &["scene", "rename", "NightDesk", "EmberDesk"],
        &store_path,
        None,
        net,
        base,
    );
    assert!(ok, "stderr: {stderr}");
    assert!(stdout.contains("Renamed scene to 'EmberDesk'"), "stdout: {stdout}");
    assert!(elapsed < Duration::from_millis(500));

    let (ok, stdout, stderr, _) = run_cli(
        &["scene", "update", "EmberDesk"],
        &store_path,
        None,
        net,
        base,
    );
    assert!(ok, "stderr: {stderr}");
    assert!(stdout.contains("Updated scene 'EmberDesk'"), "stdout: {stdout}");

    // Offline `sync start` cleanly reports that the desktop app is required.
    let (ok, _, stderr, _) = run_cli(&["sync", "start"], &store_path, None, net, base);
    assert!(!ok);
    assert!(stderr.contains("requires the Groovie desktop app"), "stderr: {stderr}");

    // Create a group containing Desk Bars, then `grooviectl forget "Desk Bars"` offline (<500ms)
    // and verify known_devices, nicknames, groups, and scenes are pruned.
    let (ok, _, stderr, _) = run_cli(
        &["group", "create", "Workstation", "Desk Bars"],
        &store_path,
        None,
        net,
        base,
    );
    assert!(ok, "stderr: {stderr}");

    let (ok, stdout, stderr, elapsed) = run_cli(&["forget", "Desk Bars"], &store_path, None, net, base);
    assert!(ok, "stderr: {stderr}");
    assert!(stdout.contains("Forgot 'Desk Bars'"), "stdout: {stdout}");
    assert!(elapsed < Duration::from_millis(500), "forget took {elapsed:?}");

    let (ok, stdout, stderr, _) = run_cli(&["--json", "config"], &store_path, None, net, base);
    assert!(ok, "stderr: {stderr}");
    assert!(!stdout.contains(&sim.config.id), "forgotten id still in config: {stdout}");
}

