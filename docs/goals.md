# Goals

Loose ends carried over from the firmware read-back Phase 1 plan
(`docs/superpowers/plans/2026-08-19-firmware-readback-phase1.md`, completed
2026-08-27, gate B).

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
- [ ] Screenshot capture quirks on AF: framebuffer reads all-zero when the
      display is asleep (wake via button press and capture immediately);
      PNG packing in `test_screenshot_hardware` renders garbage at the top —
      revisit in Phase 3 (screenshot verification).
- [ ] Device Monitor: replicate NToolbox's live device-monitoring window
      (telemetry readouts). The firmware emulation harness
      (`src-tauri/src/firmware/emu/`, plan
      `docs/superpowers/plans/2026-08-28-firmware-emulation-harness.md`) is
      intended to help replicate its rendering. NToolbox runs locally for
      reference via Wine — see `docs/wine-usb-passthrough.md`.
- [ ] Emulation harness gate: layers 1–3 done (plan
      `docs/superpowers/plans/2026-08-28-firmware-emulation-harness.md`).
      Layer 4 (`test_af_190602_render_gate`) unblocks when the Phase 3 RE
      produces `resources/animations/af_190602.json`; layer 5 (effect golden
      frames + differential vs Rust reference math) lands with the Thumb
      bytecode builder in the animation-effects work.
- [x] Phase 2 — stock library & pipeline: use `flasher::read_fw_version()`
      to match the device against bundled stock builds (see
      `docs/superpowers/specs/2026-08-19-firmware-animation-pipeline-design.md`,
      Phase 2). (done 2026-08-27: four bundled builds + devices.json with
      matcher; af_190602 fw_versions=[110] observed on the Pico; apply →
      flash → undo hardware cycle passes in `test_stock_cycle_hardware`.)
