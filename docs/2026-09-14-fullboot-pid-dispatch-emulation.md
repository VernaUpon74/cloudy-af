# Full-Boot PID-Dispatch Emulation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Hardware tasks in this plan:** none. This plan is pure-emulation; all steps are implemented and run in Rust inside `src-tauri`. No physical device is required.

**Goal:** Extend the existing Thumb-2 emulator harness to boot the *whole* patched stock image from the reset vector with a synthetic dataflash, feed each known product ID (M041, M065, M077, M177, …) through the real boot dispatch, and assert the right panel/dispatcher path is reached and execution never hits the unknown-PID infinite loop (`bl 0x2FF8`). This closes the highest-value gap in the non-hardware validation story: the exact failure that hung the Pico Dual was a runtime dispatch problem, not an image problem, and it is invisible to the current render-only harness.

**Architecture:** The existing `Harness` starts the CPU at `desc.render_entry` and stops at `RETURN_SENTINEL`. For §2 we add a *boot* entry point: set PC to the reset vector from `resources/boot/af_190602.json`, SP to `initial_sp`, model the MMIO stubs listed in that descriptor, and — critically — make dataflash reads (region `0x1F000..0x1F7FF`) return a synthetic dataflash whose PID at offset 316 is whatever we want to test. We then run until either (a) PC reaches the dispatcher/render path (success) or (b) PC hits `0x2FF8` (the documented unknown-PID hang). The cleanest implementation grows the in-memory image to cover `0x00000..0x1F7FF` (firmware + APROM tail padding + dataflash), so the existing `Bus::read` flash path already serves dataflash reads without new callback plumbing.

**Tech Stack:** Rust (existing emulator in `src-tauri/src/firmware/emu/`), the decrypted stock image `AF_fw/decrypted/af_190602.dec.bin` (gitignored RE artifact, 116,716 B), `resources/boot/af_190602.json` (boot descriptor with reset vector, init addresses, MMIO stubs, memory map), `src-tauri/src/firmware/tests/emu_test.rs` (where the new `#[ignore]` gate lives). No new crates — toolbox cargo is offline.

**Spec:** `docs/firmware-validation.md` §2 (the plan this implements), `resources/re/af_190602-boot.md` (reset/systeminit/__main/main disassembly), `resources/re/af_190602-dispatcher-analy.md` (dispatcher `0xd684`, status word, hang target `0x2FF8`), `resources/boot/af_190602.json` (boot descriptor consumed by the new harness mode), `src-tauri/src/firmware/flasher.rs` §"Screen-size ground truth" (PID→panel table mapping, the `bl 0x2FF8` hang).

## Global Constraints

- Toolbox cargo is OFFLINE — no new Rust crates. Implementation uses only existing deps (`serde`, `thiserror`, etc.).
- New test is `#[ignore]`d, matching the existing layer-4/5 gate convention (`cargo test --offline --release -- --ignored`).
- RE artifacts (`AF_fw/decrypted/af_190602.dec.bin`, goldens) stay gitignored; the new test skips gracefully when the decrypted image is absent.
- Dataflash is NOT in the stock image (image ends at `0x1C7EC`; dataflash lives at `0x1F000..0x1F7FF` in the MCU memory map). The test must synthesize a combined image (firmware + padding + dataflash) or otherwise serve dataflash reads. The plan chooses the combined-image approach (see Task 1).
- The boot path is long (reset handler → SystemInit → `__main` → `main` → init calls → dataflash load → dispatcher). Budget must be sized to get through init (expect tens of thousands of instructions) but still terminate on the success/failure signal. The success signal is *reaching the dispatcher* (PC in the `0xd684` neighborhood or the render entry `0x8cd1`); the failure signal is *hitting `0x2FF8`*. We do NOT run the full UI loop — we stop as soon as dispatch resolves.
- Brick safety: this is pure emulation; there is no device and no write path. No brick safety constraints apply beyond the normal "don't flash real devices from test code" rule, which is already enforced by the `#[ignore]` gate and the existing `test_flash_uhid_double` guard.

## Known RE facts (do not re-derive)

