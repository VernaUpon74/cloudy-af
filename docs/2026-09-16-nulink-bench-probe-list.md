# Nu-Link bench probe session — emulator-to-silicon confirmation (2026-09-16)

Goal: one physical session with the Nu-Link (ZORZA 1Pcs Nu-Link clone,
3.3/5 V, ARM Cortex-M0/M4) on the Pico 25 to close every open question the
emulator surfaced. Everything below is ordered so each step's output feeds
the next, and every probe names what the emulator currently ASSUMES, what
silicon should show, and what to change if it differs.

Companion docs: `docs/superpowers/plans/2026-09-13-swd-hardware-hacking.md`
(wiring/OpenOCD procedure), `docs/wine-usb-passthrough.md` (Nu-Link under
Wine), `docs/validation-handoff.md` Task 7 (the emulator findings this list
was built from), `resources/animations/af_190624.json` (the new-build port).

## A. Safety (read twice before touching)

1. **3.3 V logic only.** The Nu-Link has a 5 V-capable rail; the M451 target
   is 3.3 V. NEVER connect the programmer's VCC to the target. Power the
   device from its own battery; connect only SWDIO, SWCLK, GND (nRST if
   found). No exceptions — a 5 V feed can kill the board.
2. The battery stays in (device powers itself); battery door open = unpowered
   for the continuity step.
3. The daily-driver Pico 25 is flashable and rescuable: the stock kit is in
   `test-fixtures/rescue/` (stock image + dataflash + README procedure). This
   session is READ-ONLY — never program anything during it.

## B. Wiring (15 min, multimeter, battery OUT)

1. Photograph both PCB sides; read the MCU top marking (expect Nuvoton
   M451-series). In the M451 datasheet for that package, note the pin numbers
   for ICE_DAT (SWDIO), ICE_CLK (SWCLK), nRST.
2. Battery out, device unpowered. Continuity-buzz from the MCU pins to
   candidate pads/vias on the PCB. Record the wiring table and tape it to the
   bench:

   | Signal | MCU pin | PCB point (measured) | Nu-Link header pin |
   |---|---|---|---|
   | SWDIO | (datasheet) | (measured) | SWDIO |
   | SWCLK | (datasheet) | (measured) | SWCLK |
   | GND | any GND | (measured) | GND |
   | nRST | (datasheet) | (measured, optional) | nRST |

   The Nu-Link's 5-pin header is: VCC (unused), SWDIO, SWCLK, GND, nRST —
   verify against the ZORZA product silkscreen. Leave VCC unconnected.
3. Nu-Link to the host PC via USB. On Linux it enumerates as
   `VID 0416 PID 511C` (NOT in the app's SUPPORTED_DEVICES, so the app and
   the uhid gate ignore it — keep the Pico's own USB cable out during
   flashing-related work anyway).

## C. Attach test (both tool paths; use whichever works first)

- **OpenOCD path** (if a ST-Link V2 is also at hand; generic SWD works on
  M451): `openocd -f interface/stlink.cfg -f target/nuvo_m451.cfg` — expect
  `SWD DPIDR` nonzero and the target listed; if DPIDR reads
  `0x00000000`/`0xFFFFFFFF`, SWDIO/SWCLK are swapped or the wiring table is
  wrong. STOP and re-buzz; do not "retry with VCC".
- **Nu-Link path**: NuLinkCommandTool (Linux) or the ICP Programming Tool
  (Windows/Wine, `docs/wine-usb-passthrough.md` — confirm the Nu-Link shows
  under Wine's device manager as a USB device with a lib32-compatible
  driver). If OpenOCD attached fine, prefer it for live memory reads; the
  ICP tool is the ground-truth reader for chip regions (APROM/LDROM/
  dataflash/config bits).

Milestone: device identified, haltable, readable. Nothing else matters
until this works.

## D. Probe list (READ-ONLY; record every value verbatim)

### D1. Which firmware is on the device right now (build ground truth)

- Read the dataflash via the app's normal 0x35 path first (no clips needed):
  fw version at dataflash user offset 256, build stamp at offset 516
  (`13 06 02` = af_190602; `13 06 24` = af_190624).
- Then, for the record, ICP-read the full APROM and compare the image
  against `AF_fw/decrypted/af_190602.dec.bin` / `af_190624.dec.bin`.
  **Why:** the animation descriptors now cover BOTH builds; this tells us
  which one this physical device actually runs.

### D2. FMC identity values (emulator dummies today)

The emulator serves FMC ISP READ_UID/CID/DID as dummies
(UID 0x13572468/…, CID 0xDA, DID 0x0D421000). Disasm shows the boot only
STORES them (globals at 0x20000DA4+0x10C..0x114 in af_190602), nothing
compares — but confirm via live reads:

