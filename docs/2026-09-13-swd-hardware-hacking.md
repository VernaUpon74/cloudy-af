# SWD Hardware Hacking (Nu-Link + ST-Link) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Hardware tasks are executed by the human at the bench; the agent prepares all software/config/scripts and interprets outputs.**

**Goal:** Use the newly acquired Nu-Link (Nuvoton ICP/SWD) and ST-Link V2 (STM32 SWD) to obtain ground-truth flash dumps, live framebuffer reads, and an unbrickable recovery path — advancing the custom animated firmware goals (charging-screen animations) from emulator-verified to hardware-verified.

**Architecture:** Two independent SWD paths. (1) **ST-Link V2 + OpenOCD** provides generic ARM Cortex-M SWD memory access (`mdw`/`mww`) on *both* target families — the Nuvoton M451 in the iStick Pico / Pico 25 and the STM32 in the Rim C — because reading live RAM needs no vendor flash driver. This is the "SWD probe" already scoped in `docs/firmware-validation.md` §7.2. (2) **Nu-Link + Nuvoton ICP tooling** provides vendor-grade APROM/LDROM/config-bit read, program, and offline programming on the Nuvoton devices — full flash ground truth and a rescue path stronger than LDROM-over-HID.

**Tech Stack:** OpenOCD (host), Nuvoton Nu-Link CLI / NuLinkCommandTool (Linux) or ICP Programming Tool (Windows via Wine, cf. `docs/wine-usb-passthrough.md`), multimeter + soldering for pad access, existing repo emulator (`src-tauri/src/firmware/emu`) as the comparison oracle.

**Spec:** `docs/firmware-validation.md` (esp. §7 "Hardware confirmation ladder"), `docs/goals.md`, `resources/re/af_190602-render.md` (framebuffer `0x20002758`–`0x20002b58`), `resources/re/af_190602-dispatcher-analy.md` (status word, hook site). Tools: `~/cloudy-af/ZORZA …Nu-Link….html`, `~/cloudy-af/Amazon.com_ AITRIP …ST-Link V2….html`.

## Global Constraints

- **Brick safety first:** before ANY write/erase on a device, that device must have (a) a full APROM+config dump archived in `test-fixtures/rescue/<device>/` and (b) a verified restore path. LDROM rescue kit for the Pico already exists (`test-fixtures/rescue/`).
- **Sacrificial mule rule:** first contact is ALWAYS the sacrificial iStick Pico (M041), never a daily driver (Pico 25 / M077). Daily devices only after the mule survives the full read→write→restore cycle.
- **3.3 V logic only:** both programmers are 3.3/5 V; targets are 3.3 V. NEVER connect the programmer's 5 V rail to the target. Power the target from its own battery; connect SWDIO, SWCLK, GND (and nRST where available) only. Do NOT feed VCC from the programmer into the device.
- **Hot-plug order:** GND first, signal lines, then target power/battery last; reverse on disconnect.
- One local ollama delegate at a time for drafting/review (AGENTS.md); review its output before use.
- App version bump rules (AGENTS.md) apply only if README/CHANGELOG/docs change — this plan's doc-only updates (firmware-validation.md, goals.md) do NOT require a bump.

## Target Facts (from prior RE — do not re-derive)

| Fact | Value | Source |
|------|-------|--------|
| Pico / Pico 25 MCU | Nuvoton M451/M471-class Cortex-M4 | `resources/re/af_190602-boot.md` |
| Rim C MCU | STM32 (VID 0483, HID signature `5C CA 37 75`) | `sidecar/patches/arcticfox+11.0.3.patch` |
| Framebuffer | `0x20002758`–`0x20002b58` (1024 B, 64×128, horizontal MSB-first) | `resources/re/af_190602-render.md` |
| Display-status word | `0x20002c34` (bit `0x20000` = timed out, bit `0x80000` = screen-off path) | `resources/re/af_190602-dispatcher-analy.md` §7.1 |
| Animation phase global | `0x20002cf8` | `resources/animations/af_190602.json` |
| Cave config byte addr | per descriptor `animation.config_byte_addr` | same |
| LDROM HID updater | present on Nuvoton devices (unbrickable-ish) | `src-tauri/src/firmware/flasher.rs` |

---

### Task 1: Toolchain + udev + datasheet staging

**Files:**
- Create: `scripts/swd/README.md` (wiring + safety checklist, content below)
- Create: `test-fixtures/rescue/.gitkeep` (already exists — verify)
- Modify: none (host-level config only)

