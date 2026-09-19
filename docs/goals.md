# Goals

Loose ends carried over from the firmware read-back Phase 1 plan
(`docs/superpowers/plans/2026-08-19-firmware-readback-phase1.md`, completed
2026-08-27, gate B).

- [x] FIRST PRIORITY: Firmware flasher only successfully flashes decrypted .bin files,
      fails with unencrypted .bin firmware files. Add decrypt-on-bin-select. Also, bundle 
      decrypted fw versions into project for ease of use. Shorten explanation on Emergency Recovery page


- [x] Commit the (currently untracked) plan file
      `docs/superpowers/plans/2026-08-19-firmware-readback-phase1.md` as
      `docs: firmware read-back phase 1 plan`. (done: `3c1c6f3`)
- [x] Investigate the `screenshot()` HID read timeout in
      `test_check_state_hardware` (`src-tauri/src/firmware/tests/flasher_test.rs`) —
      the version/product-id reads succeed but the 1024-byte screenshot read
      times out on stock v1.00.
      (done: root cause = 0xC1 is ArcticFox-only; stock v1.00's dispatcher
      (0x4d66) services only 0x35/0x3C/0x53/0x7C/0xB4 and silently drops the
      rest. Screenshots require ArcticFox on the device. Test made non-fatal
      and `flasher::screenshot` documented accordingly.)
- [x] Upgrade Pico to ArcticFox (done 2026-08-27: af_190602 flashed via
      LDROM WriteData; product M041, boot flag 0, user-confirmed AF UI on
      screen; 0xC1 screenshot now responds). Stock v1.00 rescue kit staged in
      `test-fixtures/rescue/` (image + dataflash backup + README with the
      battery-out + Plus recovery procedure).
- [x] Screenshot capture quirks on AF: framebuffer reads all-zero when the
      display is asleep (wake via button press and capture immediately);
      PNG packing in `test_screenshot_hardware` renders garbage at the top —
      Fixed in `flasher_test.rs` via correct 1bpp indexing.
- [x] Device Monitor: replicate NToolbox's live device-monitoring window
      (telemetry readouts). The firmware emulation harness
      (`src-tauri/src/firmware/emu/`, plan
      `docs/superpowers/plans/2026-08-28-firmware-emulation-harness.md`) is
      intended to help replicate its rendering. NToolbox runs locally for
      reference via Wine — see `docs/wine-usb-passthrough.md`.
      Backend read (`read_monitoring_data`, cmd 0x66) implemented in
      `flasher.rs`; the UI window is implemented. Added a secondary y-axis
      for resistance so it is readable alongside temperature.
      (Update 2026-09-15: the chart was reverted to a single fixed y-axis
      0–640 in the working tree — resistance is plotted on the shared axis
      again; the `yAxis` field stays in `SENSORS` so the dual-axis can be
      restored cheaply if wanted.)
- [x] Emulation harness gate: layers 1–3 done (plan
      `docs/superpowers/plans/2026-08-28-firmware-emulation-harness.md`).
      Layer 4 (`test_af_190602_render_gate`) and Layer 5
      (`test_af_190602_animation_frames`) now pass locally with the in-tree
      `resources/animations/af_190602.json` and the gitignored decrypted
      firmware artifact. Tests remain `#[ignore]` so CI does not fail on
      machines lacking the decrypted artifact.
- [x] Phase 2 — stock library & pipeline: use `flasher::read_fw_version()`
      to match the device against bundled stock builds (see
      `docs/superpowers/specs/2026-08-19-firmware-animation-pipeline-design.md`,
      Phase 2). (done 2026-08-27: four bundled builds + devices.json with
      matcher; af_190602 fw_versions=[110] observed on the Pico; apply →
      flash → undo hardware cycle passes in `test_stock_cycle_hardware`.)
      
- [x] Give Firmware Editor the same 'Device is connected/disconnected' footer bar, alongside the device name header bar.
      Default first page should be 'Status' displaying even more verbose device hardware/model info

- [x] We already tried the plan in ~/cloudy-af/.github/workflows/macos-build.yml, address Github Actions macOS build error and the vulnerability Github found- context in new files in '~/cloudy-af/docs/'
      Describe errors when found
      
      **macOS build error found (2026-09-11):** `npm ci` fails during `sidecar:build` / `patch-package` because the runner's working tree contained an `arcticfox+11.0.4.patch` that did not match the installed `arcticfox@11.0.3` (`arcticfox@11.0.3 ✔` applied, then `arcticfox+11.0.4.patch` failed). Local tree has already been moved to `arcticfox@^11.0.3` + `arcticfox+11.0.3.patch`; `npm install --dry-run` in `sidecar/` now applies both patches cleanly. Next step is to push and re-run the workflow to confirm.
- [x] macOS GitHub Actions build — arcticfox patch-version fix committed to the Documents mirror (`arcticfox@^11.0.3` + `arcticfox+11.0.3.patch`); workflow file `.github/workflows/macos-build.yml` included in that push; next step is to re-run the workflow to confirm green. Context: `docs/macos-ci.md`, `docs/macos-ci-forgejo-fallback.md`, `docs/macos-job-logs.txt` (mirror). So far only VERIFIED the local `npm install --dry-run` applies both patches cleanly; the CI green result is still pending.
      
      **GitHub vulnerability found (2026-09-11):** Dependabot alert #1 — unsound `Iterator`/`DoubleEndedIterator` impls for `glib::VariantStrIter` in `src-tauri/Cargo.lock`. Current locked version is `glib 0.18.5`, which Dependabot reports as the latest possible version in the 0.18.x line. `glib` is pulled in transitively by the gtk-rs stack (`gtk`, `gdk`, `cairo-rs`, `gio`, `webkit2gtk`) used by Tauri v2.11.5. Project code does not directly use `VariantStrIter`. Fixing it likely requires upgrading the gtk-rs ecosystem / Tauri to a version that pulls `glib >= 0.19.x`.
      **Update 2026-09-12:** `cargo search` shows Tauri 2.11.5 is still the latest release and its `Cargo.toml` pins `gtk = "0.18"` and `webkit2gtk = "2"`, which in turn pin `glib 0.18.5`. A newer `glib` (0.22.9) and `gtk` (0.19.0) exist on crates.io, but Tauri does not yet declare compatibility with them. Forcing an override with `[patch.crates-io]` would likely break Tauri's Linux webview/gtk integration at compile time. This vulnerability is therefore blocked on a future Tauri/gtk-rs release; recommend monitoring and upgrading once Tauri supports gtk 0.19+/glib 0.19+.

