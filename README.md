# cloudy-af
[![License: GPL v3](https://img.shields.io/badge/License-GPL%20v3-blue.svg)](http://www.gnu.org/licenses/gpl-3.0)

Desktop configuration utility for vape battery mods running the ArcticFox firmware — a community fork of hobbyquaker's decade-old Electron [arcticfox-config](https://github.com/hobbyquaker/arcticfox-config/), rebuilt as a Tauri 2.x app packaged as a Flatpak.

Over the original it adds: a built-in live Device Monitor, system light/dark theme detection, window scaling, working Fahrenheit selection with correct temperature scaling, richer tooltips, device auto-reconnect, a "Lite" appearance mode with all five ArcticFox skins, STM32-line device support, and a Firmware Editor (inspect, patch, and flash firmware images, with emergency recovery).

![demo](demo.png)
![Screenshot](Screenshot.png)
![Screenshot_](Screenshot_.png)

# Download / Install

### Flatpak (recommended, best tested)

Pre-built bundles are on the [releases page](https://github.com/VernaUpon74/cloudy-af/releases/) and in `builds/` after building from source.

```bash
flatpak install --user builds/cloudy-af-<version>.flatpak
flatpak run org.cloudy.af
```

Or from the local Flatpak repository:

```bash
flatpak remote-add --user --no-gpg-verify cloudy-af-repo flatpak/repo
flatpak install --user cloudy-af-repo org.cloudy.af
```

### AppImage / .deb / .rpm (experimental, less testing)

Also produced by `scripts/build-appimage.sh` into `builds/` and attached to releases:

- `Cloudy_AF-<version>-x86_64.AppImage` — portable, FUSE3-only systems OK
- `Cloudy AF_<version>_amd64.deb` — Debian/Ubuntu
- `Cloudy AF-<version>-1.x86_64.rpm` — Fedora/openSUSE

These get less testing than the Flatpak and may misbehave (known: WebKit GPU crashes on some NVIDIA systems — see Debug below).

```bash
chmod +x builds/Cloudy_AF-<version>-x86_64.AppImage
./builds/Cloudy_AF-<version>-x86_64.AppImage

# or
sudo apt install ./builds/Cloudy\ AF_<version>_amd64.deb
sudo dnf install ./builds/Cloudy\ AF-<version>-1.x86_64.rpm
```

### macOS (experimental, less testing)

A manual GitHub Actions workflow (`.github/workflows/macos-build.yml`) builds a `.dmg` from source using Homebrew. ~~It works~~ but is tested rarely — expect rough edges, and HID access needs manual permission grants (below). macOS devs welcome.

```bash
brew install node@22 rust
# put node@22 on PATH (Apple Silicon: /opt/homebrew/opt/node@22/bin; Intel: /usr/local/opt/node@22/bin)
npm install
npm run sidecar:build
npm run tauri:build -- --bundles dmg
```

The `.dmg` lands in `src-tauri/target/release/bundle/dmg/`. On first launch: right-click → **Open** to pass Gatekeeper, then grant **Input Monitoring** in System Settings → Privacy & Security.

### USB permissions (all Linux builds)

The Flatpak requests `--device=all`, but HID access still needs udev rules for unprivileged users. Install them, then unplug and reconnect your device:

```bash
sudo cp flatpak/50-cloudy-af.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
```

# Building from source

Full instructions, including the mandatory `tauri/custom-protocol` feature flag and the gotchas, are in [BUILDING.md](BUILDING.md). Short version:

```bash
npm install
npm run tauri:dev        # development

npm run build            # rebuild dist/
# then cargo build --release --features tauri/custom-protocol in src-tauri/
# then flatpak-builder per BUILDING.md
```

# Usage

Keyboard shortcuts (mouse-free navigation, shared across windows): see [docs/keyboard-shortcuts.md](docs/keyboard-shortcuts.md).

Start the app and connect your ArcticFox device — it is detected automatically and its configuration downloaded. Edit profiles, power curves, TFR tables, and settings in the tabs, then **Upload** to write back.

All ArcticFox-compatible devices are supported (Joyetech eVic/Cuboid/eGrip, Eleaf iStick, Wismec Presa/Reuleaux, Vaporflask, and friends): they share the Nuvoton HID bootloader interface (VID `0x0416` / PID `0x5020`) and are told apart by the Product ID string in the device dataflash.

# Firmware encryption (VandalProof)

ArcticFox update packages (`af_*.bin`, 2018+) are AES-128-CBC encrypted ("VandalProof"): the first 16 bytes are the IV, PKCS7 padding, key = the ASCII bytes of `FA89412D87B0EFD9`. Recovered from NFirmwareEditor's obfuscated loader; full write-up in [docs/vandalproof-encryption.md](docs/vandalproof-encryption.md). The app decrypts in-app; packages can also be flashed as-is (the on-device LDROM updater decrypts them).

# Debug

If no device is detected, see USB permissions above.

### Native binary fails with `libwebkit2gtk-4.1.so.0: cannot open shared object file`

```bash
sudo dnf install webkit2gtk4.1      # Fedora / RPM
sudo apt install libwebkit2gtk-4.1-0 # Debian / Ubuntu
```

### WebKit crashes on NVIDIA (`WebKitWebProcess` abort in `libnvidia-gpucomp`)

Try software rendering first (keeps the WebKit sandbox):

```bash
./builds/cloudy-af --software-rendering
flatpak run --env=WEBKIT_FORCE_SOFTWARE_RENDERING=1 org.cloudy.af
WEBKIT_FORCE_SOFTWARE_RENDERING=1 ./builds/Cloudy_AF-<version>-x86_64.AppImage
```

Last resort — disable the WebKit sandbox entirely:

```bash
./builds/cloudy-af --disable-webkit-sandbox
flatpak run --env=WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 org.cloudy.af
```

# Project structure

- `src/` – Frontend (HTML/CSS/JS, Vite, jQuery, Photon-style CSS)
- `src-tauri/` – Tauri Rust host
- `sidecar/` – Node.js HID bridge (node-hid + arcticfox npm module)
- `flatpak/` – manifest, desktop entry, appdata, udev rules
- `public/` – static assets (i18n for 17 locales, default config)

# Fork differences

Key deviations from the original are marked with `DEVIATION` comments in `index.html`, `src/renderer.js`, `src-tauri/src/lib.rs`, `sidecar/hid-bridge.js`, `sidecar/afcfile.js`, and `sidecar/patches/arcticfox+11.0.3.patch`. Highlights:

- Tauri 2.x shell replacing Electron; Flatpak packaging with bundled Node sidecar
- Adaptive light/dark UI, window scaling, Fahrenheit by default with correct unit scaling
- Live Device Monitor with pause and hover/click point values (NToolbox-style)
- Device auto-reconnect; autofire in multi-click/shortcut actions
- Lite appearance mode; all five ArcticFox skins; NToolbox-style active-mode Layout page
- Rich hover tooltips with real vape-function descriptions, in 17 locales
- Firmware Editor: open/edit/flash ArcticFox images (all encryption schemes incl. VandalProof) with emergency recovery and "Repair Product ID"
- Support for all 47 ArcticFox Product IDs (Joyetech, Eleaf, Wismec, Vaporflask…), plus the STM32 line (Rim C family)

Planned: unique device timeout/charge animations (in progress — proven hard).

# Contributing

Clone, `npm install`, `npm run tauri:dev`. Issues and PRs welcome.

# Related

- https://github.com/hobbyquaker/arcticfox-config – original Electron app
- https://github.com/hobbyquaker/arcticfox – HID communication module
- https://github.com/hobbyquaker/arcticfox-monitor – device monitoring tool
- https://github.com/TBXin/NFirmwareEditor – NFE Team editor this work is based on

# Credits

Based on the work of [NFE Team](https://nfeteam.org/) and [hobbyquaker](https://github.com/hobbyquaker/):

- https://github.com/maelstrom2001/ArcticFox
- https://github.com/TBXin/NFirmwareEditor
- https://github.com/hobbyquaker/

This software uses [Highcharts](http://www.highcharts.com/), free __only for non-commercial use__.

Kimi Code and local models assisted in the rewrite. Images edited with OSS tools.

# Donations

Accepting donations, not expecting; I've used WinRAR.
- BTC bc1qpfq3c6hdflafccqmsl4v7ussvlw8pazpwez09m
- XMR 85YdUQXSMTgTeySZCyJjh9QTnzevgCHtZA6dhmFYMZqtE529pUZ5K8ceEC2ysaV2o4CuMuYtoaYPYdJfHYGX7m1WMgyM53i

# License

GPLv3 — Copyright (c) Sebastian Raff. Flatpak packaging and rework by OneButtFarting.

![TuxAndFox](Tux&Fox.png)
