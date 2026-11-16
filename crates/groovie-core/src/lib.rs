//! # groovie-core
//!
//! Local-first control of Govee lights over the official LAN API: no account,
//! no cloud.
//!
//! ```no_run
//! # async fn demo() -> groovie_core::Result<()> {
//! use groovie_core::{Action, Config, Groovie, Rgb, Target};
//! let g = Groovie::start(Config::default()).await?;
//! tokio::time::sleep(std::time::Duration::from_secs(2)).await; // let discovery run
//! g.apply(&Target::All, Action::Power(true)).await?;
//! g.apply(&Target::All, Action::Color(Rgb::new(255, 120, 0))).await?;
//! # Ok(()) }
//! ```

pub mod engine;
pub mod ipc;
pub mod model;
pub mod protocol;
pub mod store;
pub mod sync;

pub use engine::{
    new_store_id, send_ephemeral_action, send_ephemeral_scene, Config, Error, Groovie, Result,
};
pub use model::*;
pub use protocol::Rgb;
pub use sync::*;

impl Config {
    /// Apply developer overrides from the environment, handy for pointing the
    /// app at `groovie-sim`:
    ///
    /// * `GROOVIE_SCAN_TARGETS`: comma-separated `ip:port` list
    /// * `GROOVIE_LISTEN`: `ip:port` to receive replies on
    /// * `GROOVIE_COMMAND_PORT` / `GROOVIE_SCAN_PORT`
    /// * `GROOVIE_CONFIG`: path to the JSON config file
    pub fn with_env_overrides(mut self) -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let Some(v) = var("GROOVIE_SCAN_TARGETS") {
            let targets: Vec<_> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if !targets.is_empty() {
                self.scan_targets = targets;
            }
        }
        if let Some(addr) = var("GROOVIE_LISTEN").and_then(|v| v.parse().ok()) {
            self.listen = addr;
        }
        if let Some(p) = var("GROOVIE_COMMAND_PORT").and_then(|v| v.parse().ok()) {
            self.command_port = p;
        }
        if let Some(p) = var("GROOVIE_SCAN_PORT").and_then(|v| v.parse().ok()) {
            self.scan_port = p;
        }
        if let Some(p) = var("GROOVIE_CONFIG") {
            self.store_path = Some(p.into());
        }
        self
    }
}
