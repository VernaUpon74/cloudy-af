# Changelog

## 1.2.1 — 2026-09-20 (rebuilt binaries)

*Entry rewritten at the 2026-09-20 rebuild: it covers everything actually in the
shipped 1.2.1 binaries — the 2026-09-18 evening commits that missed the original
entry, plus the working-tree changes at rebuild time.*

### Added
- **TFR curve calculator**: "Calculate from Resistance" in the TFR Curve Editor computes the 7-point temperature factor table from measured coil resistances (68–1112 °F), with optional extrapolation of the top point and one-click apply to the chart. A forked implementation of SkrunksModEXT by Aiden Lopez (CC BY-NC-SA 4.0). Shipped in all 17 locales (`scripts/check-i18n.py` green: 398 keys per locale).
- **Device fire command end-to-end**: `fire_cmd` (Tauri) → sidecar `makePuff` (0x44 puff command, firmware-managed puff timer) → `fireDevice()` bridge — the backend for the Device Monitor press-and-hold fire button and autofire.
- **Ctrl+`` ` `` sub-tab cycling**: next sub-tab within the active main tab (main-tab `Ctrl`+`Tab` unchanged); documented in `docs/keyboard-shortcuts.md`.
- **Controls sub-tab tooltips**: Settings, Multi Clicks, Shortcuts VW, and Shortcuts TC tabs now show descriptive tooltips on hover.
- **Device Monitor fire button**: press-and-hold fires; double-click toggles autofire mode with visual indicator.

### Changed
- **Device Monitor button moved back to Stats tab** (out of Advanced → Settings).
- **Resistance precision**: Device Monitor shows resistance values to 3 decimal places.
- **build.sh CLI simplified**: `./build.sh [appimage|flatpak] [-y|-n]` (`-y`/`--experimental` enables experimental patches, `-n`/`--skip` disables; prompts skipped), including the pre-build `check_disk_space()` gate. The flatpak stage now pre-creates the local OSTree repo with `core.min-free-space-size 200MB` — the default 3%-of-volume floor aborts the bundle commit on near-full disks.
- **AppImage validation scripted**: `scripts/verify-appimage.sh` launches the build inside the build toolbx (extract-mode, sandbox enabled) and gates on a mapped "Cloudy AF" window plus live WebKit helper processes; see AGENTS.md "Distribution validation".
- **Emulator progress (Nuvoton M041)**: FMC ISP model, ARMv7-M unaligned load/store legalization, UXTB/UXTH/SXTB/SXTH — M041 firmware boots through all init and ~212K main-loop instructions (pushed-LR divergence at 0xA46 still open). Suites green: emu 112/112, anim 24/24.

### Fixed
- **Linux packaging**: Flatpak/AppImage install all icon sizes + `StartupWMClass`; build.sh toolbox cargo fallback and gtk plugin path; AppImage carries root-level `libexec`/`lib64` symlinks plus the injected-bundle `.so` so the WebKit helper path resolves.
- **Device Monitor flicker**: the sidecar emits only the first disconnect instead of one per repeated probe failure.
- **Boot gate bit fix**: bus write hook sets bit 2 (not bit 3) of RAM 0x20002C30, matching firmware's `lsls r3, #0x1d` check.
- **Flatpak runtime-repo**: install uses `--runtime-repo=https://flathub.org/repo/flathub.flatpakrepo`.

## 1.2.0 — 2026-09-12

### Added
- **Firmware Editor Status tab**: verbose device hardware/model info on the default page, the same 'Device is connected/disconnected' footer bar as the rest of the app, and screenshot support status.
- **Image tools** for firmware image tables (list/read/write preview) and resource-pack preview in the Firmware Editor.
- **Patch batch actions**: apply all pending / rollback all applied at once, plus patch list search.

### Fixed
- JavaScript syntax error in `src/renderer-firmware.js` (duplicated `setImagesVisible` signature) that broke any frontend or Flatpak build of commits `87435d8`/`ebde818`.
- Curve/Materials chart reflow on window resize.
- Local Flatpak build: stale `build-dir/` caused `opendir(refs/heads): No such file or directory`; `rofiles-fuse` Permission denied on btrfs homed mounts is worked around with `--disable-rofiles-fuse`.