- [x] Address these two format errors- 1) switching Puffs Time Format corrupts data, is not displayed correctly if set to HH:MM:SS 
       2) After switching from C to F in Regional, the set profile temperature should be converted and rounded. Changing from F to C, the set temperature scales down correctly, but not the other way around. Describe code error when found
       
- [x] Investigated GitHub Dependabot `glib::VariantStrIter` unsoundness vulnerability. Tauri 2.11.5 pins `gtk = "0.18"` / `webkit2gtk = "2.0"`, which transitively locks `glib 0.18.5`. Newer `glib`/`gtk` versions exist but Tauri does not yet declare compatibility; a `[patch.crates-io]` override would likely break Linux webview/gtk compilation. Fix is blocked on a future Tauri/gtk-rs release. No local Cargo.lock change committed.

- [x] M0 RE freeze: write `resources/re/af_190602-boot.md` + `resources/boot/af_190602.json` documenting the reset handler, SystemInit, C runtime startup, and main() dispatch loop for build af_190602.

- [x] Section 2 — full-boot emulation with per-PID dispatch (done 2026-09-18):
      plan `docs/superpowers/plans/2026-09-14-fullboot-pid-dispatch-emulation.md`;
      three compilation defects fixed (missing braces, bogus `use`, wrong
      `RETURN_SENTINEL` path); boot gate (`test_af_190602_boot_dispatch_by_pid`)
      now passes — all 9 known PIDs settle at dispatcher 0x0000d684, unknown
      PID XXXX hits the 0x2FF8 hang.  Key fix: bus write hook for 0x40040000
      models the hardware side-effect (sets bit 2 of RAM 0x20002C30) that
      unblocks the clock_pll_init polling loop at 0x17452.

- [x] Continue pursuing earlier todoss — boot gate (Section 2) completed,
      AppImage flicker follow-ups completed (2026-09-18).

- [x] Fix Firmware Editor syntax blocker (2026-09-12): commit `ebde818` shipped
      `src/renderer-firmware.js` with a duplicated `function setImagesVisible(hasHandle) {`
      signature (lines 1003–1004, second had no body) — one unmatched brace, node --check
      failed with "Unexpected end of input" at line 1757. Fixed minimally by deleting the
      duplicated line; the 87435d8 firmware-editor features (Status tab, image tools, patch
      batch actions, resource-pack preview) are preserved. A previously staged mass-revert
      (~518 lines) in the Documents mirror was discarded (backup: /tmp/staged-revert-backup.patch).
- [x] Flatpak local build fixed (2026-09-12): stale `build-dir/` (missing refs/) caused
      "opendir(refs/heads): No such file or directory"; after clearing build state the build
      then failed on `rofiles-fuse` (Permission denied on btrfs homed mount), worked around
      with `flatpak-builder --disable-rofiles-fuse`. Build of `flatpak/org.cloudy.af-local.yml`
      completed and committed to the local OSTree `repo/` (commit 57b212b…).

- [x] Version 1.2.0 release work (2026-09-12): merged remote 1.19.1 into the
      1.2.0 line (translations, tooltips, README, kept both sides' changelog
      jokes); bumped version; committed the never-committed
      `src-tauri/src/firmware/resources.rs`; fixed the FW editor's tab-switching
      TDZ abort (`currentTab` used before declaration via initTabs's
      synchronous initial click — every tab showed Status, and post-771 module
      bindings never ran); removed Save/Save As; moved Flash to Device into the
      footer; added shared keybinds (docs/keyboard-shortcuts.md); Puff Time now
      displays inside the field with device-default format (seconds fallback);
      i18n: 8 missing keys added to all 17 locales + locale-switch BCP-47 fix
      in renderer-firmware/tfr; Patches tab behind an untracked
      `local-flags.json` build flag (off on GitHub builds, on in ~/cloudy-af
      until feature testing passes).
- [x] Firmware Editor Strings/Resource Packs tabs (2026-09-12): T2 delegate
      (queue2) found no port was needed — refreshStringsTab (renderer-firmware.js
      :1369) and refreshResourcePacksTab (:1638) already exist from the preserved
      87435d8 feature set, with tab markup in firmware.html (:169,:188) calling
      the resources.rs backend (listStringsCmd/listResourcePacks). Remaining:
      functional test with a firmware fixture (glyph editor, pack preview,
      extract/inject round-trip vs NFirmwareEditor parity).
- [x] AppImage device-detection flicker (resolved 2026-09-18): root cause
      was two devices or two software instances connected simultaneously,
      not a code defect.  Follow-ups (renderer edge-detect, bridge
      first-failure guard) applied as defensive measures.
- [x] Build script disk space check (2026-09-18): added `check_disk_space()`
      function to `build.sh` that runs before each build (AppImage/Flatpak).
      Checks available disk space and offers to clear caches (pip, npm, cargo,
      flatpak-builder, etc.) when below minimum threshold. Supports `-y` flag
      for non-interactive mode.

