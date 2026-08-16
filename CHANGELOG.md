# Changelog

## 1.15.2 — 2026-08-15

### Fixed
- Firmware Editor button in **Advanced → Settings** now opens the Firmware Editor window (the `firmware` IPC channel was not routed to the sub-window opener).
- **Advanced → Materials** now renders the eight TFR tables as a grid of curve previews with `[TFR] Ni`, `[TFR] Ti`, `[TFR] 304`, `[TFR] 316`, `[TFR] 316L`, `[TFR] 321`, `[TFR] NF30`, `[TFR] NiFe` labels, matching the original NToolbox layout. Clicking a card opens the TFR plot editor.
- **Advanced → Power Curves** now renders as a grid of area charts with the original NToolbox names: `Soft`, `Boost 1s`, `Boost 2s`, `Sine 1`, `Sine 2`, `Cooldown`, `Triangle`, `Linear`. Clicking a card opens the Power Curve editor.
- Profile page temperature unit is now shown as plain text (`°F` / `°C`) derived from the global **Screen → Regional → Temperature Units** setting, and the temperature input step is `10` for Fahrenheit and `5` for Celsius.
- Synced dropdown/combobox labels and button text with the original `NFirmwareEditor` English language pack (`NFE-Tools-v7.1.1/Languages/EN.lpack.txt`), including `Lock Device` deep-sleep mode.

## 1.15.1 — 2026-08-15

### Added
- Firmware Editor window accessible from **Advanced → Settings → Firmware Editor**.
- Open encrypted ArcticFox/Joyetech `.bin` firmware files, detect the device definition, and list compatible XML patches.
- Apply and rollback binary patches with wildcard support and rollback logging.
- Save modified firmware back to disk with the original encryption.
- Flash modified firmware directly to a connected HID device from the Firmware Editor toolbar.
- **Undo Changes** button reflashes the originally-opened firmware backup stored in `~/.config/cloudy-af/firmware-backups/`.
- Bundled ArcticFox firmware definition and patch directory.

## 1.2.1 — 2026-08-15

### Fixed
- Advanced Materials list now renders each user-editable TFR table as `[TFR] <name>` (e.g. `[TFR] Ni`) instead of generic `TFR1`..`TFR8`, matching the original NToolbox Advanced → Materials UI.
- TFR curve editor chart updated to match the original NToolbox TFR Profile dialog:
  - Spline curve with yellow-green line (`#9acd32`) and 2 px width.
  - X-axis 0–800 °F with light-gray grid lines every 100 °F.
  - Y-axis 1.0–4.0 with light-gray grid lines every 0.5.
  - Circle markers and live data labels showing the factor value.

## 1.2.0 — 2026-08-15

### Changed
- Renamed all runtime and packaging references from `arcticfox-config` to `cloudy-af` (binary name, Flatpak resources, desktop entry, Rust crate, and Cargo package).
- Synced version to 1.2.0 across Tauri, frontend, sidecar, and Flatpak metadata.
- Fixed window scaling: the window is locked to the base 536×621 aspect ratio, so every resize is diagonal and the UI stays proportional without dead space.
- Restored the original ArcticFox coil material labels in the Profiles dropdown: `Nickel 200`, `Titanium 1`, `SS 316`, `TCR`, and `TFR1`..`TFR8`.
- Improved AppImage build script: it now repackages Tauri's AppDir with the FUSE3 type2 runtime, bundles the Node sidecar, and prefers the smaller Flatpak-built binary/runtime when available.
- AppImage/.deb/.rpm builds now run inside the org.gnome.Sdk Flatpak runtime so the host no longer needs GTK/WebKit development headers; the bundled libraries come from the GNOME SDK.

### Added
- Runtime escape hatches for WebKit/NVIDIA rendering issues that keep the WebKit sandbox enabled by default:
  - `--software-rendering` forces WebKit software rendering (`WEBKIT_FORCE_SOFTWARE_RENDERING=1`).
  - `--disable-webkit-sandbox` disables the WebKit sandbox only when explicitly requested (`WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1`).

## 1.14.1 — 2026-08-14

### Security / Permission Policy
- Flatpak manifest no longer disables the WebKit sandbox (`WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1`).
- Removed `--talk-name=org.freedesktop.Flatpak` from Flatpak permissions.
- Removed `GDK_BACKEND=x11` so the app can use Wayland natively.

## 1.14.0 — 2026-08-13

### Added
- Hover tooltips on every labeled settings row; specific vape-function descriptions are shown where `Tooltips.*` i18n keys exist.
- Higher `z-index` on `<select>` elements and the Configuration dropdown so dropdowns layer above surrounding content.

### Changed
- Reworked window scaling so content anchors to the top-left and the footer no longer gets overlapped on resize.
- Bumped app version to 1.14.0 across Tauri, frontend, sidecar, and Flatpak metadata.

### Security
- `highcharts`: 6.1.0 → 9.0.0 (fixes high-severity XSS advisories).
- `xml2js`: 0.4.17 → 0.6.2 (fixes prototype pollution).
- Replaced the unmaintained `put` buffer-builder (used by `arcticfox`) with a local zero-initialized implementation (`sidecar/put-replacement/`) to avoid sensitive-data exposure from uninitialized `Buffer` allocations.