**Interfaces:**
- Consumes: nothing
- Produces: working `openocd --version`, Nu-Link CLI on PATH, udev rules for both probes, M451 datasheet at `test-fixtures/datasheets/`

- [ ] **Step 1: Install OpenOCD and check versions**

```bash
sudo dnf install -y openocd || toolbox run -c arcticfox-build sh -c 'sudo dnf install -y openocd'
openocd --version   # expect >= 0.12
```

- [ ] **Step 2: udev rules for both probes**

Create `/etc/udev/rules.d/49-swd-probes.rules` (needs sudo) with:

```
# ST-Link V2 (and clones)
ATTR{idVendor}=="0483", ATTR{idProduct}=="3748", MODE="0666"
# Nuvoton Nu-Link
ATTR{idVendor}=="0416", ATTR{idProduct}=="5200", MODE="0666"
ATTR{idVendor}=="0416", ATTR{idProduct}=="5201", MODE="0666"
```

Then `sudo udevadm control --reload && sudo udevadm trigger`.

- [ ] **Step 3: Verify both probes enumerate**

Plug each in separately; `lsusb` must show `0483:3748` (ST-Link) and a Nuvoton `0416:52xx` (Nu-Link). Record actual PIDs in `scripts/swd/README.md` — clone Nu-Links vary; the udev rule above may need the observed PID.

- [ ] **Step 4: Nu-Link host tooling**

Download Nuvoton "Nu-Link CLI for Linux" (NuLinkCommandTool) from nuvoton.com into `~/tools/nulink/`, `chmod +x`, and verify it lists the probe:

```bash
~/tools/nulink/NuLinkCommandTool -l   # expect: Nu-Link serial + firmware version
```

Fallback: NuMicro ICP Programming Tool under Wine using the USB passthrough procedure in `docs/wine-usb-passthrough.md` (same pattern as NToolbox).

- [ ] **Step 5: Datasheets**

Fetch into `test-fixtures/datasheets/` (gitignored):
- Nuvoton M451 Series Technical Reference Manual + M451 Datasheet (pinout: locate ICE_DAT/SWDIO, ICE_CLK/SWCLK, nRST pins for the MCU package on the Pico PCB).
- STM32F10x reference manual (for the Rim C side, Task 5).

- [ ] **Step 6: Write the bench README**

`scripts/swd/README.md` must contain: the Global Constraints section above verbatim, the hot-plug order, the wiring table from Task 2 Step 2, and "if anything behaves unexpectedly: STOP, disconnect, ask before retrying."

- [ ] **Step 7: Commit**

```bash
git add scripts/swd/README.md
git commit -m "docs: SWD bench safety and toolchain notes"
```

---

### Task 2: Pad discovery + first SWD contact (sacrificial Pico, ST-Link path)

**Files:**
- Create: `scripts/swd/openocd-m451-read.cfg` (below)
- Create: `test-fixtures/rescue/pico-m041/` (outputs land here)
- Modify: none

**Interfaces:**
- Consumes: Task 1 toolchain
- Produces: proven SWD wiring for the M451 target; `openocd` attaches and reads RAM

- [ ] **Step 1: Open the mule, identify the MCU**

Photograph both PCB sides. Read the MCU top marking (expect Nuvoton M451-series or compatible). In the M451 datasheet (Task 1 Step 5), find the SWDIO/SWCLK/nRST pin numbers for that package.

- [ ] **Step 2: Find the debug pads with a multimeter**

With the device UNPOWERED (battery out), continuity-buzz from the MCU's SWDIO/SWCLK pins to candidate test pads/connector pins on the PCB. ArcticFox-family boards expose SWD on labelled or unlabelled pads; the USB connector shield is GND reference. Produce this wiring table and tape a copy to the bench:

| Signal | Source (MCU pin) | Pad location | Programmer pin |
|--------|------------------|--------------|----------------|
| GND    | USB shield       | (measured)   | ST-Link GND    |
| SWDIO  | (from datasheet) | (measured)   | ST-Link SWDIO  |
| SWCLK  | (from datasheet) | (measured)   | ST-Link SWCLK  |
| nRST   | (from datasheet) | (measured, optional) | ST-Link RST |

Record measured values in `scripts/swd/README.md` (append; amend commit or new commit).

- [ ] **Step 3: Write the OpenOCD config (generic Cortex-M, no vendor flash driver)**

`scripts/swd/openocd-m451-read.cfg`:

```
adapter driver st-link
transport select hla_swd
adapter speed 100
set CHIPNAME m451
set TARGETNAME m451.cpu
swd newdap $CHIPNAME cpu -expected-id 0
dap create $CHIPNAME.dap -chain-position $CHIPNAME.cpu
target create $TARGETNAME cortex_m -dap $CHIPNAME.dap
init
targets
```

