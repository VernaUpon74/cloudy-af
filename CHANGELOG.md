# Changelog

## Unreleased

### Fixed
- **Zoom fit at startup**: the fit computation now measures the real content container and re-runs once web fonts and i18n text settle, fixing the tab bar being clipped under the header until the first window resize.

## 1.17.0 — 2026-09-06

### Added
- **Small & Medium main-screen skins** for 64×128 displays: Appearance → Main Screen Skin now offers all five ArcticFox skins, and the Screen Layout page shows only the active mode's fields with a caption naming it (NToolbox-style), replacing the old manual mode tab bar.

### Changed
- **Crisp text at any window size**: the scaled content area now uses CSS `zoom` (text re-rasterized at the final size) instead of `transform: scale()` (1× raster upscaled), fixing blurry/aliased text — most visible on Advanced → Settings.
- **Controls → Shortcuts (VW & TC) rearranged into a 2×2 grid**: In Standby, In Menu, In Edit and In Profile Select each get their own titled table (the shared "In Menu | In Edit" table was split into two), matching NToolbox's layout.
- **Puff Cut-Off steps in whole seconds**: the spinner moves by 1 s and fractional values read from the device are rounded to the nearest second on load, so the field never shows decimal points. The device still stores 0.1 s units internally.

## 1.16.2 — 2026-09-04

### Fixed
- **Emergency Recovery flash now accepts encrypted `.bin` files**: the recovery LDROM updater expects plaintext, so any selected firmware (VandalProof/ArcticFox/Joyetech or already-decrypted) is decrypted before streaming to the device.

### Added
- **Bundled decrypted firmware images** in `resources/firmware/decrypted/` for the four supported builds (`af_170222`, `af_180913`, `af_190602`, `af_211009`), making it easier to inspect or patch firmware without manual decryption.

### Changed
- Shortened the Emergency Recovery tab explanation in the Firmware Editor.

## 1.16.1 — 2026-09-04

### Fixed
- **Bricked-device flash race (root cause of a real brick)**: the app no longer allows concurrent instances. A second launch now focuses the existing window instead of spawning a parallel instance whose auto-reconnecting HID sidecar could interleave config commands into another instance's firmware flash stream and corrupt the image.
- A process-wide flash mutex now serializes every command that touches the device directly (flash, undo, recovery, dataflash/Product ID reads, restart), closing the same interleaving race between two windows of the SAME instance (e.g. Recovery tab waiting for a device while the editor flashes).
- **Firmware flash write now retries the whole upload** (command + full stream, 15 s window) on failure, matching NToolbox's `WriteFirmware` behavior; previously a persistent mid-stream error aborted the upload and left a partially programmed APROM. Also added NToolbox's 100 ms settle delay between the dataflash boot-flag write and the restart command.
- **Recovery flash now survives device flapping**: the wait-for-device → flash cycle repeats until the flash completes instead of failing on the first drop, and deterministic errors (bad image size) abort immediately instead of looping forever.
- **HID sidecar crash** (`free(): invalid pointer` in node-hid when the device disappears mid-read, e.g. while rebooting into LDROM): the sidecar now pauses node-hid's reader thread before closing the device handle.
- **Firmware emulation harness** (pre-flash gate): CBZ/CBNZ and all CPS variants now decode (real compiler output faulted as Undefined), the BL second halfword is validated, and stack-exhaustion is caught by a canary at the stack-region bottom plus minimum-sp tracking.

### Changed
- **Recovery tab now auto-loads the bundled original manufacturer firmware** for the detected device (iStick Pico V1.00 for the Pico; iStick Pico 25 V1.05 as the stock loop-breaker for the other 18 Eleaf Nuvoton-line devices; more can be added to `resources/firmware/devices.json`) instead of requiring file selection, and pre-fills the Product ID guard. A bricked device must be flashed with stock firmware first — ArcticFox flashed directly onto a bricked device keeps the boot loop (re-verified on hardware 2026-09-04), so devices without a bundled stock image ask for the manufacturer file rather than falling back to AF. After a successful stock recovery flash it offers to flash the matching bundled ArcticFox build immediately. Choosing a file manually still overrides the auto-selection.

## 1.16.0 — 2026-09-03

### Added
- **Firmware Editor → Recovery tab**: emergency recovery flasher that waits for a bricked device to enumerate (handles USB flapping), optionally guards on Product ID (e.g. `M041`), streams the image with retry/backoff, restarts the device, and verifies it boots the flashed firmware. Includes a live connected-devices list.
- **All ArcticFox-compatible devices are now recognised**, not just the iStick Pico: the device list grew from 19 Eleaf-only Product IDs to the full 47 known IDs (Joyetech eVic/Cuboid/eGrip, Eleaf iStick, Wismec Presa/Reuleaux, Vaporflask, Beyondvape, Vaponaute, Vapor Shark). All share the same Nuvoton HID interface (VID `0x0416` / PID `0x5020`); no udev rule changes needed.
- **VandalProof decryption documented with the recovered AES key** (`FA89412D87B0EFD9` as ASCII): see `docs/vandalproof-encryption.md`.
- **Appearance → Clock Type → Clock Animation** dropdown (Off / Swirl / Gradient Fade / Rippling Wave), persisted in `.afc` configs as `ClockAnimation`. Disabled until firmware patch support lands.
- Default TFR/power-curve plot data: zeroed tables fall back to the built-in default curves so plots never render a flat zero line.
- Hardware-in-the-loop tests (`cargo test -- --ignored`) covering dataflash reads, multi-device enumeration, screenshots, logo writes, and recovery flashing.