- Reset vector: `0x00001659` (Thumb entry; reset handler at `0x1658`). Initial SP: `0x200031C0`.
- Reset handler `0x1658..0x1688`: unlock REGWRPROT, unlock protected regs, call SystemInit then `__main`.
- SystemInit `0x16A0..0x16AA`: enable FPU (CPACR `0xE000ED88`, OR in `0x00F00000`).
- `__main` `0x16B2..0x16C3`: reload SP, copy `.data` from flash `0x1C794` → RAM `0x20000000`, zero `.bss` `0x20000058`, branch to `main` at `0x1834D`.
- `main` `0x1834C`: init calls (clock, GPIO, ADC, HID/USB, display, timer, interrupts, dataflash load), then dispatcher loop. The dispatcher is `FUN_0000d684` at `0xD684`.
- Dataflash-load function at `0x16C64` (role `dataflash_load_settings`): reads the dataflash region; PID lives at dataflash offset 316 (`df[316..320]`, ASCII). In the MCU memory map dataflash is at `0x1F000`, so PID byte 0 is at `0x1F13C`.
- Unknown/erased PID → dispatch does not match → firmware hangs in an infinite loop at `0x2FF8` (`push.w {r4-r7, lr}`; `movs r2, #1`; write flag; `b 0x3004` loop). Hitting `0x2FF8` = failure.
- Known PIDs and their panel geometries (from `flasher.rs`): 64×128 default/fallback (classic Joyetech family), 96×16 (Pico M041, Pico Dual M065, …), 128×32 (Pico 25 M077, …), 64×48 (Wismec RX family, …), 64×32 (Pico Squeeze 2 M105, ASTER RT M064). PID → panel is runtime-selected; there are no per-screen firmware images.
- Decrypted image size: 116,716 B (`0x1C7EC`). Dataflash size: 2,048 B. Combined image for the test: `0x1F800` B (firmware `0x00000..0x1C7EB`, APROM tail/gap `0x1C7EC..0x1EFFF` padded `0xFF`, dataflash `0x1F000..0x1F7FF`).
## Task 1: Combined-image builder + dataflash synthesis helper

**Files:**
- Modify: `src-tauri/src/firmware/tests/emu_test.rs` (add helpers + the gate)
- Consumes: `AF_fw/decrypted/af_190602.dec.bin` (when present), `resources/boot/af_190602.json`
- Produces: in-memory combined image used by the boot harness; no on-disk artifact required (the test is self-contained)

- [ ] **Step 1: Add a `make_combined_image` helper**

In `emu_test.rs`, add a helper that builds the combined image:

```rust
/// Build a combined firmware+dataflash image suitable for booting from reset.
/// The stock firmware (0x1C7EC bytes) is laid out at 0x00000..0x1C7EB,
/// the APROM tail / gap (0x1C7EC..0x1F000) is padded with 0xFF, and the
/// dataflash (2048 B) is placed at 0x1F000..0x1F7FF. Returns the image and
/// the offset within it of dataflash byte 0 (should be 0x1F000).
fn make_combined_image(firmware: &[u8], dataflash: &[u8; 2048]) -> Vec<u8> {
    let mut img = vec![0xFFu8; 0x1F800];
    // firmware
    img[..firmware.len()].copy_from_slice(firmware);
    // dataflash at 0x1F000
    img[0x1F000..0x1F000 + 2048].copy_from_slice(&dataflash[..]);
    img
}
```

- [ ] **Step 2: Add a `make_dataflash` helper**

```rust
/// Build a 2048-byte dataflash with the given 4-byte product ID at offset 316
/// (DF offset 316 = address 0x1F13C) and a sane default for the rest
/// (checksum 0, boot flag 0, fw version 110 at offset 256 — matching the
/// AF_190602 observation in goals.md). Offsets beyond the PID are zero.
fn make_dataflash(pid: &[u8; 4]) -> [u8; 2048] {
    let mut df = [0u8; 2048];
    // fw version 110 at dataflash offset 256 (observed on the Pico: fw_versions=[110])
    df[256..260].copy_from_slice(&110u32.to_le_bytes());
    // boot flag 0 (boot into APROM / normal runtime)
    df[9] = 0;
    // PID at offset 316
    df[316..320].copy_from_slice(pid);
    df
}
```

