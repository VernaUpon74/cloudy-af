# Goals

Loose ends carried over from the firmware read-back Phase 1 plan
(`docs/superpowers/plans/2026-08-19-firmware-readback-phase1.md`, completed
2026-08-27, gate B).

- [ ] Commit the (currently untracked) plan file
      `docs/superpowers/plans/2026-08-19-firmware-readback-phase1.md` as
      `docs: firmware read-back phase 1 plan`.
- [ ] Investigate the `screenshot()` HID read timeout in
      `test_check_state_hardware` (`src-tauri/src/firmware/tests/flasher_test.rs`) —
      the version/product-id reads succeed but the 1024-byte screenshot read
      times out on stock v1.00.
- [ ] Phase 2 — stock library & pipeline: use `flasher::read_fw_version()`
      to match the device against bundled stock builds (see
      `docs/superpowers/specs/2026-08-19-firmware-animation-pipeline-design.md`,
      Phase 2). Gets its own plan.
