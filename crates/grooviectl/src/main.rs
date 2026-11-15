//! `grooviectl`: control Govee lights from the terminal.
//!
//! Every command starts the engine, waits briefly for discovery and status
//! replies, does its thing and exits. Point it at the simulator with the
//! `GROOVIE_*` environment variables (see `groovie_core::Config`).

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use groovie_core::{Action, Config, Device, Groovie, Rgb, Target};
use std::net::Ipv4Addr;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "grooviectl", version, about = "Control Govee lights over your local network")]
struct Cli {
    /// How long to wait for lights to answer before acting (ms).
    #[arg(long, global = true, default_value_t = 1500)]
    wait: u64,
    /// Print JSON instead of a table (for `list`, `groups`, `scenes`).
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
    /// Add a light by IP when discovery can't find it.
    AddIp { ip: Ipv4Addr },
    /// Manage groups.
    #[command(subcommand)]
    Group(GroupCmd),
    /// Manage saved scenes.
    #[command(subcommand)]
    Scene(SceneCmd),
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
    let g = Groovie::start(Config::default().with_env_overrides())
        .await
        .context("could not start (is UDP port 4002 free?)")?;
    tokio::time::sleep(Duration::from_millis(cli.wait)).await;

    match cli.cmd {
        Cmd::List => {
            let devices = g.devices().await;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&devices)?);
            } else if devices.is_empty() {
                println!("No lights found. Is LAN Control enabled in the Govee app?");
            } else {
                print_devices(&devices);
            }
            return Ok(());
        }
        Cmd::On { target } => act(&g, &target, Action::Power(true)).await?,
        Cmd::Off { target } => act(&g, &target, Action::Power(false)).await?,
        Cmd::Toggle { target } => act(&g, &target, Action::Toggle).await?,
        Cmd::Brightness { target, value } => act(&g, &target, Action::Brightness(value)).await?,
        Cmd::Color { target, hex } => {
            let c = Rgb::from_hex(&hex).with_context(|| format!("'{hex}' is not a colour like #ff8800"))?;
            act(&g, &target, Action::Color(c)).await?
        }
        Cmd::Kelvin { target, value } => act(&g, &target, Action::Kelvin(value)).await?,
        Cmd::Rename { target, name } => {
            let id = device_id(&g, &target).await?;
            g.rename_device(&id, &name).await?;
        }
        Cmd::AddIp { ip } => {
            g.add_device_by_ip(ip).await?;
            tokio::time::sleep(Duration::from_millis(cli.wait)).await;
            match g.devices().await.into_iter().find(|d| d.ip == ip) {
                Some(d) => println!("Added {} ({})", d.name, d.sku),
                None => bail!("no answer from {ip}; check the IP and that LAN Control is on"),
            }
        }
        Cmd::Group(c) => group(&g, c, cli.json).await?,
        Cmd::Scene(c) => scene(&g, c, cli.json).await?,
    }
    // Give UDP sends a moment to leave before the process exits.
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
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

async fn find_group(g: &Groovie, q: &str) -> Result<groovie_core::Group> {
    g.groups()
        .await
        .into_iter()
        .find(|x| x.id == q || x.name.eq_ignore_ascii_case(q))
        .with_context(|| format!("no group called '{q}'"))
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
            let name = |id: &String| {
                devices.iter().find(|d| &d.id == id).map(|d| d.name.clone()).unwrap_or(id.clone())
            };
            for gr in groups {
                let members: Vec<_> = gr.members.iter().map(name).collect();
                println!("{}: {}", gr.name, members.join(", "));
            }
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
            if json {
                println!("{}", serde_json::to_string_pretty(&scenes)?);
            } else {
                for s in scenes {
                    println!("{} ({} lights)", s.name, s.entries.len());
                }
            }
        }
        SceneCmd::Save { name, lights } => {
            let ids = if lights.is_empty() { None } else { Some(device_ids(g, &lights).await?) };
            let s = g.save_scene(&name, ids).await?;
            println!("Saved scene '{}' with {} lights", s.name, s.entries.len());
        }
        SceneCmd::Apply { scene } => g.apply_scene(&scene).await?,
        SceneCmd::Delete { scene } => {
            let s = g
                .scenes()
                .await
                .into_iter()
                .find(|s| s.id == scene || s.name.eq_ignore_ascii_case(&scene))
                .with_context(|| format!("no scene called '{scene}'"))?;
            g.delete_scene(&s.id).await?;
        }
    }
    Ok(())
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
