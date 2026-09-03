# cloudy-af
[![License: GPL v3](https://img.shields.io/badge/License-GPL%20v3-blue.svg)](http://www.gnu.org/licenses/gpl-3.0)

> The project is Cloudy AF (originally Arcticfox Config), a Linux desktop configuration utility for vape battery mods that run the ArcticFox firmware. It is a community fork that modernizes the decade-old Electron-based app into a Tauri desktop application, packaged as a Flatpak.

  This fork reworks hobbyquaker's Electron-based Linux/macOS [project](https://github.com/hobbyquaker/arcticfox-config) as a Rust [Tauri](https://tauri.app/) desktop app, packaging it as a sandboxed [Flatpak](https://flatpak.org) image for Linux, permission-controllable (through [Flatseal](https://github.com/tchx84/Flatseal)), because current npm is a minefield, Wine USB passthrough is a headache, and so is creating Windows VMs.
  The fork also adds quality-of-life improvements such as window scaling, an eternally dark UI, Freedom Unit selection that works (original defaulted to Celsius and capped F at 400), Autofire as a multi-click/shortcut option, device auto-reconnect, and a "Lite" appearance mode for small devices.

![demo](demo.png)
![Screenshot](Screenshot.png)
![Screenshot_](Screenshot_.png)

# Download / Install

### Linux Build Images (Flatpak, Appimage)

Pre-built Flatpak bundle is available on the
[releases page](https://github.com/VernaUpon74/cloudy-af/releases/)
and in the local `builds/` directory after running the build scripts.

Portable Appimage binary also provided on the releases page. Installable .deb and .rpm files can be found there as well. And in the local `builds/` directory after running the build scripts.

#### Install from the `.flatpak` bundle after building from source

```bash
flatpak install --user builds/cloudy-af.flatpak
```

#### Install from the local Flatpak repository

If you built the Flatpak locally, add the local repository and install from it:

```bash
flatpak remote-add --user --no-gpg-verify cloudy-af-repo flatpak/repo
flatpak install --user cloudy-af-repo org.cloudy.af
```

#### Run

```bash
flatpak run org.cloudy.af
```

#### USB permissions

The Flatpak manifest requests `--device=all`, but HID access also requires udev rules for
unprivileged users. Install the provided rules:

```bash
sudo cp flatpak/50-cloudy-af.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
```

Then unplug and reconnect your device.

### Linux (AppImage / .deb / .rpm)

Native Linux packages are produced by `scripts/build-appimage.sh` and placed in `builds/`:

- `Cloudy_AF-<version>-x86_64.AppImage` — portable, no install required
- `Cloudy AF_<version>_amd64.deb` — Debian/Ubuntu installer
- `Cloudy AF-<version>-1.x86_64.rpm` — Fedora/openSUSE installer
  
 These can also be found [here](https://github.com/VernaUpon74/cloudy-af/releases).

#### AppImage

Make the file executable and run it:

```bash
chmod +x builds/Cloudy_AF-1.2.0-x86_64.AppImage
./builds/Cloudy_AF-1.2.0-x86_64.AppImage
```

The AppImage uses a static runtime and works on systems with only FUSE3.

#### .deb / .rpm

```bash
# Debian / Ubuntu
sudo apt install ./builds/Cloudy\ AF_1.2.0_amd64.deb

# Fedora
sudo dnf install ./builds/Cloudy\ AF-1.2.0-1.x86_64.rpm
```

#### USB permissions

Native packages also need the udev rules for unprivileged HID access:

```bash
sudo cp flatpak/50-cloudy-af.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
```

Then unplug and reconnect your device.

### macOS

macOS builds are not officially produced by this fork, but the Tauri app can be built from source on
macOS using Homebrew. The resulting `.app` / `.dmg` uses the Homebrew-installed Node.js runtime to
run the HID sidecar.

#### Install prerequisites with Homebrew

```bash
# Install Homebrew if you don't have it:
# /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

brew install node@22 rust
```

Make sure Homebrew's `node@22` is in your PATH:

```bash
# For Apple Silicon Macs:
echo 'export PATH="/opt/homebrew/opt/node@22/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc

# For Intel Macs:
echo 'export PATH="/usr/local/opt/node@22/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

#### Build

```bash
npm install
npm run sidecar:build
npm run tauri:build -- --bundles dmg
```

The `.dmg` is written to `src-tauri/target/release/bundle/dmg/`.

#### USB access on macOS

macOS does not allow unprivileged HID access out of the box. The app must be granted
Input Monitoring permission in **System Settings → Privacy & Security → Input Monitoring**
after the first launch attempt. You may also need to right-click the app and choose **Open**
the first time to bypass Gatekeeper.

## Building from source

### Requirements

- Node.js 22 (LTS)
- Rust stable toolchain
- Flatpak Builder (for the Flatpak package)
- `org.gnome.Platform` runtime 49 and matching SDK
- `org.freedesktop.Sdk.Extension.node22`
- `org.freedesktop.Sdk.Extension.rust-stable`

### Local development

```bash
npm install
npm run tauri:dev
```

### Build a native release

```bash
npm install
npm run build:native
```

The native release binary is copied to `builds/cloudy-af`.
The AppImage / .deb / .rpm build script also copies completed packages to `builds/`.

### Build the Flatpak

```bash
cd flatpak
flatpak-builder --disable-rofiles-fuse --force-clean --repo=repo build-dir org.cloudy.af.yml
flatpak build-bundle repo cloudy-af.flatpak org.cloudy.af
mv cloudy-af.flatpak ../builds/
```

### Build the AppImage / .deb / .rpm

```bash
npm install
npm run appimage:build
```

All native Linux packages are copied to `builds/`.

> The AppImage build script runs Tauri's release build inside the org.gnome.Sdk
> Flatpak runtime, so the host does not need GTK/WebKit development headers.
> `scripts/build-appimage.sh` will reuse freshly built Flatpak binary, sidecar,
> and Node runtime artifacts when they are present.

## Usage

Start the application and connect your ArcticFox device. The app will automatically detect the
device and download its configuration. Use the tabs to edit profiles, power curves, TFR tables,
and device settings, then click **Upload** to write the configuration back to the device.

All ArcticFox-compatible devices are supported (Joyetech eVic/Cuboid/eGrip, Eleaf iStick,
Wismec Presa/Reuleaux, Vaporflask, and friends) — they all share the same Nuvoton HID
bootloader interface (VID `0x0416` / PID `0x5020`) and are told apart by the Product ID
string in the device dataflash.

## Firmware encryption (VandalProof)

ArcticFox firmware update packages (`af_*.bin`, 2018 and later) are encrypted with
"VandalProof": AES-128-CBC, the first 16 bytes of the file are the IV, PKCS7 padding.
The AES key is the 16 ASCII bytes of the string **`FA89412D87B0EFD9`**
(key bytes hex: `46 41 38 39 34 31 32 44 38 37 42 30 45 46 44 39`), recovered from
NFirmwareEditor's obfuscated loader. Full format and provenance:
[docs/vandalproof-encryption.md](docs/vandalproof-encryption.md). The app decrypts these
packages in-app; they can also be flashed as-is (the on-device LDROM updater decrypts them).

## Debug

If no device is detected, follow the USB permissions instructions above.

### Native binary fails with `libwebkit2gtk-4.1.so.0: cannot open shared object file`

The native Linux binary needs the system's WebKitGTK 4.1 runtime libraries:

```bash
# Fedora / RPM-based
sudo dnf install webkit2gtk4.1

# Debian / Ubuntu / apt-based
sudo apt install libwebkit2gtk-4.1-0
```

### WebKit crashes on NVIDIA (`WebKitWebProcess` ABRT in `libnvidia-gpucomp`)

The default package keeps WebKit's internal WebProcess/GPU sandbox enabled.
On some NVIDIA systems this sandbox conflicts with the driver and the webview
process aborts. Two runtime workarounds are available; try the first one
before the second because it keeps the sandbox intact.

1. **Software rendering** (keeps the sandbox):
   ```bash
   ./builds/cloudy-af --software-rendering
   # Flatpak
   flatpak run --env=WEBKIT_FORCE_SOFTWARE_RENDERING=1 org.cloudy.af
   # AppImage
   WEBKIT_FORCE_SOFTWARE_RENDERING=1 ./builds/Cloudy_AF-1.2.0-x86_64.AppImage
   ```

2. **Disable the WebKit sandbox** (last resort):
   ```bash
   ./builds/cloudy-af --disable-webkit-sandbox
   # Flatpak
   flatpak run --env=WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 org.cloudy.af
   # AppImage
   WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 ./builds/Cloudy_AF-1.2.0-x86_64.AppImage
   ```

## Project structure

- `src/` – Frontend source (HTML/CSS/JS, Vite build)
- `src-tauri/` – Tauri Rust host
- `sidecar/` – Node.js HID bridge run as a Tauri sidecar
- `flatpak/` – Flatpak manifest, desktop entry, appdata, and udev rules
- `public/` – Static assets (i18n, default config)

## Programming language / stack composition:

- Rust — Tauri 2.x host shell (src-tauri/), window management, and IPC.
- JavaScript (ES2021) + HTML/CSS — Frontend UI built with Vite, jQuery, and Photon-style CSS (src/, index.html).
- Node.js — HID sidecar (sidecar/) that bridges USB HID communication via node-hid and the arcticfox npm module.
- YAML / Shell — Flatpak manifest, desktop entry, appdata, and build scripts (flatpak/).
- JSON — i18n translations, default configuration, and package manifests.

## Fork differences

Key deviations from the original `hobbyquaker/arcticfox-config` are documented inline with
`DEVIATION` comments in:

- `index.html`
- `src/renderer.js`
- `src-tauri/src/lib.rs`
- `sidecar/hid-bridge.js`
- `sidecar/afcfile.js`
- `sidecar/patches/arcticfox+11.0.3.patch`

Notable changes include:

- Tauri 2.x desktop shell replacing Electron
- Flatpak packaging with bundled Node.js sidecar for HID access
- Dark UI by default. Up Material UIrs
- Window scaling
- Freedom units by default
- Removed Coil Material gobbledygook, TFR list back. She's got curves, baby.
- Autofire added to multi-click / shortcut dropdowns
- Device auto-reconnect on unexpected disconnect
- Lite mode support in Appearance settings
- Hover tooltips on all settings rows (specific vape-function descriptions where available)
- Dependency security updates (`highcharts` 9.x, `xml2js` 0.6.2, local `put` replacement)
- Firmware Editor: open/edit/patch/flash ArcticFox firmware images (all encryption schemes, incl. VandalProof), with an emergency recovery flasher
- Support for every ArcticFox-compatible device (47 Product IDs: Joyetech, Eleaf, Wismec, Vaporflask…), not just the iStick Pico


Planned developments include:
- Screen animations
- Auto TFR curve plotting
- Device Monitor (live device telemetry window like NToolbox's; the in-progress firmware emulation harness — `src-tauri/src/firmware/emu/` — will be used to replicate its rendering behaviour)

## Contributing

Clone the repo, run `npm install`, and use `npm run tauri:dev` for development. Submit issues and pull requests. Go crazy.

## Related

- https://github.com/hobbyquaker/arcticfox-config – Original Electron application
- https://github.com/hobbyquaker/arcticfox – Node module that abstracts HID communication with Arcticfox firmware
- https://github.com/hobbyquaker/arcticfox-monitor – Device monitoring tool
- https://github.com/hobbyquaker/dna-monitor – DNA chipset configuration tool
- https://github.com/TBXin/NFirmwareEditor – NFE Team editor that this work is based on

## Credits

Based on the work of [NFE Team](https://nfeteam.org/) and [hobbyquaker](https://github.com/hobbyquaker/)

- https://github.com/maelstrom2001/ArcticFox
- https://github.com/TBXin/NFirmwareEditor
- https://github.com/hobbyquaker/


This software uses [Highcharts](http://www.highcharts.com/) which is free __only for non-commercial use__.

Kimi Code assisted in software rewrite.
Images all edited by mouse using OSS.

## Donations

Accepting cryptocurrency donations for rework
- BTC bc1qpfq3c6hdflafccqmsl4v7ussvlw8pazpwez09m
- XMR 85YdUQXSMTgTeySZCyJjh9QTnzevgCHtZA6dhmFYMZqtE529pUZ5K8ceEC2ysaV2o4CuMuYtoaYPYdJfHYGX7m1WMgyM53i

## License

GPLv3

Copyright (c) Sebastian Raff

Flatpak packaging and rework by OneButtFarting

![TuxAndFox](Tux&Fox.png)
