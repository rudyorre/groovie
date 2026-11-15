# Groovie Lights

**Groovie** is a small, open-source, local-first app for controlling Govee lights. It works with no account and no cloud: it talks to your lights directly over your Wi-Fi using Govee's official **LAN API**.

- On/off, brightness, RGB colour, white colour temperature
- Groups (e.g. "Desk") and saved scenes (e.g. "Evening")
- Desktop app for Windows and macOS with a tray icon, plus a command-line tool (`grooviectl`)

> Groovie is an independent project and is not affiliated with or endorsed by Govee.

## Before you start: turn on LAN Control

Each light must be set up once in the official Govee Home app (so it joins your Wi-Fi). Then, in the Govee app, open the light → **Settings** (gear) → turn on **LAN Control**. Your computer must be on the same network as the lights.

Tested device types: H6072 floor lamp, H6056 light bars. Most Wi-Fi Govee lights with a "LAN Control" switch should work.

## Download

Installers are built automatically by GitHub Actions. Open the [Actions tab](https://github.com/rudyorre/groovie/actions), click the latest successful **CI** run on `main`, and download **Groovie-Windows** (`.msi` / `-setup.exe`) or **Groovie-macOS** (`.dmg`, works on Apple Silicon and Intel).

The builds are not code-signed yet, so:
- **Windows:** SmartScreen may say "Windows protected your PC". Click **More info → Run anyway**.
- **macOS:** if it says the app "is damaged" or can't be opened, drag it to Applications and run `xattr -cr /Applications/Groovie.app` in Terminal once.
- **Firewall:** allow Groovie on **private** networks when asked. It needs UDP ports 4001–4003 to talk to the lights.

## Command line

```sh
grooviectl list                           # find lights
grooviectl on all
grooviectl color "Floor Lamp" "#ff8800"
grooviectl kelvin "Light Bars" 3000
grooviectl brightness all 40
grooviectl group create Desk "Light Bars"
grooviectl scene save Evening
grooviectl scene apply Evening
grooviectl add-ip 192.168.1.50            # if discovery can't find a light
```

Targets can be a light's name, IP or id, a group name, or `all`. Run `grooviectl --help` for everything.

## Troubleshooting

- **No lights found:** check LAN Control is on, the computer and lights are on the same Wi-Fi (not a guest network), and the firewall allowed Groovie. Some routers block multicast; use **Add light by IP** (or `grooviectl add-ip`) with the light's IP from your router.
- **Other Govee apps:** the official Govee Desktop app and SignalRGB also listen on UDP port 4002. Close them while using Groovie.
- **Debug logs (CLI):** `GROOVIE_LOG=debug grooviectl list`.

## Development

Layout:

| Path | What |
|---|---|
| `crates/groovie-core` | Rust library: discovery, device state, commands, groups, scenes, config file |
| `crates/groovie-sim` | Fake Govee lights over real UDP, for testing without hardware |
| `crates/grooviectl` | CLI |
| `ui/` | Svelte 5 + Vite UI (runs in a browser with mock lights via `npm run dev`) |
| `apps/desktop/` | Tauri 2 desktop app wrapping the UI and the core |

You need [Rust](https://rustup.rs) and [Node.js](https://nodejs.org) 20+. On Windows also install the "Desktop development with C++" workload from [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) (WebView2 is already part of Windows 10/11). See the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for details.

```sh
cargo test                                # core tests (the end-to-end ones run on Linux)
cargo run -p grooviectl -- list           # CLI against real lights

cd ui && npm install && npm run dev       # UI only, with mock lights, in the browser

cd apps/desktop && npm install            # also installs ui/ deps
npm run dev                               # desktop app with hot reload
npm run build                             # installer in src-tauri/target/release/bundle
```

Testing without lights (Linux): run `cargo run -p groovie-sim -- --count 3`, then start the CLI or app with `GROOVIE_SCAN_TARGETS=127.0.0.2:4001,127.0.0.3:4001,127.0.0.4:4001`.

Groups, scenes and nicknames are stored in `groovie.json` in your OS config folder (`%APPDATA%\groovie` on Windows, `~/Library/Application Support/groovie` on macOS).

## Licence

Dual licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
