# Firmware validation pathways

How the animation-patch firmware (and flasher changes) are validated without
risking daily-driver devices, ordered by implementation cost. Status marks
what exists today.

## 1. Effect property tests — pure Rust ✅ (exists, being extended)

`src/firmware/anim/effects.rs::tests` runs the three fade references
(gradient / center / diagonal) as pure functions and asserts invariants:
phase periodicity, band movement, centre-vs-edge behaviour, cave-vs-reference
equality inside the Thumb-2 emulator, and gated-off passthrough.

Extension in progress (ollama-drafted, human-reviewed): a **parametric panel
geometry sweep** over the known ArcticFox geometries — 64×128 (1024 B),
96×16 (192 B), 128×32 (512 B), 64×48 (384 B), 64×32 (256 B) — asserting no
panics, buffer-length invariance, and that effects only ever clear whole
bytes. Note: the cave index math (`i >> 6`, `i & 63`, `^7`) is hardwired to
the 64×128 framebuffer, so these tests *document* that the effects are
64×128-only today; supporting small screens means deriving the index math
from width/stride, not just testing it.

## 2. Full-boot emulation with per-PID dispatch ⬜ (highest value next)

The repo's Thumb-2 emulator (`src/firmware/emu`) already steps the emitted
cave. Extend it to boot the *whole* patched image with a synthetic dataflash:

- Feed each known product ID (M041, M065, M077, M177, …) through boot
  dispatch (`0x302C` → class `0x20002C2E` → model `0x20002750` → panel init
  table) and assert the right panel table is reached and execution never
  hits the unknown-PID infinite loop (`bl 0x2FF8`).
- This tests *exactly* the failure that hung the Pico Dual — a runtime
  dispatch problem, not an image problem — with zero hardware risk.
- With a watchdog timer model, assert a patched image whose config byte
  selects an effect still services the watchdog during the fade loop.

## 3. Deterministic framebuffer differencing ⬜

Drive the timeout/clock path in the emulator with synthetic state (clock
digits, battery count 1 vs 2), run the fade window patched-vs-stock, and
diff framebuffers. Only the fade window may differ; everything after must
converge byte-exact. Golden-file the per-effect frames.

## 4. Static hook/cave audit ⬜ (partially covered by tests)

Disassemble the patched image at hook + cave (capstone): AAPCS register/stack
discipline, writes bounded to the framebuffer RAM range, bounded loops, clean
return, patched bytes confined to declared offsets, rollback byte-exact.
The existing `test_build_patch_layout` / `test_all_builders_fit_cave` cover
offset confinement and cave fit; register/flag restoration is covered by the
emulator tests (`assert_cave_matches_reference`).

## 5. Virtual-HID flasher end-to-end ⬜ (CI gate for flasher changes)

Linux `uhid` lets userspace implement a fake HID device. A test double
emulating the Nuvoton LDROM updater — including NAK-when-busy and the ragged
final-chunk death observed on the Pico Dual — would let CI exercise
`write_firmware_stream` (512-row padding, retries, progress) and the
recovery-wait fallback against a device that cannot brick.

## 6. usbmon differential capture — free, no new hardware ✅ (method)

`usbmon` + Wireshark records every HID report host-side. Capture NToolbox's
firmware-update traffic on a live device as the reference oracle, diff our
flasher's traffic against it. Would have caught the final-chunk bug
immediately. Use whenever the flasher protocol is touched.

## 7. Hardware confirmation ladder (real devices, minimal risk)

1. **Sacrificial mule**: a used original iStick Pico (M041, 96×16 panel —
   same family as the Pico Dual) as the dedicated flash-test target, so
   daily devices stay out of the loop. LDROM makes it effectively
   unbrickable, and the recovery-wait fallback (1.18.3) self-recovers even
   mid-flash failures.
2. **SWD probe** (optional, ~$15 ST-Link clone + OpenOCD): the Nuvoton MCU
   exposes SWD on test pads — read the framebuffer at `0x20002758` while an
   effect runs and compare pixel-for-pixel against emulator output. The
   gold-standard cross-check if emulation and reality ever disagree.

## Screen-size ground truth

From NFE v190718 + af_190602 disassembly (see the table in
`src-tauri/src/firmware/flasher.rs` above `read_product_id`): ArcticFox is
one universal binary per MCU line; screen geometry is runtime-selected by
the dataflash product ID at offset 316 (`df[316..320]`). **There are no
per-screen firmware images to bundle.** The flasher's obligations are: keep
the dataflash PID intact across flashes, and offer a "Force PID" repair
(rewrite `df[316..320]` + reboot, as NToolbox does) when a flash leaves it
erased — the only dataflash write a flasher should ever do.

## Device USB operation modes (Pico Dual repair findings, 2026-09-10)

The same USB VID/PID (0416:5020) serves both boot stages — identify the
mode by the iManufacturer string, not the IDs:

| Mode | iManufacturer | Serial | HID commands | Purpose |
|---|---|---|---|---|
| APROM (AF running) | `Joyetech` | dataflash serial (e.g. A02014090304) | full AF config protocol | normal settings operation |
| LDROM (updater) | `Nuvoton` | baked-in updater serial (A02015081302) | `0x35` / `0xB4` / `0xC3` only | firmware flashing |

Operational notes (verified on the repaired Pico Dual):

- **"Nuvoton HID Transfer" is the expected state for firmware/software
  flashing operations**, not an error. After a successful `0xC3` firmware
  write the LDROM clears the boot flag (`df[13]` → 0) but stays in LDROM;
  the next `0xB4` restart boots APROM (enumerates as `Joyetech`).
- APROM enumerating for ~2 s then reverting to `Nuvoton` means something
  re-armed the boot flag (a flash flow) — an MCU crash alone cannot, since
  the flag lives in dataflash.
- The dataflash info block (raw offsets in the 2048-byte `0x35` read, which
  is `[4-byte LE checksum][2044 bytes user data]`): fw version at 260, PID
  at 316..320, boot flag at 13. AF migrates a stock config in place on
  first boot (bumps version, keeps the PID slot); a healthy AF dataflash
  has a valid checksum, PID at 316, and the build stamp (e.g. `13 06 02`
  for 19.06.02) at offset 516.
- The boot-flag-1 wait + plain-`af_190602` flash + Force-PID repair
  sequence restored a Dual that had been left with stock Pico 25 (M077)
  firmware and a rewritten dataflash identity.
