<div align="center">

<img src="docs/logo-dark.svg#gh-dark-mode-only" height="44" alt="Groovie" />
<img src="docs/logo-light.svg#gh-light-mode-only" height="44" alt="Groovie" />

<p>
  Local-first Govee light control for Windows, macOS, Linux &amp; iOS — desktop tray app, mobile app + <code>grooviectl</code> CLI.<br />
  No account, no cloud, instant two-way LAN sync.
</p>

<p>
  <a href="https://github.com/rudyorre/groovie/releases/latest"><b>Download</b></a>
  &nbsp;·&nbsp;
  <a href="#ios">iOS</a>
  &nbsp;·&nbsp;
  <a href="#setup">Setup</a>
  &nbsp;·&nbsp;
  <a href="#command-line">CLI</a>
  &nbsp;·&nbsp;
  <a href="#configuration">Configuration</a>
  &nbsp;·&nbsp;
  <a href="#development">Development</a>
</p>

<img src="docs/preview-dark.svg#gh-dark-mode-only" width="820" alt="Groovie desktop app showing Floor Lamp in RGB Colour mode and Light Bars in 4000K White mode" />
<img src="docs/preview-light.svg#gh-light-mode-only" width="820" alt="Groovie desktop app showing Floor Lamp in RGB Colour mode and Light Bars in 4000K White mode" />

</div>

<br />

**Groovie** talks directly to Wi-Fi Govee lights over your local network using Govee's official UDP LAN API:

- **Direct control:** power, brightness, RGB colour, and white colour temperature (`2000K–9000K`)
- **Two-way LAN sync:** changes made in the Govee mobile app sync back automatically without cloud polling
- **Groups & scenes:** control rooms together (e.g. "Desk") and snapshot saved looks (e.g. "Evening")
- **Screen Sync (Windows):** low-latency 2D spatial display sync with per-segment & split-bar RGBIC streaming, Oklab smoothing, and automatic letterbox cropping
- **Desktop, iOS & CLI:** native Tauri apps for Windows, macOS, Linux, and iOS (free sideload), plus `grooviectl` for scripts and keybindings

> Groovie is an independent open-source project and is not affiliated with or endorsed by Govee.

## Download