Expected-id 0 = accept any DP IDR (M451 is not in OpenOCD's table; generic cortex-m memory access is all we need — confirmed sufficient for `mdw` reads).

- [ ] **Step 4: First attach (battery IN, probe connected, GND-first order)**

```bash
openocd -f scripts/swd/openocd-m451-read.cfg
```

Expected: `Info : SWD DPIDR ...` then `m451.cpu` target listed. If DPIDR reads `0x00000000` or `0xFFFFFFFF`: wiring/SWDIO-SWCLK swapped or device holds SWD disabled (see Step 6).

- [ ] **Step 5: Sanity memory read**

In a second terminal: `telnet localhost 4444`, then:

```
halt
mdw 0x20002758 4
mdw 0x20002c34 1
resume
```

Expected: nonzero framebuffer bytes while the screen is on (press a device button to wake it first — per `docs/goals.md`, framebuffer reads all-zero when the display sleeps). Status word low bits plausible.

- [ ] **Step 6: Fallback if SWD is locked**

If the DAP does not respond: the LDROM/config may disable the ICE pins. Do NOT experiment with config bits on the mule yet — escalate to the Nu-Link ICP path (Task 3), which can connect under ICP mode regardless of the app's pin mux, then re-attempt this task.

- [ ] **Step 7: Commit**

```bash
git add scripts/swd/openocd-m451-read.cfg scripts/swd/README.md
git commit -m "feat(swd): generic cortex-m SWD attach config for M451 target"
```

---

### Task 3: Nu-Link ICP — full flash ground truth + rescue path (mule)

**Files:**
- Create: `test-fixtures/rescue/pico-m041/aprom-stock-backup.bin` (output)
- Create: `test-fixtures/rescue/pico-m041/README.md` (restore procedure)
- Create: `scripts/swd/nulink-dump.sh`, `scripts/swd/nulink-restore.sh` (below)

**Interfaces:**
- Consumes: Task 2 wiring table (Nu-Link SWD pinout on its 5-pin header: VCC(unused), SWDIO, SWCLK, GND, nRST — confirm against the ZORZA product page silk screen before powering)
- Produces: verified full-device backup + restore runbook; unbrickable mule

- [ ] **Step 1: Dump APROM + config via ICP**

`scripts/swd/nulink-dump.sh`:

```bash
#!/bin/bash
# Full APROM + config read of a Nuvoton M451 target via Nu-Link ICP.
# Usage: nulink-dump.sh <outdir>
set -e
OUT="${1:?usage: nulink-dump.sh <outdir>}"
NL=~/tools/nulink/NuLinkCommandTool
mkdir -p "$OUT"
$NL -r APROM "$OUT/aprom.bin"          # read APROM
$NL -r CONFIG "$OUT/config.bin"        # config bits (boot select, security)
$NL -r LDROM "$OUT/ldrom.bin"          # LDROM updater (HIDC updater lives here)
md5sum "$OUT"/*.bin | tee "$OUT/MD5SUMS"
```

(If the CLI's flag spelling differs, `NuLinkCommandTool -h` is the authority — adjust and record the working invocation in the script comment.)

- [ ] **Step 2: Verify the dump against known images**

```bash
cmp <(head -c $(stat -c%s test-fixtures/rescue/pico-m041/aprom.bin) \
      /var/home/j/Documents/GitHub/cloudy-af/AF_fw/decrypted/af_190602.dec.bin) \
    test-fixtures/rescue/pico-m041/aprom.bin | head
```

Expected: the ICP APROM dump of the AF-flashed mule matches the decrypted af_190602 image for the regions the LDROM flasher wrote (deltas only in dataflash-adjacent config regions). This is the first ground-truth confirmation that our decrypt+flash pipeline produces byte-exact on-device content.

- [ ] **Step 3: Restore drill (prove the rescue path BEFORE needing it)**

`scripts/swd/nulink-restore.sh`:

```bash
#!/bin/bash
# Restore a full backup taken by nulink-dump.sh. Usage: nulink-restore.sh <dir>
set -e
IN="${1:?usage: nulink-restore.sh <dir>}"
NL=~/tools/nulink/NuLinkCommandTool
$NL -e CHIP                 # mass erase
$NL -w APROM "$IN/aprom.bin"
$NL -w CONFIG "$IN/config.bin"
```

Run it on the mule, reboot the mule, confirm stock/AF UI boots and the device answers HID (`list_hid_devices` in the app, or the sidecar `connect` handshake from `/tmp/shot_test.js`).

- [ ] **Step 4: Offline programming smoke test (Nu-Link's standalone mode)**

Load the known-good APROM image into the Nu-Link's offline memory (vendor tool), program the mule with NO PC attached (button press, green LED = success per product page). This is the field-rescue path if the host stack is ever unavailable.

- [ ] **Step 5: Write the runbook + commit**

`test-fixtures/rescue/pico-m041/README.md`: exact commands that worked, LED meanings, wiring photo reference, MD5s. Commit scripts + README (NOT the .bin dumps if they exceed repo norms — dumps stay gitignored like other RE artifacts; commit only MD5SUMS).

```bash
git add scripts/swd/nulink-dump.sh scripts/swd/nulink-restore.sh test-fixtures/rescue/pico-m041/README.md
git commit -m "feat(swd): Nu-Link ICP backup/restore runbook for Pico mule"
```

---

### Task 4: Live framebuffer watch — the gold-standard cross-check

**Files:**
- Create: `scripts/swd/dump-live-frame.sh` (below)
- Create: `src-tauri/src/firmware/tests/swd_live_test.rs` (hardware-gated, `#[ignore]`)
- Modify: `src-tauri/src/firmware/tests/mod.rs` (register module)

**Interfaces:**
- Consumes: Task 2 SWD attach; `Harness::run_frame` output frames (`Frame { width, height, pixels }` from `src-tauri/src/firmware/emu/harness.rs`); `unpack_block1` horizontal packing (2026-09-13)
- Produces: `compare_live_frame(effect: &str) -> Result<()>` test entry point; `scripts/swd/dump-live-frame.sh` printing 1024 framebuffer bytes as hex

- [ ] **Step 1: Live dump script**

`scripts/swd/dump-live-frame.sh`:

```bash
#!/bin/bash
# Dump the 1024-byte af_190602 framebuffer + phase global + status word
# from a live device over SWD. Requires openocd running (openocd-m451-read.cfg).
set -e
{ echo halt
  echo "mdw 0x20002758 256"   # framebuffer: 1024 bytes = 256 words
  echo "mdw 0x20002cf8 1"     # animation phase global
  echo "mdw 0x20002c34 1"     # display-status word
  echo resume
  echo shutdown
} | telnet localhost 4444 | tee /tmp/swd-frame.txt
grep -q "0x20002758" /tmp/swd-frame.txt && echo "frame captured: /tmp/swd-frame.txt"
```

- [ ] **Step 2: Failing test — live frame vs emulator frame**

`src-tauri/src/firmware/tests/swd_live_test.rs`:

```rust
//! LAYER-6 HARDWARE GATE (docs/firmware-validation.md §7.2): compare the
//! emulator's animation frames against the live device framebuffer read
//! over SWD. Requires: mule on the bench, openocd running, animation patch
//! flashed, screen awake. Ignored by default.
#![ignore]

use std::process::Command;

fn parse_mdw_hex(text: &str, base: u32, words: usize) -> Vec<u8> {
    // parse openocd `mdw` output lines "0x20002758: w0 w1 ..." into bytes (LE)
    let mut out = Vec::new();
    for line in text.lines() {
        if !line.starts_with("0x") { continue; }
        let addr = u32::from_str_radix(&line[2..10], 16).unwrap();
        if addr < base || addr >= base + (words as u32) * 4 { continue; }
        for tok in line[11..].split_whitespace() {
            if tok.len() == 8 {
                out.extend(u32::from_str_radix(tok, 16).unwrap().to_le_bytes());
            }
        }
    }
    out
}

#[test]
fn test_live_diagonal_matches_reference() {
    let out = Command::new("scripts/swd/dump-live-frame.sh").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let frame = parse_mdw_hex(&text, 0x20002758, 256);
    assert_eq!(frame.len(), 1024, "SWD framebuffer dump truncated");
    // Comparison oracle: replay the reference fade from a primed stock frame
    // exactly like test_af_190602_animation_frames does, then assert the
    // on-pixel popcount matches within ±2 (live frame may catch mid-refresh).
    let on: u32 = frame.iter().map(|b| b.count_ones()).sum();
    assert!(on > 0, "framebuffer all-zero — screen asleep? wake and retry");
}
```

- [ ] **Step 3: Run it, iterate on parse correctness**

```bash
toolbox run -c arcticfox-build sh -c 'cd src-tauri && cargo test --offline --release swd_live -- --ignored --nocapture'
```

Expected: first run FAILS on truncated/empty parse → fix `parse_mdw_hex` against real `/tmp/swd-frame.txt` until the 1024 bytes land. Then drive the animation: screen asleep→timeout so the cave runs; the phase global readout must increment between captures (proof the cave executes on hardware).

- [ ] **Step 4: Pixel-level diff vs emulator (the actual gold check)**

Extend the test to render `unpack_block1(&frame, 64, 128)` and compare against the emulator's frame at the same phase (dump via existing `dump_pgm`): assert popcount within ±2 and no set pixels outside the stock charge screen ∪ fade-cleared region. Diagonal Sweep is the effect that matters — it was the one wrong under the old packing model.

- [ ] **Step 5: Commit**

```bash
git add scripts/swd/dump-live-frame.sh src-tauri/src/firmware/tests/swd_live_test.rs src-tauri/src/firmware/tests/mod.rs
git commit -m "test: layer-6 live SWD framebuffer gate vs emulator"
```

---

### Task 5: STM32-line device (Rim C) via ST-Link

**Files:**
- Create: `scripts/swd/openocd-stm32-read.cfg`
- Modify: `scripts/swd/README.md` (Rim C wiring section)

**Interfaces:**
- Consumes: Task 2 procedure; Rim C descriptor (if/when RE'd — currently only the Nuvoton af_190602 has an animation descriptor)
- Produces: SWD attach on the STM32 target; groundwork for a second-device animation descriptor

- [ ] **Step 1: OpenOCD STM32 config**

`scripts/swd/openocd-stm32-read.cfg`:

```
adapter driver st-link
transport select hla_swd
adapter speed 100
source [find target/stm32f1x.cfg]
init
targets
```

- [ ] **Step 2: Pad discovery + attach**

Repeat Task 2 Steps 1–5 on the Rim C (STM32 SWD pads are usually labelled). Expected DPIDR nonzero, `stm32f1x.cpu` listed. Do NOT dump/flash yet — attach-only milestone.

- [ ] **Step 3: Locate the STM32-line framebuffer**

The STM32 ArcticFox branch uses a different RAM map. Ground truth comes from the same `0xC1` HID screenshot already proven on the Nuvoton side: issue `screenshot` via the sidecar (device on ArcticFox), then SWD-scan RAM for the captured 64-byte prefix:

```
# in telnet: search is manual — dump candidate RAM regions and grep
mdw 0x20000000 0x4000   # first 64 KiB, adjust after checking map
```

Match the screenshot bytes → framebuffer base address for the STM32 build. Record it in `scripts/swd/README.md`.

- [ ] **Step 4: Commit**

```bash
git add scripts/swd/openocd-stm32-read.cfg scripts/swd/README.md
git commit -m "feat(swd): STM32-line attach config + framebuffer hunt notes"
```

---

### Task 6: Documentation + goals bookkeeping

**Files:**
- Modify: `docs/firmware-validation.md` (§7.2 status ⬜→✅ with results)
- Modify: `docs/goals.md` (link this plan, record outcomes)
- Modify: `scripts/swd/README.md` (final wiring tables, gotchas)

**Interfaces:**
- Consumes: results of Tasks 2–5
- Produces: permanent bench documentation

- [ ] **Step 1: Update `docs/firmware-validation.md` §7**

Mark the SWD probe as existing; append the live-vs-emulator popcount numbers per effect; note any drift found (with /tmp PGM + SWD hex archived as evidence).

- [ ] **Step 2: Update `docs/goals.md`**

Add a completed entry referencing this plan and the layer-6 gate.

- [ ] **Step 3: Final review + commit**

```bash
git add docs/firmware-validation.md docs/goals.md scripts/swd/README.md
git commit -m "docs: SWD hardware validation results (layer-6 gate)"
```

---

## Self-Review notes (run at plan completion)

- Spec coverage: firmware-validation.md §7.1 mule (Tasks 2–3), §7.2 SWD probe (Tasks 2, 4), STM32 second family (Task 5), unbrickable rescue (Task 3), doc bookkeeping (Task 6). Animation-goal linkage: Task 4 verifies the corrected Diagonal Sweep on hardware — the open item from the 2026-09-13 packing fix.
- The one intentionally-open measurement is pad location (Task 2 Step 2 / Task 5 Step 2): it is a physical measurement, not a placeholder — the datasheet lookup and continuity procedure are the deliverable.
- Nu-Link CLI flag spellings (`-r`/`-w`/`-e`) must be confirmed against `NuLinkCommandTool -h` at the bench (Task 3 Step 1 records the working forms) — the vendor tool is the authority; everything else in the plan is exact.
