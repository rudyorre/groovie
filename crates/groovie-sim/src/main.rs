//! `groovie-sim`: run fake Govee lights locally.
//!
//! ```text
//! # Three loopback devices (Linux; macOS needs `ifconfig lo0 alias 127.0.0.x`):
//! groovie-sim --count 3
//! GROOVIE_SCAN_TARGETS=127.0.0.2:4001,127.0.0.3:4001,127.0.0.4:4001 grooviectl list
//!
//! # One device that answers real multicast discovery (on a spare machine):
//! groovie-sim --multicast --ip 192.168.1.50
//! ```

use clap::Parser;
use groovie_sim::{SimConfig, SimDevice};
use std::net::Ipv4Addr;

#[derive(Parser)]
#[command(version, about = "Fake Govee LAN devices for testing Groovie")]
struct Args {
    /// First device IP; subsequent devices increment the last octet.
    #[arg(long, default_value = "127.0.0.2")]
    ip: Ipv4Addr,
    /// Number of devices.
    #[arg(long, default_value_t = 3)]
    count: u8,
    /// Comma-separated SKUs to cycle through.
    #[arg(long, default_value = "H6072,H6056,H6056")]
    skus: String,
    #[arg(long, default_value_t = 4001)]
    scan_port: u16,
    #[arg(long, default_value_t = 4003)]
    command_port: u16,
    #[arg(long, default_value_t = 4002)]
    reply_port: u16,
    /// Join the Govee multicast group (only one device per host).
    #[arg(long)]
    multicast: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args = Args::parse();
    let skus: Vec<&str> = args.skus.split(',').map(str::trim).collect();
    let count = if args.multicast { 1 } else { args.count };
    let mut devices = vec![];
    let mut targets = vec![];
    for i in 0..count {
        let o = args.ip.octets();
        let ip = Ipv4Addr::new(o[0], o[1], o[2], o[3].wrapping_add(i));
        let mut cfg = SimConfig::new(ip, skus[i as usize % skus.len()]);
        cfg.scan_port = args.scan_port;
        cfg.command_port = args.command_port;
        cfg.reply_port = args.reply_port;
        cfg.multicast = args.multicast;
        let d = SimDevice::spawn(cfg.clone()).await?;
        println!("sim device {} ({}) at {}", cfg.id, cfg.sku, ip);
        targets.push(format!("{ip}:{}", args.scan_port));
        devices.push(d);
    }
    if !args.multicast {
        println!("\nPoint Groovie at them with:\n  GROOVIE_SCAN_TARGETS={}", targets.join(","));
    }
    tokio::signal::ctrl_c().await?;
    Ok(())
}