Grab the latest installer from [**Releases**](https://github.com/rudyorre/groovie/releases/latest):

- **Windows:** `Groovie_x.y.z_x64-setup.exe`
- **macOS:** `Groovie_x.y.z_universal.dmg` (Apple Silicon & Intel)
- **Linux:** `Groovie_x.y.z_amd64.AppImage` or `.deb`
- **iOS (free sideload):** `Groovie_x.y.z_ios.ipa` or add `altstore-source.json` in [SideStore](https://sidestore.io) / [AltStore](https://altstore.io) (see [**iOS**](#ios))
- **`grooviectl` CLI (optional):** `grooviectl-windows-x64.zip`, `grooviectl-macos-universal.tar.gz`, `grooviectl-linux-x64.tar.gz`

**In-app updates (desktop):** when a new release is published, Groovie shows an **Update** button in the sidebar that downloads, verifies, installs, and relaunches in one click.

Notes on first launch (builds are not OS-notarized yet):
- **Windows:** if SmartScreen says *"Windows protected your PC"*, click **More info → Run anyway**.
- **macOS:** if Gatekeeper blocks opening, drag `Groovie.app` to Applications and run `xattr -cr /Applications/Groovie.app` once.
- **Firewall:** allow Groovie on **private** networks when prompted (UDP ports `4001–4003`).

## iOS

Groovie runs natively on iPhone and iPad and talks directly to your lights over Wi-Fi UDP (even when your PC is turned off) — **no \$99/year Apple Developer account or Mac required**:

1. **Install via SideStore, AltStore, or LiveContainer (free Apple ID):**
   - **Source URL (1-tap updates in SideStore / AltStore → Sources → `+`):**
     ```text
     https://github.com/rudyorre/groovie/releases/latest/download/altstore-source.json
     ```
   - **Direct `.ipa`:** download `Groovie_x.y.z_ios.ipa` from [**Releases**](https://github.com/rudyorre/groovie/releases/latest) (or the `Groovie-iOS` artifact on any GitHub Actions CI run) and open it in SideStore, AltStore, or LiveContainer.
2. **Allow Local Network on first launch:** tap **Allow** when iOS prompts. Because Apple gates UDP multicast behind a paid developer entitlement, Groovie on iOS automatically sweeps your local `/24` Wi-Fi subnet over unicast UDP (`<ip>:4001`) while ignoring VPN/tunnel interfaces (so it works alongside SideStore's WireGuard VPN).
3. **Sync your `groovie.json`:** open the iOS **Files** app → **On My iPhone → Groovie → `groovie.json`**, or open **Menu → Configuration → Edit / Paste** (and **Share** / **Copy JSON**) to import or export your desktop nicknames, groups, scenes, and light IPs.

## Setup

Each light must join your Wi-Fi once in the official Govee Home app. Then open the light in the Govee app → **Settings** (gear) → enable **LAN Control**, and make sure your computer or phone is on the same network.

Tested hardware includes `H6072` (Floor Lamp) and `H6056` (Light Bars); most Wi-Fi Govee lights with a **LAN Control** toggle work out of the box.

## Command line

Download the `grooviectl-*` archive for your OS from [**Releases**](https://github.com/rudyorre/groovie/releases/latest) (or build from source with `cargo install --path crates/grooviectl`) and place `grooviectl` on your `PATH`:

```sh
grooviectl list                           # discover lights and print status
grooviectl on all
grooviectl color "Floor Lamp" "#5e5ce6"
grooviectl kelvin "Light Bars" 4000
grooviectl brightness all 80
grooviectl group create Desk "Light Bars"
grooviectl scene save Evening
grooviectl scene apply Evening
grooviectl add-ip 192.168.1.50            # add a light directly if multicast is blocked
grooviectl config                         # print active groovie.json path (--json for contents)
```

Targets accept a light's nickname, IP, or device ID, a group name, or `all`. Run `grooviectl --help` for full usage.

## Configuration

Nicknames, groups, scenes, and known devices are stored in plain, deterministic JSON (`groovie.json`) designed to play nicely with dotfiles:

- **Default locations:** `%APPDATA%\groovie\groovie.json` on Windows, `~/Library/Application Support/groovie/groovie.json` on macOS, `~/.config/groovie/groovie.json` on Linux, and **On My iPhone → Groovie → `groovie.json`** (`~/Documents/groovie.json`) on iOS.
- **Cross-OS dotfiles:** if `~/.config/groovie/groovie.json` exists on Windows or macOS, Groovie uses it automatically. You can also **Edit / Paste** or **Copy / Share** raw `groovie.json` directly inside the **Configuration** dialog on both desktop and iOS.
- **Symlink & custom-path friendly:** atomic saves follow symlinks (`ln -s`, `stow`, `chezmoi`) instead of replacing them, or you can point Groovie at any path from the **Configuration** button in the sidebar footer (or via `GROOVIE_CONFIG=/path/to/groovie.json`). External edits reload automatically when you focus the app.

## Troubleshooting

- **No lights found:** confirm **LAN Control** is enabled, your device and lights are on the same Wi-Fi (not a guest VLAN), and your firewall/OS allows Local Network access for Groovie. If your router blocks multicast or client discovery, use **+** (**Add light by IP**), paste your `groovie.json` in **Configuration**, or run `grooviectl add-ip <ip>`.
- **Port 4002 busy:** Govee Desktop and SignalRGB also bind UDP port `4002` — close them while running Groovie.
- **Packet & state logs:** click the terminal (`>_`) button in the sidebar (or **Menu → Activity & network log** on mobile) to inspect live UDP traffic and state transitions, or run `GROOVIE_LOG=debug grooviectl list` in the terminal.

## Development

| Path | Description |
|---|---|
| `crates/groovie-core` | Rust library: UDP LAN protocol, discovery, state sync, groups, scenes, `groovie.json` store |
| `crates/groovie-sim` | Simulated Govee LAN lights over real UDP for local testing without hardware |
| `crates/grooviectl` | Command-line interface |
| `ui/` | Svelte 5 + Vite frontend (runs standalone in a browser with mock lights via `npm run dev`) |
| `apps/desktop/` | Tauri 2 desktop shell + system tray + auto-updater + Windows DXGI Screen Sync |
| `apps/ios/` | Tauri 2 iOS shell (unsigned `.ipa` built headlessly in GitHub Actions via `.github/scripts/build-ios-ipa.sh`) |

Prerequisites: [Rust](https://rustup.rs), [Node.js](https://nodejs.org) 20+, and platform [Tauri v2 dependencies](https://v2.tauri.app/start/prerequisites/).

```sh
cargo test --workspace                    # unit + simulated UDP integration tests
cargo run -p grooviectl -- list           # run CLI against local lights

cd ui && npm install && npm run dev       # browser UI with mock lights

cd apps/desktop && npm install            # installs desktop + UI dependencies
npm run dev                               # desktop app with hot reload
npm run build                             # native installer in src-tauri/target/release/bundle
```

To test without physical lights on Linux, run `cargo run -p groovie-sim -- --count 3` in one terminal and launch Groovie or `grooviectl` with `GROOVIE_SCAN_TARGETS=127.0.0.2:4001,127.0.0.3:4001,127.0.0.4:4001`.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