### Fixed
- HID protocol correctness in the firmware flasher, verified on real devices:
  - Command packets are 18 bytes with a 4-byte little-endian checksum (was 15 bytes/1 byte).
  - `ReadDataflash`/`WriteDataflash` send the length argument (was 0).
  - HID reads no longer skip a report-ID byte (report ID 0 devices return pure payload).
  - LDROM boot-flag reads use the correct dataflash offset (raw 13, was 9).
  - Firmware streaming retries with backoff while the LDROM programs flash (was a hard failure mid-flash).
- Product ID read offset corrected for the 4-byte dataflash checksum prefix.
- Flashing now restarts the device afterwards and waits for it to return in APROM mode.
- **Firmware Editor failed to open STM32-line builds** (`af_211009.bin`, "Definition not found"): the shipped `resources/definitions/ArcticFox.xml` lacked the STM32-line definition (`Joyetech APP` marker) that the test suite had inlined. Definition added; the stock test now parses the shipped file so the gap can't regress.
- **Puff Cut-Off ignored on current firmware (SettingsVersion 12)**: the v12 config layout widened `PuffCutOff` from u8 to u16 (tenths of a second, up to 60 s), but the config codec still used the v11 single byte — misaligning every Advanced field after it (battery offsets, TFR tables, power curves all read/written shifted by one byte). The codec now branches on SettingsVersion; encode/decode round-trips byte-exact against the live device, and the UI cap is raised 15 s → 60 s.
- Float truncation on scaled config writes (`Voltage * 100`, `Factor * 10000`, power/resistance scaling) could land one unit low; all scaled encodes now round.
- **Firmware flash hardening** (audit of the Firmware Editor flash path):
  - Flashing from the editor now verifies the connected device's Product ID belongs to the firmware's device line (Nuvoton vs STM32), refusing cross-line flashes. The guard check runs *before* the device is switched to bootloader mode, so a refused flash leaves the device untouched.
  - The post-flash restart is retried with backoff instead of failing single-shot while the LDROM finalizes flash — a successful flash is no longer reported as failed.
  - The HID sidecar (config polling) is suspended for the duration of any flash/undo/recovery operation, closing a race where config commands could interleave into the firmware stream.
  - Firmware images are size-checked before streaming (1 KiB–128 KiB); oversized images are rejected instead of being written past the end of APROM.
- **Flatpak builds didn't bundle `resources/firmware`**, so Download Stock / Open Stock Build failed with "firmware resource directory not found". Both manifests now copy it.

### Changed
- VandalProof-encrypted update packages are now decrypted in-app (AES-128-CBC, key `FA89412D87B0EFD9` as ASCII, IV = first 16 bytes of the file — see `docs/vandalproof-encryption.md`). They can also still be flashed as-is: VandalProof decryption happens on-device (LDROM).

## 1.15.3 — 2026-08-15

### Fixed
- **Advanced → Materials** labels now use the fixed NFE default names `[TFR] Ni`, `[TFR] Ti`, `[TFR] 304`, `[TFR] 316`, `[TFR] 316L`, `[TFR] 321`, `[TFR] NF30`, `[TFR] NiFe` regardless of stored table names.
- **Advanced → Power Curves** labels and the profile **Power Curve** dropdown now use fixed NFE names: `Soft`, `Boost 1s`, `Boost 2s`, `Sine 1`, `Sine 2`, `Cooldown`, `Triangle`, `Linear`.

## 1.15.2 — 2026-08-15

### Fixed
- Firmware Editor button in **Advanced → Settings** now opens the Firmware Editor window (the `firmware` IPC channel was not routed to the sub-window opener).
- **Advanced → Materials** now renders the eight TFR tables as a grid of curve previews with `[TFR] <name>` labels, matching the original NToolbox layout. Clicking a card opens the TFR plot editor.
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
- Runtime escape hatches for WebKit/NVIDIA rendering issues that keep the WebKit sandbox enabled by default:
  - `--software-rendering` forces WebKit software rendering (`WEBKIT_FORCE_SOFTWARE_RENDERING=1`).
  - `--disable-webkit-sandbox` disables the WebKit sandbox only when explicitly requested (`WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1`).
### Changed
- Renamed all runtime and packaging references from `arcticfox-config` to `cloudy-af` (binary name, Flatpak resources, desktop entry, Rust crate, and Cargo package).
- Improved AppImage build script: it now repackages Tauri's AppDir with the FUSE3 type2 runtime, bundles the Node sidecar, and prefers the smaller Flatpak-built binary/runtime when available.
- AppImage/.deb/.rpm builds now run inside the org.gnome.Sdk Flatpak runtime so the host no longer needs GTK/WebKit development headers; the bundled libraries come from the GNOME SDK.

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
