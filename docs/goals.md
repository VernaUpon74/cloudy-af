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

- [ ] We already tried the plan in ~/cloudy-af/.github/workflows/macos-build.yml, address Github Actions macOS build error and the vulnerability Github found- context in new files in '~/cloudy-af/docs/'
      Describe errors when found
      
      **macOS build error found (2026-09-11):** `npm ci` fails during `sidecar:build` / `patch-package` because the runner's working tree contained an `arcticfox+11.0.4.patch` that did not match the installed `arcticfox@11.0.3` (`arcticfox@11.0.3 ✔` applied, then `arcticfox+11.0.4.patch` failed). Local tree has already been moved to `arcticfox@^11.0.3` + `arcticfox+11.0.3.patch`; `npm install --dry-run` in `sidecar/` now applies both patches cleanly. Next step is to push and re-run the workflow to confirm.
      
      **GitHub vulnerability found (2026-09-11):** Dependabot alert #1 — unsound `Iterator`/`DoubleEndedIterator` impls for `glib::VariantStrIter` in `src-tauri/Cargo.lock`. Current locked version is `glib 0.18.5`, which Dependabot reports as the latest possible version in the 0.18.x line. `glib` is pulled in transitively by the gtk-rs stack (`gtk`, `gdk`, `cairo-rs`, `gio`, `webkit2gtk`) used by Tauri v2.11.5. Project code does not directly use `VariantStrIter`. Fixing it likely requires upgrading the gtk-rs ecosystem / Tauri to a version that pulls `glib >= 0.19.x`.
      **Update 2026-09-12:** `cargo search` shows Tauri 2.11.5 is still the latest release and its `Cargo.toml` pins `gtk = "0.18"` and `webkit2gtk = "2"`, which in turn pin `glib 0.18.5`. A newer `glib` (0.22.9) and `gtk` (0.19.0) exist on crates.io, but Tauri does not yet declare compatibility with them. Forcing an override with `[patch.crates-io]` would likely break Tauri's Linux webview/gtk integration at compile time. This vulnerability is therefore blocked on a future Tauri/gtk-rs release; recommend monitoring and upgrading once Tauri supports gtk 0.19+/glib 0.19+.

- [x] Address these two format errors- 1) switching Puffs Time Format corrupts data, is not displayed correctly if set to HH:MM:SS 
       2) After switching from C to F in Regional, the set profile temperature should be converted and rounded. Changing from F to C, the set temperature scales down correctly, but not the other way around. Describe code error when found

- [ ] Continue pursuing earlier todoss