## 1.19.1 — 2026-09-10

### Added
- **Complete translations for all 17 locales**: every non-English language file is now key-complete (364 keys, including the Device Monitor, clock-animation and new-tooltip strings that were previously missing) and the large block of English-only tooltip text is translated. Czech `cs`/`cz` twins updated together.
- **Tab tooltips**: the Advanced sub-tabs (Settings, Power Curves, Materials, BVO) and Screen sub-tabs (Settings, Appearance, Layout, Stealth, Regional) now show tooltip descriptions on hover, matching the main tabs.

### Changed
- **Device Monitor button moved to the Stats tab** (out of Advanced → Settings).

## 1.19.0 — 2026-09-10

### Added
- **Device Monitor chart: hover/click any point for its exact value**: lines now show a shared tooltip with per-series values and a crosshair at the hovered timestamp, and the chart is restyled (dark theme, single toggle column on the left) as a Highcharts port matching NToolbox's Device Monitor.
- **System dark/light theme detection**: the app now follows the OS `prefers-color-scheme` (light palette added) and switches live when the system theme changes — including the Device Monitor chart.
- **"Repair Product ID" (Force PID) in the Firmware Editor's Recovery tab**: when a flash leaves the device dataflash's product ID erased or wrong (symptom: the mod crash-loops or isn't detected after flashing — e.g. stock firmware from another model rewrote the identity), the app rewrites the ID to the expected value, clears the boot flag, restarts, and verifies the write stuck. Same recovery NToolbox exposes; diagnosed and proven on a Pico Dual that had been left with Pico 25 (M077) stock firmware on 96×16 hardware.
- **Post-flash progress events end-to-end**: `flash-progress` lines (waiting for bootloader replug, flashing, verifying) now reach the Firmware Editor status line in every flash path.
- **Broader STM32-line device support**: the Rim C entry now covers the ArcticFox STM32 device family reported by NFE post-190718 builds (M149/M172 alongside M177).

### Fixed
- **Recovery flash could fail to return after a successful write**: the LDROM finalizes a flash asynchronously and drops USB briefly, so a single-shot restart after streaming the image could fail transiently and report a good flash as failed. The restart is now retried before giving up.
- **Autofire timeout fixed**: was cutting off at 15s instead of 60s and units jumping by fractional seconds. Fixed sometime before this version.
- **Fixed Puff Time**
- **Sucks less now**
- **Line setup detects Appearance setting and shows correct one automatically.**
- **No more Herobrine**

### Docs
- `docs/firmware-validation.md` gains the device USB operation modes (APROM "Joyetech" vs LDROM "Nuvoton" — the latter is the normal updater state for flashing, not an error) and the dataflash info-block ground truth (version at 260, PID at 316, boot flag at 13, build stamp at 516), from the Pico Dual repair session.

## 1.18.3 — 2026-09-10

### Fixed
- **Firmware flash failing on devices that won't soft-switch to bootloader mode** (observed on the Pico Dual): when the boot-flag switch and restart don't re-enumerate the device in LDROM mode, the flash now falls back to the recovery-style protocol — it waits for the device to appear in bootloader mode (unplug and replug it, holding a button while plugging in if needed) and then streams as usual. Progress reaches the Firmware Editor status line via a new `flash-progress` event, so the user knows to replug.

### Added
- **Device Monitor pause**: a Pause/Resume button plus the Space key freeze and resume the live graph (Space is ignored while a button/input has focus, so native Space behavior is preserved).

### Changed
- **Firmware Editor auto-loads stock firmware**: on editor open (and when a device appears later), the matching stock ArcticFox build for the connected device is loaded automatically, as on Pico mods. "Download Stock" is now "Download FW".

## 1.18.2 — 2026-09-10

### Fixed
- **Sidecar crash (SIGABRT in node-hid) when opening the Firmware Editor or Device Monitor with a device plugged in**: node-hid 2.2.0's `close()` frees the hidraw handle while its read thread can still be blocked inside a read — closing at that moment aborts the whole sidecar ("free(): invalid pointer" in `HID_hidraw.node`). The bundled node-hid now carries a patch (`sidecar/patches/node-hid+2.2.0.patch`) that mutex-serializes `close()` against the read thread. Stress-tested live: 60 consecutive suspend/resume/monitoring cycles against a plugged-in device with no abort.
- **Configuration dropdown showing two down-arrows**: the EN label carried a literal `▾` on top of the CSS chevron used by every other dropdown; removed so only the shared style remains.

## 1.18.1 — 2026-09-10

### Added
- **Eleaf iStick Rim C support (ArcticFox STM32 line)**: STM32-line devices (USB VID 0483 / PID 5750, product M177) now connect, load settings and stream Device Monitor telemetry. The HID command packet needs the STM signature `5C CA 37 75` instead of the Nuvoton `HIDC`, and firmware flashing targets the STM flash base `0x0800C000` instead of 0 — both taken from NFE's NCore STM32 support (NFE-Tools v190718 beta). Logo upload on STM32-line devices is untested and unchanged.
- **Full NFE device catalog**: the product table now covers all 69 devices of NFE-Tools v190718's NCore database (was 37) — newly named Eleaf iStick Pico 25/21700/S, Tria, Pico Squeeze 2, ASTER RT, iKuu i80, iKonn 220, Invoke 220, Lexicon, iStick Mix; Joyetech eVic Primo Mini SE / Primo Fit, Elitar Pipe, Ultex T80, Espion / Espion Solo; Twisp Vega / Vega Mini; Wismec RX GEN3 (incl. Dual, RX2 20700/21700), Active, Luxotic DF/MF, Sinuous P80/CB-80/V80/V200/Ravage230, ES300/myTri — so any NFE-supported ArcticFox device now shows its proper name, and firmware builds match it to the right device line.

### Fixed
- **Device Monitor crashing the HID sidecar**: each monitor sample used to suspend the sidecar — closing and reopening the hidraw handle every poll — which raced node-hid's read thread (SIGABRT, "free(): invalid pointer") and killed the sidecar, leaving the monitor non-functional. Samples now go through the sidecar's own `monitoring` request, which keeps the handle open.
- **Firmware Editor "Download Stock" failing while the device was connected**: direct-from-Rust device reads (`download_stock`, `read_device_dataflash`, `read_device_product_id`, restart) did not suspend the sidecar, whose always-on hidraw reader thread consumed the device's response — the read then timed out. All four now suspend the sidecar for the duration (same guard pattern as flashing).
- **Device Monitor temperature readings (wrong value and unit)**: Temperature and TemperatureSet were divided by 10 as if the firmware reported tenths, and the Fahrenheit→Celsius conversion was applied on top of that scaled value. The firmware reports whole degrees in the device's configured unit (NToolbox passes them to the chart unscaled; only PowerSet/V/A/Ω are scaled), so a 70 °F coil read as −13.9 °C. All three temperature sensors (including BoardTemperature, which also follows the device unit) now display the raw whole-degree value with the °C/°F label following the device setting, matching NToolbox's Device Monitor. Verified live against a °F-mode device.

## 1.18.0 — 2026-09-10

### Added
- **Accessibility tooltips on every setting**: all ~120 settings rows now carry a concise description of what the setting does, shown on hover/focus, written for screen-reader and low-vision users.
- **Live Device Monitor under Advanced** (NToolbox port): real-time battery voltage, board temperature, live current and resistance readouts streamed from the device while connected.
- **Charge-screen timeout animations** (firmware patch, af_190602): three selectable CRT-style fade effects — Gradient Fade, Center Pulse, and Diagonal Sweep — injected into the firmware's show-clock-on-timeout path via a code-cave patch. Effect is chosen by a dataflash config byte; each frame of the patched firmware is verified against an in-emulator reference before flashing.

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