- [ ] **Step 3: Wire the boot MMIO stubs from the boot descriptor**

The boot descriptor `resources/boot/af_190602.json` already lists `mmio_stubs`:

```json
"mmio_stubs": [
  { "addr": "0x40000100", "value": 0, "note": "REGWRPROT: reads as locked after reset" },
  { "addr": "0x40000024", "value": 0, "note": "protected register stub" },
  { "addr": "0x40000200", "value": 0, "note": "protected register stub" },
  { "addr": "0xe000ed88", "value": 0, "note": "CPACR: no coprocessor access before SystemInit" }
]
```

The existing `Harness::new` already calls `bus.set_stub(addr, value)` for each stub in `desc.stubs`. For the boot test we will load the boot descriptor and pass its `mmio_stubs` into the harness (the boot descriptor's `render` sub-object contains the same `render_entry`/`display_buffer`/`ram_globals`/`args` the existing harness expects — see Task 2).

NOTE: The reset handler WRITES to these stubs (unlock sequences). The current `Bus::set_stub` makes writes a no-op (dropped) and reads return the stubbed value. That is exactly what we want for these MMIO regs — the unlock writes are dropped, the reads return the stubbed value, and the reset handler proceeds. No new plumbing needed for the listed stubs.
## Task 2: Boot-from-reset harness mode

**Files:**
- Modify: `src-tauri/src/firmware/tests/emu_test.rs`

- [ ] **Step 1: Add the boot gate skeleton**

Add a new `#[ignore]` test `test_af_190602_boot_dispatch_by_pid` that:

1. Skips if the decrypted image or boot descriptor is absent.
2. Loads the decrypted stock image and the boot descriptor.
3. For each PID under test (start with M041, M077, plus one unknown), builds a dataflash and a combined image.
4. Constructs a `Harness` from the combined image + boot descriptor (the boot descriptor's `render` sub-object provides `render_entry`, `display_buffer`, `ram_globals`, `args`; its `mmio_stubs` provide the MMIO stubs; its `processor.initial_sp` and `reset_handler`/… provide the boot addresses — see Step 2 below for how to plumb these).
5. Sets PC to the reset vector, SP to `initial_sp`, LR to a safe value (the reset handler will `bx __main` eventually; we can set LR to `RETURN_SENTINEL | 1` as the existing harness does, since the boot path returns to `main` which loops forever — we will stop on the success/failure signal instead of waiting for a `bx lr` to `RETURN_SENTINEL`).
6. Runs with a large budget (e.g. 1,000,000 instructions) until either PC hits `0x2FF8` (failure) or PC reaches the dispatcher/render neighborhood (success). Record which happened.

- [ ] **Step 2: Plumb the boot descriptor into the harness**

The cleanest approach: add a *second* constructor or a configuration flag to `Harness` that takes the reset vector + initial SP. Two options:

Option A (minimal): In the test, after `Harness::new`, override `h.cpu.pc` and `h.cpu.sp` directly and set `h.cpu.lr` — the existing `run_frame` sets these anyway, so for the boot test we write a custom `run_boot` helper in the test file that does the same `cpu.run_until(bus, stop_pc, budget)` loop but with a *dynamic* stop condition (success PC range OR `0x2FF8`). This avoids touching `harness.rs` at all for the first cut.

Option B (cleaner long-term): Add a `Harness::boot` constructor or a `BootConfig` to `harness.rs`. Prefer Option A for the first implementation to keep the change localized; refactor into `harness.rs` only if the test code becomes unwieldy.

For the success PC range: the dispatcher is at `0xD684`. The render entry (charge-screen composer inner body) is at `0x8CD1`. After a successful dispatch, `main` calls into the dispatcher which eventually calls the render entry. For the test, "success" = PC reaches any of `{0xD684, 0x8CD1}` or enters the dispatcher loop neighborhood. Concretely, stop when `pc == 0x2FF8` (fail) or `pc` is in `[0xD684, 0xD684 + 0x100]` or `pc == 0x8CD1` (success). Refine the range after the first run reveals where the dispatcher actually lands.

- [ ] **Step 3: First run with M041**

Run the test with PID `b"M041"` and print the final PC + a CPU debug dump. Expected: PC reaches the dispatcher/render neighborhood, NOT `0x2FF8`. If it hits `0x2FF8` or budget-exceeded, inspect the debug dump to find what MMIO access failed and add the needed stub.

- [ ] **Step 4: Run with unknown PID**

Run with PID `b"XXXX"` (or `b"\xFF\xFF\xFF\xFF"`). Expected: PC hits `0x2FF8`. If it instead reaches the dispatcher, the dispatch is more lenient than expected — investigate.

- [ ] **Step 5: Run with erased PID**

Run with PID `b"\xFF\xFF\xFF\xFF"`. Expected: PC hits `0x2FF8` (same as unknown).


- [ ] **Step 4: Verify the dataflash address math**

dataflash byte 0 is at MCU address `0x1F000`. PID byte 0 is at dataflash offset 316, i.e. MCU address `0x1F000 + 316 = 0x1F13C`. In the combined image, that is `img[0x1F13C]`. Assert in a tiny test that `make_combined_image(fw, df)[0x1F13C..0x1F140] == b"M041"` when `df` was built from `b"M041"`.


## Task 3: Per-PID dispatch matrix

**Files:**
- Modify: `src-tauri/src/firmware/tests/emu_test.rs`

- [ ] **Step 1: Parameterize over the known PIDs**

Extend the test to cover the known ArcticFox PIDs from `flasher.rs`:

| PID | Panel | Notes |
|-----|-------|-------|
| M041 | 96×16 | Pico |
| M065 | 96×16 | Pico Dual |
| M077 | 128×32 | Pico 25 |
| M177 | (STM32 line) | Rim C family (different MCU — may not boot in this Nuvoton image; see Step 3) |
| M045 | 96×16 | Pico Mega |
| M038 | 96×16 | Pico RDTA |
| M037 | 96×16 | ASTER |
| M095 | 64×48 | Invoke |
| M105 | 64×32 | Pico Squeeze 2 |
| M064 | 64×32 | ASTER RT |

For each known PID (except M177 — see Step 3), assert the boot reaches the dispatcher/render neighborhood and does NOT hit `0x2FF8`.

- [ ] **Step 2: Assert the panel geometry is selectable**

For at least M041 (96×16) and M077 (128×32), after boot succeeds, read the display buffer geometry the firmware selected and assert it matches the expected panel (96×16 for M041, 128×32 for M077). The display buffer geometry is runtime-selected by the dispatch; if the emulator reaches the dispatcher with the right PID, the display buffer setup should reflect the right geometry. The exact RAM/MMIO location of the selected geometry is an RE detail to nail in Step 3; for the first cut, asserting "reached dispatcher" is the primary signal.

- [ ] **Step 3: M177 / STM32 line — mark as expected-failure or skip**

M177 is an STM32-line device (different MCU family, per `flasher.rs` and the SWD plan). The `af_190602` image is a Nuvoton M451/M471 image. Booting M177's PID in the Nuvoton image is not a meaningful test — the dispatch may reject it (hit `0x2FF8`) or may misbehave. For the matrix, run M177 and record the outcome; do NOT assert success. If it hits `0x2FF8`, that's expected (unknown PID in this image). Document the result in the test comments.

- [ ] **Step 4: Unknown/erased PID matrix**

Add explicit rows for:
- `b"XXXX"` (unknown ASCII PID)
- `b"\xFF\xFF\xFF\xFF"` (erased PID)
- `b"\x00\x00\x00\x00"` (null PID)

For each, assert PC hits `0x2FF8` (the documented hang). These are the negative cases that validate the test actually catches the failure mode.

## Task 4: Watchdog servicing assertion (stretch — do after Task 3 if time permits)

**Files:**
- Modify: `src-tauri/src/firmware/tests/emu_test.rs` (or a new test file)

- [ ] **Step 1: Identify the watchdog register**

The Nuvoton M451 has a watchdog (IWDG-style or M451-specific WDT). Find the watchdog feed address from the RE (search the decrypted image for the watchdog feed sequence, or consult the M451 reference manual). The boot descriptor's MMIO stubs do not currently include a watchdog reg — add it if needed.

- [ ] **Step 2: Assert the fade loop services the watchdog**

In the existing layer-5 gate (`test_af_190602_animation_frames`), the patched firmware runs the fade loop via the cave. Add an assertion that the watchdog feed address is written to during the fade loop (i.e., the cave or the surrounding dispatcher code feeds the watchdog). This validates that a patched image whose config byte selects an effect still services the watchdog — one of the explicit scope items in `firmware-validation.md` §2.

If the watchdog address is not yet known, mark this task `[ ]` pending RE and move on.

## Task 5: Doc bookkeeping

**Files:**
- Modify: `docs/firmware-validation.md` (§2 status ⬜ → ✅ with entry point)
- Modify: `docs/goals.md` (add completed entry referencing this plan + the layer-6 boot gate)

- [ ] **Step 1: Update `docs/firmware-validation.md` §2**

Mark §2 ✅. Add the entry point (`test_af_190602_boot_dispatch_by_pid` in `emu_test.rs`), list the PIDs covered, note the success/failure signals (reaches dispatcher vs hits `0x2FF8`), and note any PIDs skipped (M177/STM32 line) with the reason.

- [ ] **Step 2: Update `docs/goals.md`**

Add a completed entry: "Firmware validation §2 — full-boot per-PID dispatch emulation gate (`test_af_190602_boot_dispatch_by_pid`): boots the stock image from reset with a synthetic dataflash for each known PID (M041, M065, M077, M045, M038, M037, M095, M105, M064) and asserts the dispatcher is reached without hitting the `0x2FF8` unknown-PID hang; negative cases (XXXX, 0xFFFFFFFF, 0x00000000) correctly hit the hang. M177/STM32 line skipped (different MCU family)."

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/firmware/tests/emu_test.rs docs/firmware-validation.md docs/goals.md
git commit -m "test(emu): full-boot per-PID dispatch gate (validation §2) — known PIDs reach dispatcher, unknown PIDs hit 0x2FF8 hang"
```

## Self-Review notes (run at plan completion)

- Spec coverage: firmware-validation.md §2 (per-PID dispatch, unknown-PID hang, watchdog stretch). Boot descriptor coverage: reset handler, SystemInit, __main, main init calls, dataflash load, dispatcher — all from `resources/boot/af_190602.json` + `resources/re/af_190602-boot.md`.
- MMIO stub coverage: the boot descriptor's `mmio_stubs` list (REGWRPROT, two protected regs, CPACR) is consumed by the harness. Any *additional* MMIO accesses the boot path hits (GPIO, ADC, USB, display, timer, …) will surface as `EmuError::Unmapped` on first run and must be stubbed then — that is expected and is the normal discovery process. Document each added stub with the address and the reason.
- Dataflash coverage: the combined-image approach means dataflash reads are served by the existing flash read path. The test synthesizes the PID at offset 316; the rest of dataflash is zero (or 110 at fw version offset 256). If `dataflash_load_settings` reads other dataflash offsets and behaves differently with non-zero values, the test may need a richer synthetic dataflash — that surfaces on first run.
- Budget: if the boot path exceeds the budget before reaching the dispatcher, increase the budget and/or add a progress print of the current PC every N instructions so a hung path can be diagnosed. The success/failure signals (dispatcher PC vs `0x2FF8`) should be reached well within a modest budget if the stubs are correct.
- M177/STM32: documented as skipped/expected-failure. Do not assert success for M177 in the Nuvoton image.
- This gate validates the *dispatch* path only. It does not replace the §1–§4 animation validation (effect property tests, goldens, static cave audit) — those remain the primary animation coverage. The §2 gate's value is specifically that it tests the dispatch failure mode that hung the Pico Dual, which no other gate covers.