- PDID (SYS, 0x4000_0000): read the 32-bit part ID (M451 expected — record
  the exact value; it goes into the emulator's DID dummy).
- If the FMC identity globals matter later, read RAM
  0x20000DB0..0x20000DB8 while halted after boot.

### D3. LDROM full dump + the SKU block (Task 7 root cause)

- ICP-read the whole LDROM (0x0010_0000..). The emulator assumes 16 KiB
  (fixture `test-fixtures/ldrom/ldrom_m041_16k.bin`, code in the first
  4 KiB, "M041" at 0x878, HIDC at 0x5C7).
- **Confirm:** the real LDROM length (8 KiB and 4 KiB are both possible M451
  configs), and byte-diff the dump against the 16 KiB fixture.
- **Why:** the boot SKU scan (fn 0x302C in af_190602) FMC-READs the first
  4 KiB of LDROM and matches words against ~60 device-ID literals ("M041"
  at literal 0x3324). If the physical LDROM differs from the dump anywhere
  in the first 4 KiB, update the fixture — the boot gate depends on it.

### D4. Dataflash true size (the 4 KiB window question)

- The 190602 SKU scan counts r5 to 0x1000 in +4 steps = a 4 KiB window
  starting at ISPADR 0x0010_0000 (LDROM) — the OLD dataflash hypothesis is
  dead, but the LDROM length check (D3) is what actually settles the window.
- ICP-read the dataflash (user area) separately: confirm whether the chip's
  DataFlash is 2 KiB (our dumps' size) or 4 KiB (configurable on M451 — read
  the config bits / CFG0 for DFEN + size). If 4 KiB: extend the boot test's
  `DATACFLASH_SIZE` and re-place the rescue dump.

### D5. Live framebuffer check (emulator vs silicon, both builds)

With the device showing the charge screen (plugged in, battery, screen on):

- Halt via SWD, dump 1 KiB at **0x2000_2758** (af_190602) or
  **0x2000_2740** (af_190624 — shifted −0x18).
- Expected: the packed 64×128 charge screen; on-pixel popcount **409**
  (the emulator's golden for both builds). Record the raw dump as a golden
  artifact under `AF_fw/` if it matches; if the popcount differs, that is a
  REAL finding — capture a screen photo alongside for the handoff.

### D6. Phase-global spares (the one free emulator assumption left)

The cave bumps a phase counter at RAM **0x2000_2CF8** (af_190602) /
**0x2000_2CE0** (af_190624) — chosen as zero-literal spare words. Confirm on
silicon, device running for a minute:

- Read the 16 bytes at 0x2000_2CF0..0x2000_2D00 repeatedly (3 samples, 5 s
  apart). The chosen spare must stay **constant**; if it is actually a live
  variable, the cave's phase counter would race with firmware state: pick
  the next spare from the emulator's 0-literal candidate list and re-emit
  the descriptor.

### D7. Config byte cell (patch storage)

- Read dataflash offset **0x7F0** (= flash 0x1F7F0 in the combined layout):
  expected stock value 0 or 0xFF (erased/unused). This is where the
  animation config byte lives; if the stock firmware ever writes it, the
  patch gate (`image_supports_animation`) needs a stock-value allowance.

### D8. 0x4005_0000 block (the §2 boot runaway's territory)

The §2 boot emulation runs ~212K instructions then strays at this block
(accessor fns 0xA0C-0xA56 in af_190602; struct offsets +0x20/+0x1000/
+0x1020). On silicon, halted mid-run:

- Read 0x4005_0000..0x4005_0020 and 0x4005_1000..0x4005_1030 — record raw
  values and (if possible) observe while the device is idle vs busy.
- Check the M451 TRM for which peripheral sits at 0x4005_x000 and note it in
  the handoff. This closes the last decode-gap hunt with facts instead of
  inference.

### D9. JWEI marker (build-identity gate)

- Read APROM bytes at **0x1BF13..0x1BF16** — expect ASCII "JWEI" (the SKU
  scan's gate; identical offset in BOTH 19.06.x builds). Confirms the ICP
  read is byte-aligned with the decrypted images.

## E. Recording (do this before leaving the bench)

For every probe: a one-line entry in `docs/validation-handoff.md` (Task 8)
with the raw value. Dumps (LDROM, framebuffer, 0x4005_xxxx) go under
`AF_fw/` with the date in the filename. Anything that contradicts an
assumption above becomes the next emulator fix with its silicon citation.

## F. After the session (back at the desk)

1. Update the emulator dummies/values that silicon corrected (D2/D3/D4/D8).
2. Re-run `firmware::anim` + `emu_test --include-ignored` and record any
   signature changes.
3. If D5 matched: the emulator's framebuffer rendering is silicon-verified —
   the screenshot-GIF pipeline (`anim_shots`) is now a trustworthy preview of
   on-device animation for both af_190602 and af_190624.

