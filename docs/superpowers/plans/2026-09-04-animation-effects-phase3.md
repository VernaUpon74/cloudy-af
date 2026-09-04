# Animation Effects Phase 3 — First Effect Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reverse-engineer the charge-screen render function in `af_190602`, build a unit-tested Thumb bytecode builder, and ship a Gradient Fade animation patch that passes the emulator pre-flash gate and on-device screenshot verification.

**Architecture:** The render function of the decrypted ArcticFox build `af_190602` is located with the existing headless Ghidra tooling; findings land in a per-build descriptor `resources/animations/af_190602.json`. The Thumb emulator (`src-tauri/src/firmware/emu/`, currently 16-bit Thumb-1 only) is extended with the Thumb-2 subset the real function actually uses, driven by gate failures. A small bytecode builder emits the Gradient Fade effect into a firmware code cave; a 2-halfword branch at the render hook site detours through it. The emulator's layer-4 gate (real render function, 4 frames, no faults) and layer-5 golden-frame differential test must pass before any hardware flash; verification closes with HID screenshots on the Pico (M041).

**Tech Stack:** Rust (emu, patch engine, Tauri commands), Ghidra 12.1.2 headless + Temurin JDK 21 (dev-time only, in gitignored `DecryptProject/`), existing `.patch` byte-line format.

**Spec:** `docs/superpowers/specs/2026-08-19-firmware-animation-pipeline-design.md` (Phase 3, section "Implementation Phasing"; Animation Injection and Testing sections bind). Related: `docs/superpowers/plans/2026-08-28-firmware-emulation-harness.md` (layers 4–5), `docs/goals.md` (screenshot quirks item).

## Global Constraints

- Ghidra + JDK are dev-time-only tooling living in gitignored `DecryptProject/`; the app never calls them at runtime and nothing RE-related is packaged (spec, "Dependencies").
- Every flash goes through `flash_firmware_guarded`; a failed checksum aborts before any erase (spec, "Safety & Error Handling").
- Hardware-in-the-loop tests are `#[ignore]`d by default and run explicitly, mirroring existing `flasher_test.rs` practice.
- Effects use all-integer math on a shared 16-entry sine/cosine LUT; ~200–400 bytes of Thumb each, emitted by the bytecode builder — not a general-purpose assembler (spec, "Animation Injection").
- Phase advances per rendered frame using the render loop's natural cadence (~8 fps target); no timer hardware in v1 (spec).
- Animation config byte lives at a fixed dataflash offset; values `0=Off, 1=Swirl, 2=Gradient Fade, 3=Rippling Wave` (spec).
- Per AGENTS.md: when README/CHANGELOG/docs are updated for a change, bump the app version in the same commit (`package.json`, `sidecar/package.json` + `sidecar/package-lock.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` + `Cargo.lock`, `flatpak/org.cloudy.af.appdata.xml` `<release>` entry, `CHANGELOG.md`).
- Cargo tests run offline: `cargo test --offline` from `src-tauri/`.
- af_190602 is VandalProof-encrypted; decrypted working copy is `AF_fw/decrypted/af_190602.dec.bin` (gitignored RE artifact, 116,716 bytes, already imported in the `AFSamples` Ghidra project).
- The MCU is Cortex-M4F-class (Nuvoton M451/M471): Thumb-2 32-bit encodings WILL appear in real render code (`resources/re/ldrom-commands.json`, mcu notes).

---

### Task 1: Ghidra environment sanity + render-function RE on af_190602

RE is investigative, so steps are concrete commands with decision points instead of TDD cycles. Deliverable: the descriptor file + RE notes; the gate may still fault on Thumb-2 — that is Task 2's input.

**Files:**
- Modify: `DecryptProject/` (gitignored scratch — JDK symlink, Ghidra project state)
- Create: `resources/animations/af_190602.json`
- Create: `resources/re/af_190602-render.md` (RE notes)
- Test: gate `src-tauri/src/firmware/tests/emu_test.rs:173` (`test_af_190602_render_gate`, `#[ignore]`d)

**Interfaces:**
- Consumes: `AF_fw/decrypted/af_190602.dec.bin`; Ghidra scripts in `DecryptProject/ghidra-scripts/`; `emu::harness::Descriptor` JSON schema (`build`, `render_entry`, `display_buffer{start,end,width,height}`, `ram_globals[]`, `ram_size`, `args[4]`; u32 fields accept `"0x…"` strings; unknown extra fields are ignored by serde).
- Produces: `resources/animations/af_190602.json` — a valid `Descriptor` plus an extra `"animation"` object consumed by Tasks 5–7:
  ```json
  {
    "build": "af_190602",
    "render_entry": "0x<addr>",
    "display_buffer": { "start": "0x<addr>", "end": "0x<addr>", "width": 64, "height": 128 },
    "ram_globals": [ { "start": "0x<addr>", "end": "0x<addr>" } ],
    "ram_size": 32768,
    "args": [0, 0, 0, 0],
    "animation": {
      "hook_site": "0x<addr>",
      "hook_resume": "0x<addr>",
      "code_cave": { "start": "0x<addr>", "size": 512 },
      "config_byte_addr": "0x<addr>",
      "phase_global": "0x<addr>"
    }
  }
  ```

- [ ] **Step 1: Fix the JDK path mismatch**

`scripts/fetch-ghidra.sh` prints usage with `tools/jdk-21`, but the on-disk dir is `tools/jdk-21.0.12.1+1`. Symlink rather than re-download:

```bash
cd /var/home/j/cloudy-af/DecryptProject/tools
ln -sfn jdk-21.0.12.1+1 jdk-21
ls jdk-21/bin/java && jdk-21/bin/java -version
```

Expected: `openjdk version "21…"` prints.

- [ ] **Step 2: (Re)analyze af_190602 with coverage count**

```bash
cd /var/home/j/cloudy-af/DecryptProject
JAVA_HOME="$PWD/tools/jdk-21" ghidra_12.1.2_PUBLIC/support/analyzeHeadless \
  ghidra-projects AFSamples \
  -process af_190602.dec.bin -noanalysis \
  -scriptPath ghidra-scripts -postScript HeadlessAFCount.java 2>&1 | grep RESULT
```

(If `-process` reports the program missing, re-import: replace `-process af_190602.dec.bin -noanalysis` with `-import ../AF_fw/decrypted/af_190602.dec.bin -processor ARM:LE:32:Cortex -loader-baseAddr 0x0 -preScript HeadlessAFPre.java -postScript HeadlessAFCount.java`, then run analysis via a second `-process` pass WITHOUT `-noanalysis`.) Expected: `RESULT af_190602.dec.bin functions=<hundreds> …`. A near-zero function count means analysis never ran — redo with a full analysis pass before continuing.

- [ ] **Step 3: Locate the HID dispatcher and the 0xC1 screenshot handler**

```bash
JAVA_HOME="$PWD/tools/jdk-21" ghidra_12.1.2_PUBLIC/support/analyzeHeadless \
  ghidra-projects AFSamples -process af_190602.dec.bin -noanalysis \
  -scriptPath ghidra-scripts -postScript FindHidCommands.java 2>&1 | tail -60
JAVA_HOME="$PWD/tools/jdk-21" ghidra_12.1.2_PUBLIC/support/analyzeHeadless \
  ghidra-projects AFSamples -process af_190602.dec.bin -noanalysis \
  -scriptPath ghidra-scripts -postScript FindCommandImmediates.java 2>&1 | grep -i c1
```

The `0xC1` (screenshot) handler reads the framebuffer and streams it over HID — its code reveals the **display buffer address** and dimensions. Record every candidate address.

- [ ] **Step 4: Decompile the 0xC1 handler and its neighbors**

```bash
JAVA_HOME="$PWD/tools/jdk-21" ghidra_12.1.2_PUBLIC/support/analyzeHeadless \
  ghidra-projects AFSamples -process af_190602.dec.bin -noanalysis \
  -scriptPath ghidra-scripts -postScript DecompileFunctions.java 0x<handler> [0x<callee> …] 2>&1 | tail -200
```

Decision points:
- The display buffer is the RAM region the handler reads 1024 bytes from (64×128/8). Confirm width/height = 64/128 from loop bounds.
- Walk backwards from buffer writes to find the **render entry**: the function that composes a full frame into the buffer. Cross-check with `FindMagicConstants.java <buffer-addr>` and decompile each xref site; the charge-screen render is on the HID-command-less path (called from the main loop), and references font/string-table pointers (per `resources/definitions/ArcticFox.xml` image/string table pointers).
- **Hook site:** inside that render function, after the frame is composed but before the buffer is pushed to the OLED (look for the SPI/display-flush call as the last callee). Needs 2 consecutive halfwords (4 bytes) safe to overwrite — i.e. a spot where a 4-byte detour can be re-executed in the cave. `hook_resume` = address of the first instruction after the patch.
- **Code cave:** find a ≥512-byte `0xFF`-filled (or trailing-padding) region in APROM, 4-byte aligned. Verify with `DumpDisassembly.java` that nothing references it.
- **Config byte:** a dataflash offset not touched by the settings struct (dataflash base `0x1F000`, cache 0x800 bytes per `resources/re/ldrom-commands.json`); confirm the APROM reads dataflash via direct memory-mapped loads (LDROM RE: `FUN_00001d2c` uses `mov.w r0, #0x1f000`) and pick a spare byte; record its absolute address as `config_byte_addr`.
- **Phase global:** a free 4-byte word in RAM (inside a `ram_globals` range the descriptor allows) used as the animation phase counter.
- **ram_globals:** every RAM region the render function (and its callees) reads/writes outside the display buffer and stack.

- [ ] **Step 5: Write the descriptor + RE notes**

Write `resources/animations/af_190602.json` exactly in the schema above with the addresses found. Write `resources/re/af_190602-render.md` documenting: dispatcher address, 0xC1 handler address, display buffer address + how found, render entry + evidence, hook site + the 2 halfwords being replaced, cave bounds + verification it is unreferenced, config byte offset rationale, and the Ghidra commands used.

- [ ] **Step 6: Smoke-parse the descriptor**

```bash
cd /var/home/j/cloudy-af/src-tauri
cargo test --offline --lib firmware::emu::harness 2>&1 | tail -3
```
(sanity that the crate still builds), then:

```bash
cargo test --offline --lib firmware::tests::emu_test -- --ignored 2>&1 | tail -20
```

Expected: gate runs (descriptor + image both present now). PASS is ideal; FAIL with `EmuError::Undefined { pc, instr }` where `instr >> 11` is `0b11101`/`0b11110`/`0b11111` is the expected Thumb-2 gap and feeds Task 2. Any other failure (Unmapped/ACL/stack) means the descriptor addresses are wrong — fix in Task 1, not Task 2.

- [ ] **Step 7: Commit**

```bash
cd /var/home/j/cloudy-af
git add resources/animations/af_190602.json resources/re/af_190602-render.md
git commit -m "feat(anim): af_190602 charge-screen render descriptor + RE notes (phase 3 task 1)"
```

(No version bump: no README/CHANGELOG/user-facing docs change.)

---

### Task 2: Emulator Thumb-2 subset extension (gate-driven, TDD per encoding)

The emu (`emu/thumb.rs::decode(hw: u16)`) is Thumb-1 only; real M451 render code uses Thumb-2. Extend iteratively: run the gate, hit `EmuError::Undefined`, add exactly that encoding with unit tests, repeat until the gate runs clean. Implement ONLY what the gate hits. If an `Undefined` instruction turns out to be VFP/CP10-11, stop and reassess scope before writing a float unit — check first whether the faulting path is actually reachable on the charge screen (it usually is not; the render entry may just need an `args`/setup tweak).

**Files:**
- Create: `src-tauri/src/firmware/emu/thumb2.rs` (32-bit decode, `pub fn decode32(hw1: u16, hw2: u16) -> Option<Instr>`)
- Modify: `src-tauri/src/firmware/emu/thumb.rs` (new `Instr` variants; carve IT out of the `0xBFxx → Nop` arm at `thumb.rs:145`)
- Modify: `src-tauri/src/firmware/emu/cpu.rs:62-91` (route Thumb-2 prefixes in `step`; IT state field)
- Modify: `src-tauri/src/firmware/emu/mod.rs` (`mod thumb2;`)
- Test: unit tests in the modified files' `#[cfg(test)]` modules; gate = `test_af_190602_render_gate`

**Interfaces:**
- Consumes: existing `Instr` enum (`thumb.rs:6`), `Cpu::step` BL interception pattern (`cpu.rs:69-88`), `EmuError::Undefined { pc, instr }` (carries the faulting first halfword).
- Produces:
  - `thumb2::decode32(hw1: u16, hw2: u16) -> Option<Instr>` — pure decode, same style as `decode`.
  - `Instr` gains variants as needed, e.g. `Movw { rd: u8, imm: u16 }`, `Movt { rd: u8, imm: u16 }`, `LsImmW { load: bool, byte: bool, rt: u8, rn: u8, imm: u16 }`, `BW { off: i32 }`, `AddSubW { sub: bool, rd: u8, rn: u8, imm: u32 }`, `LslLsrW { … }`, `It { mask: u8, cond: u8 }`.
  - `Cpu` gains `it_state: u8` (0 = not in IT block; else ITSTATE); `execute` consults it for conditional execution of the next ≤4 instructions.

- [ ] **Step 1: Reproduce the first Undefined fault**

```bash
cd /var/home/j/cloudy-af/src-tauri
cargo test --offline --lib firmware::tests::emu_test -- --ignored 2>&1 | grep -A5 "frame 0"
```

Record `pc` and `instr`. Disassemble the faulting site to confirm the encoding:

```bash
cd /var/home/j/cloudy-af/DecryptProject
JAVA_HOME="$PWD/tools/jdk-21" ghidra_12.1.2_PUBLIC/support/analyzeHeadless \
  ghidra-projects AFSamples -process af_190602.dec.bin -noanalysis \
  -scriptPath ghidra-scripts -postScript PrintFunction.java 0x<pc> 2>&1 | tail -40
```

- [ ] **Step 2: Write the failing decode test (example: MOVW)**

In `thumb2.rs` test module — golden bytes from the ARM ARM (MOVW `11110 i 100100 imm4 | 0 imm3 Rd imm8`); `movw r0, #0x1234` = `0xF241 0x1034`:

```rust
#[test]
fn test_decode_movw() {
    assert_eq!(decode32(0xF241, 0x1034), Some(Instr::Movw { rd: 0, imm: 0x1234 }));
}
```

```bash
cd /var/home/j/cloudy-af/src-tauri && cargo test --offline --lib firmware::emu::thumb2 2>&1 | tail -3
```
Expected: FAIL (`decode32` does not exist).

- [ ] **Step 3: Implement decode + execute for that encoding**

```rust
// thumb2.rs
use super::thumb::Instr;

/// Thumb-2 32-bit decode. Called by Cpu::step when hw1 >> 11 is
/// 0b11101/0b11110/0b11111 and it is not the BL pair.
pub fn decode32(hw1: u16, hw2: u16) -> Option<Instr> {
    if hw1 >> 5 == 0b11110_010010_0 >> 1 || (hw1 & 0xFBF0) == 0xF240 {
        // MOVW/MOVT: 11110 i 10010 0/1 imm4 | 0 imm3 Rd imm8
        let i = ((hw1 >> 10) & 1) as u16;
        let imm4 = (hw1 & 0xF) as u16;
        let imm3 = ((hw2 >> 12) & 0x7) as u16;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let imm8 = (hw2 & 0xFF) as u16;
        let imm = (imm4 << 12) | (i << 11) | (imm3 << 8) | imm8;
        return Some(if hw1 & 0x0080 == 0 { Instr::Movw { rd, imm } } else { Instr::Movt { rd, imm } });
    }
    None // later encodings add arms above this
}
```

In `cpu.rs` `step`, extend the existing 32-bit interception: when `hw >> 11` is `0b11101`/`0b11110`/`0b11111` and it is not the BL pair, read `hw2`, call `decode32`, and `execute`. Wire `Movw`/`Movt` in `execute` (MOVW: `r[rd] = imm`, no flags; MOVT: `r[rd] = (r[rd] & 0xFFFF) | (imm << 16)`). Add an execute test:

```rust
#[test]
fn test_exec_movw_movt() {
    // movw r0, #0x1234 ; movt r0, #0x5678 -> r0 == 0x56781234
    let mut cpu = Cpu::new();
    let mut bus = Bus::new(vec![0x41, 0xF2, 0x34, 0x10, 0xC0, 0xF2, 0x78, 0x56], 0x1000);
    cpu.pc = 0;
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.r[0], 0x5678_1234);
}
```

- [ ] **Step 4: Run the new tests, then the gate**

```bash
cargo test --offline --lib firmware::emu 2>&1 | tail -3
cargo test --offline --lib firmware::tests::emu_test -- --ignored 2>&1 | grep -A5 "frame 0"
```

- [ ] **Step 5: Repeat Steps 1–4 per faulting encoding**

One encoding (or one tight family, e.g. `ldr.w/str.w` T3) per cycle. Special cases:
- **`B.W` (T4)**: same S/J1/J2 imm25 math as the existing BL decode at `cpu.rs:79-86`, minus the LR write. Reuse, don't duplicate — extract a shared `fn branch_off25(hw1: u16, hw2: u16) -> i32` helper with a regression test for the existing BL values from `emu_test.rs` (`bl 0x40` at `0x10`: `0xF000 0xF816` → off 44).
- **IT blocks**: carve `0xBF08..=0xBFFF` (and masked variants) out of the `0xBFxx → Nop` arm into `Instr::It { mask, cond }`; `Cpu.it_state` loads ITSTATE (`(cond << 4) | mask`), and `execute` skips (no-op) instructions whose condition fails, shifting ITSTATE per A rules (`it_state = (it_state << 1) & 0x1F`, block ends when `it_state & 0xF == 0b1000` pattern completes). TDD with a 2-instruction IT block: `itt eq; moveq r0,#1; movne r1,#2` with Z set/clear.
- **`0xBFxx` hints** (NOP/WFI/YIELD) must still decode as `Nop` — keep the existing arm for the hint subspace.

- [ ] **Step 6: Gate green across 4 frames**

```bash
cargo test --offline --lib firmware::tests::emu_test -- --ignored 2>&1 | tail -5
```
Expected: `test_af_190602_render_gate ... ok` (4 frames, no fault).

- [ ] **Step 7: Full suite + commit**

```bash
cargo test --offline 2>&1 | tail -5
cd /var/home/j/cloudy-af
git add src-tauri/src/firmware/emu/
git commit -m "feat(emu): Thumb-2 subset needed by af_190602 render gate (phase 3 task 2)"
```

---

### Task 3: Finalize descriptor + layer-4 gate green

The gate passing under Task 2 may still expose wrong descriptor addresses (Unmapped / ACL / stack-canary errors point here, not at decode). This task closes that loop.

**Files:**
- Modify: `resources/animations/af_190602.json`
- Modify: `resources/re/af_190602-render.md`

**Interfaces:**
- Consumes: gate test `test_af_190602_render_gate`; `EmuError` variants from `emu/mod.rs`.
- Produces: a descriptor under which `Harness::run_frame(5_000_000)` completes 4 frames with zero ACL violations, no canary corruption, no stack underflow.

- [ ] **Step 1: Run the gate; classify the failure**

```bash
cd /var/home/j/cloudy-af/src-tauri
cargo test --offline --lib firmware::tests::emu_test -- --ignored 2>&1 | tail -25
```

- `Unmapped`/`ACL violations` → the faulting address (in the error dump) is missing from `ram_globals` or is the display buffer range; confirm against Ghidra and widen the descriptor ranges to exactly the real globals (never blanket-allow all of RAM — the ACL is the anti-brick signal).
- `stack canary corrupted` / `stack pointer bottomed out` → raise `ram_size` to the MCU's real SRAM (M451: 32 KiB → 32768; M471: 64 KiB) and confirm `sp` in the dump vs. the real firmware's stack top (vector table entry 0 of the APROM image — read it: `python3 -c "import struct;d=open('../AF_fw/decrypted/af_190602.dec.bin','rb').read();print(hex(struct.unpack('<I',d[4:8])[0]), hex(struct.unpack('<I',d[0:4])[0]))"` — reset handler, initial SP).
- `BudgetExceeded` → the render loop waits on hardware (e.g. polls a DMA/SPI-done MMIO flag). Find the polled address in the trace tail of `debug_dump`, confirm in Ghidra, and add it to the descriptor as a bus stub (extend `Descriptor` with `"stubs": [{"addr": "0x…", "value": …}]` mapped to `bus.set_stub` in `Harness::new` — harness.rs change with a unit test: stubbed read returns the value).

- [ ] **Step 2: Apply descriptor/harness fix, rerun gate until 4 frames clean**

- [ ] **Step 3: Eyeball a frame**

In the gate test temporarily (or in a small `#[ignore]`d helper test) call `emu::harness::dump_pgm(&frame, Path::new("/tmp/af190602_frame.pgm"))` and view it (e.g. `display /tmp/af190602_frame.pgm` or open in an image viewer). The stock charge screen should be recognizable (battery/percent layout). Garbage that nevertheless passes means `render_entry` is wrong — revisit Task 1 Step 4.

- [ ] **Step 4: Commit**

```bash
cd /var/home/j/cloudy-af
git add resources/animations/af_190602.json resources/re/af_190602-render.md src-tauri/src/firmware/emu/harness.rs
git commit -m "fix(anim): af_190602 descriptor tuned to real globals/stack; gate green (phase 3 task 3)"
```

---

### Task 4: Thumb bytecode builder

Minimal assembler for the Thumb-1 subset the effects use (emitting only 16-bit instructions keeps the builder small; the one 4-byte sequence is the hook branch, which may need `B.W` — emit that as two explicit halfwords via the branch helper, not general Thumb-2 support).

**Files:**
- Create: `src-tauri/src/firmware/anim/asm.rs`
- Create: `src-tauri/src/firmware/anim/mod.rs` (`pub mod asm;` — and later tasks' modules)
- Modify: `src-tauri/src/firmware/mod.rs` (`pub mod anim;`)
- Test: `src-tauri/src/firmware/anim/asm.rs` test module

**Interfaces:**
- Consumes: nothing from other tasks (standalone).
- Produces:
  ```rust
  pub struct Asm { base: u32, code: Vec<u8>, labels: HashMap<String, u32>, fixups: Vec<Fixup>, pool: Vec<u32> }
  impl Asm {
      pub fn new(base: u32) -> Self;
      pub fn movs(&mut self, rd: u8, imm: u8) -> &mut Self;
      pub fn ldr_lit(&mut self, rt: u8, label: &str) -> &mut Self;   // pool reference
      pub fn ldr_imm(&mut self, rt: u8, rn: u8, imm5: u8) -> &mut Self;
      pub fn str_imm(&mut self, rt: u8, rn: u8, imm5: u8) -> &mut Self;
      pub fn strb_imm(&mut self, rt: u8, rn: u8, imm5: u8) -> &mut Self;
      pub fn ldrb_imm(&mut self, rt: u8, rn: u8, imm5: u8) -> &mut Self;
      pub fn adds(&mut self, rdn: u8, imm8: u8) -> &mut Self;
      pub fn subs(&mut self, rdn: u8, imm8: u8) -> &mut Self;
      pub fn cmp(&mut self, rn: u8, imm8: u8) -> &mut Self;
      pub fn ands(&mut self, rdn: u8, rm: u8) -> &mut Self;
      pub fn lsls(&mut self, rd: u8, rm: u8, imm5: u8) -> &mut Self;
      pub fn lsrs(&mut self, rd: u8, rm: u8, imm5: u8) -> &mut Self;
      pub fn muls(&mut self, rdm: u8, rn: u8) -> &mut Self;
      pub fn b(&mut self, label: &str) -> &mut Self;
      pub fn bcond(&mut self, cond: u8, label: &str) -> &mut Self;
      pub fn label(&mut self, name: &str) -> &mut Self;
      pub fn pool_word(&mut self, value: u32) -> &mut Self;          // literal pool entry, returns via ldr_lit label
      pub fn finish(mut self) -> Vec<u8>;                            // resolves labels, appends pool, 4-aligns
  }
  /// 4-byte Thumb-2 B.W pair from `from` to `to` (for the hook site).
  pub fn bw_pair(from: u32, to: u32) -> [u16; 2];
  ```

- [ ] **Step 1: Failing golden-byte tests**

```rust
#[test]
fn test_golden_encodings() {
    // Bytes cross-checked against emu_test.rs's hand-assembled synthetic image.
    let mut a = Asm::new(0x60);
    a.movs(0, 0xFF);            // 0x20FF
    assert_eq!(a.finish(), vec![0xFF, 0x20]);

    let mut a = Asm::new(0x60);
    a.movs(2, 0);               // 0x2200
    a.strb_reg_label_demo();    // see note — instead:
    // strb r3, [r1, r2] is register-offset; cover with explicit op:
    // (emu_test.rs used 0x548B at 0x08)
}

#[test]
fn test_bcond_backward() {
    // bne to an instruction 10 bytes back, mirroring emu_test.rs 0x0E: 0xD1FB
    let mut a = Asm::new(0x08);
    a.label("loop");
    a.adds(2, 1);               // 2 bytes -> pc 0x0A
    a.cmp(2, 16);               // 2 bytes -> pc 0x0C
    a.bcond(1, "loop");         // at 0x0E: 0xD1FB
    assert_eq!(a.finish(), vec![0x01, 0x32, 0x10, 0x2A, 0xFB, 0xD1]);
}

#[test]
fn test_bw_pair() {
    // b.w from 0x20 to 0x60: off = 0x3C, S=0, imm10=0, J1=J2=1, imm11=0x1E
    // hw1 = 11110 0 0000000000 = 0xF000, hw2 = 10 1 1 1 0000000011110 = 0xB81E
    let [hw1, hw2] = bw_pair(0x20, 0x60);
    assert_eq!((hw1, hw2), (0xF000, 0xB81E));
}
```

```bash
cd /var/home/j/cloudy-af/src-tauri && cargo test --offline --lib firmware::anim 2>&1 | tail -3
```
Expected: FAIL (module does not exist).

- [ ] **Step 2: Implement the builder**

Straight-line emission (`Vec<u8>`, LE halfwords), two-pass label resolution in `finish` (fixups store offset + kind: `B11`, `BCond8`, `LdrLit8`), literal pool appended after code with 4-byte alignment (pad with `0x46C0` nop), `ldr_lit` pc-relative math `addr = align(pc+4, 4) + imm8*4` matching the emu's `LdrLit` semantics (`thumb.rs:28`). Range-check every fixup: `B` ±2048, `BCond` ±254 (even), `LdrLit` imm8 ≤ 255 — panic with label name on overflow (build-time bug, not runtime).

- [ ] **Step 3: Tests pass, then round-trip through the emulator**

Add a test that assembles the cave body from `emu_test.rs::hook_patch` (`movs r0,#0xFF; ldr r1,[pc,#8]; strb r0,[r1,#15]; b back; nop; <pool>`) and asserts byte-equality with the hand-written bytes there (`0x20FF, 0x4902, 0x73C8, b_off(0x66,0x24), 0x46C0, pool`).

```bash
cargo test --offline --lib firmware::anim 2>&1 | tail -3
```

- [ ] **Step 4: Commit**

```bash
cd /var/home/j/cloudy-af
git add src-tauri/src/firmware/anim/ src-tauri/src/firmware/mod.rs
git commit -m "feat(anim): Thumb bytecode builder with golden-byte tests (phase 3 task 4)"
```

---

### Task 5: Gradient Fade effect — Rust reference + Thumb emission

Gradient Fade (spec: phase counter + 16-entry sine LUT + vertical threshold): a vertical intensity band sweeping over the charge screen, XORed/ANDed into the existing frame. Integer math only.

**Files:**
- Create: `src-tauri/src/firmware/anim/effects.rs`
- Modify: `src-tauri/src/firmware/anim/mod.rs` (`pub mod effects;`)
- Test: `src-tauri/src/firmware/anim/effects.rs` test module

**Interfaces:**
- Consumes: `asm::Asm`, `asm::bw_pair` (Task 4); descriptor `"animation"` block (Task 1).
- Produces:
  ```rust
  /// 16-entry quarter-wave sine LUT, values 0..=255.
  pub const SINE_LUT: [u8; 16] = [ /* 0,25,50,74,98,120,142,162,180,197,212,225,236,245,251,255 */ ];
  /// Pure reference: apply gradient fade for `phase` to a Block1-packed buffer in place.
  pub fn gradient_fade_apply(buf: &mut [u8], width: usize, height: usize, phase: u32);
  /// Emits the cave body: bump phase global, check config byte (!= 2 -> return),
  /// run the fade over the display buffer, branch back to hook_resume.
  pub fn emit_gradient_fade(a: &mut Asm, anim: &AnimationDesc) -> Result<(), AsmError>;
  /// Builds the full Patch: 4-byte B.W at hook_site + cave body at code_cave.
  pub fn build_gradient_fade_patch(desc_json: &str) -> Result<crate::firmware::patch::Patch, AnimError>;
  ```
  (`AnimationDesc` = serde struct over the descriptor's `"animation"` object; `AnimError` new error enum in `anim/mod.rs`.)

- [ ] **Step 1: Failing tests for the reference implementation**

```rust
#[test]
fn test_gradient_fade_band_moves_with_phase() {
    let mut a = vec![0xFFu8; 64 * 128 / 8]; // all pixels on
    let mut b = a.clone();
    gradient_fade_apply(&mut a, 64, 128, 0);
    gradient_fade_apply(&mut b, 64, 128, 32);
    assert_ne!(a, b, "different phases must dim different rows");
    // band is vertical: same row pattern for every column byte index stride
    assert_eq!(a[0], a[1], "columns equal at same y-group for phase 0");
}

#[test]
fn test_gradient_fade_period() {
    let mut a = vec![0xFFu8; 64 * 128 / 8];
    let mut b = a.clone();
    gradient_fade_apply(&mut a, 64, 128, 0);
    gradient_fade_apply(&mut b, 64, 128, 64); // full LUT period (4 * 16 entries)
    assert_eq!(a, b);
}
```

- [ ] **Step 2: Implement `gradient_fade_apply`**

Row `y` brightness = `SINE_LUT[((y + phase) / (height / 64)) & 15]`-style mapping; pixel cleared where brightness below threshold. Keep it ~15 lines; make the phase→row mapping identical to what the Thumb version can compute with shifts/adds (divide by power of two only).

- [ ] **Step 3: Failing test for emission + patch construction**

```rust
#[test]
fn test_build_patch_layout() {
    let desc = r#"{ "build": "t", "render_entry": "0x0",
        "display_buffer": {"start":"0x20001000","end":"0x20001400","width":64,"height":128},
        "animation": { "hook_site": "0x100", "hook_resume": "0x104",
            "code_cave": {"start":"0x8000","size":512},
            "config_byte_addr":"0x1F100", "phase_global":"0x20000000" } }"#;
    let p = build_gradient_fade_patch(desc).unwrap();
    // hook: 4 bytes at 0x100 = bw_pair(0x100, 0x8000)
    let [h1, h2] = crate::firmware::anim::asm::bw_pair(0x100, 0x8000);
    let at = |off: usize| p.modifications.iter().find(|m| m.offset == off).unwrap().patched;
    assert_eq!(at(0x100), h1 as u8 & 0xFF);
    assert_eq!(at(0x101), (h1 >> 8) as u8);
    assert_eq!(at(0x102), h2 as u8 & 0xFF);
    // cave body starts at 0x8000, fits in 512 bytes
    let max = p.modifications.iter().map(|m| m.offset).max().unwrap();
    assert!(max < 0x8000 + 512);
}
```

- [ ] **Step 4: Implement `emit_gradient_fade` + `build_gradient_fade_patch`**

Emission order: load config byte via `ldr_lit` → `cmp` 2 → `bcond NE` to exit; bump phase global (`ldr_lit`/`ldr_imm`/`adds`/`str_imm`); loop over the 1024 buffer bytes applying the fade using the LUT (pool: buffer addr, phase addr, config addr, LUT bytes emitted as pool words); `b` to hook_resume. Then `build_gradient_fade_patch` flattens to `PatchModification { offset, original: None, patched }` byte lines (same flattening as `emu_test.rs:95-109`). Parse the descriptor with the existing `load_descriptor` for the base fields plus a private struct for `"animation"`.

- [ ] **Step 5: Tests pass + commit**

```bash
cargo test --offline --lib firmware::anim 2>&1 | tail -3
cd /var/home/j/cloudy-af && git add src-tauri/src/firmware/anim/
git commit -m "feat(anim): Gradient Fade reference impl + Thumb emission + patch builder (phase 3 task 5)"
```

---

### Task 6: Layer 5 — emu golden frames + differential test

**Files:**
- Modify: `src-tauri/src/firmware/tests/emu_test.rs` (new `#[ignore]`d test)
- Test: `test_af_190602_gradient_fade_frames`

**Interfaces:**
- Consumes: `build_gradient_fade_patch` (Task 5), `apply_patch`, `Harness`, `frames_differ`, `dump_pgm`, `gradient_fade_apply` reference.
- Produces: pre-flash proof that the patched real firmware animates and matches reference math.

- [ ] **Step 1: Write the failing test**

```rust
/// LAYER-5: patched af_190602 renders an animated Gradient Fade in the emu.
/// Same skip-if-missing discipline as the layer-4 gate.
#[test]
#[ignore]
fn test_af_190602_gradient_fade_frames() {
    use std::path::Path;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let desc_path = root.join("resources/animations/af_190602.json");
    let img_path = root.join("AF_fw/decrypted/af_190602.dec.bin");
    if !desc_path.exists() || !img_path.exists() { eprintln!("skipping"); return; }
    let desc_json = std::fs::read_to_string(&desc_path).unwrap();
    let mut img = std::fs::read(&img_path).unwrap();
    let mut patch = crate::firmware::anim::effects::build_gradient_fade_patch(&desc_json).unwrap();
    let mut log = std::collections::HashMap::new();
    crate::firmware::patch::apply_patch(&mut img, &mut patch, &mut log).unwrap();
    let desc = load_descriptor(&desc_json).unwrap();
    let mut h = Harness::new(&img, desc);
    let mut frames = Vec::new();
    for i in 0..4 {
        frames.push(h.run_frame(5_000_000).unwrap_or_else(|e| panic!("frame {i}: {e}")));
    }
    for w in frames.windows(2) {
        assert!(Harness::frames_differ(&w[0], &w[1]), "animation must change frames");
    }
    dump_pgm(&frames[0], Path::new("/tmp/af_fade_f0.pgm")).unwrap();
    // …differential vs gradient_fade_apply reference: replicate phase from
    // the descriptor's phase_global reads (assert each frame's changing band
    // lands where the reference puts it for that phase).
}
```

Run: `cargo test --offline --lib firmware::tests::emu_test -- --ignored`; expected FAIL until the emission/emulator agree (budget, LUT pool alignment, and the phase-global RAM allowance in the descriptor are the usual suspects).

- [ ] **Step 2: Fix forward until green** — the reference (Task 5 Step 2) is ground truth; where they disagree, dump the cave bytes and step the emu (`cpu.trace`) to find the divergence. Do not weaken the assertions.

- [ ] **Step 3: Commit**

```bash
cd /var/home/j/cloudy-af
git add src-tauri/src/firmware/tests/emu_test.rs resources/animations/af_190602.json
git commit -m "test(anim): layer-5 gradient-fade golden frames through real render (phase 3 task 6)"
```

---

### Task 7: `.patch` serialization + pipeline wiring

The generated animation patch must be inspectable in the Firmware Editor's existing Patches tab before flashing (spec, UI section). That needs a serializer (patch.rs only parses today) plus a discovery entry.

**Files:**
- Modify: `src-tauri/src/firmware/patch.rs` (`pub fn patch_to_xml(patch: &Patch) -> String`)
- Create: `resources/patches/anim-gradient-fade-af_190602.patch` — generated artifact, committed for inspection
- Test: `patch.rs` test module round-trip

**Interfaces:**
- Consumes: `Patch`/`PatchModification` (`patch.rs`), `build_gradient_fade_patch` (Task 5).
- Produces: `patch_to_xml(&Patch) -> String` emitting `<Patch Name=… Version=… Author=…><Description>…</Description><Data> 0x…: * - 0x… </Data></Patch>`; `parse_patch(patch_to_xml(p), id)` round-trips modifications exactly.

- [ ] **Step 1: Failing round-trip test**

```rust
#[test]
fn test_patch_xml_roundtrip() {
    let mut p = Patch { id: "t".into(), name: "T".into(), version: "1.0".into(),
        author: "a".into(), description: "d".into(), applied: false,
        modifications: vec![PatchModification { offset: 0x10, original: None, patched: 0xFF }] };
    let xml = patch_to_xml(&p);
    let q = parse_patch(&xml, "t").unwrap();
    assert_eq!(q.modifications.len(), 1);
    assert_eq!(q.modifications[0].offset, 0x10);
    assert_eq!(q.modifications[0].patched, 0xFF);
    p.modifications.clear();
}
```

- [ ] **Step 2: Implement `patch_to_xml`; round-trip passes** (`*` for `original: None`; hex uppercase `0x%04X: %s - 0x%02X` to match `resources/patches/README.md`'s documented format).

- [ ] **Step 3: Generate the committed artifact**

Small `#[ignore]`d generator test (or `examples/` bin — follow existing repo layout) that writes `resources/patches/anim-gradient-fade-af_190602.patch` from `build_gradient_fade_patch` + the real descriptor; run it once, commit the output. Byte count sanity: hook 4 bytes + cave body ≤ 512.

- [ ] **Step 4: Commit**

```bash
cd /var/home/j/cloudy-af
git add src-tauri/src/firmware/patch.rs resources/patches/anim-gradient-fade-af_190602.patch
git commit -m "feat(patch): .patch serializer + generated gradient-fade artifact (phase 3 task 7)"
```

---

### Task 8: Hardware screenshot verification (Pico M041, `#[ignore]`d)

Spec's visual verification: frames differ over time, changing region matches the effect, undo restores static stock. Also closes the goals.md screenshot-quirks item (framebuffer reads all-zero when the display is asleep; PNG packing garbage at the top).

**Files:**
- Modify: `src-tauri/src/firmware/tests/flasher_test.rs` (new `#[ignore]`d `test_gradient_fade_hardware`)
- Modify: `src-tauri/src/firmware/flasher.rs` + its screenshot PNG packing (goals.md quirk fix, if root-caused)
- Modify: `docs/goals.md` (tick the screenshot-quirks item; note layer 4/5 done)

**Interfaces:**
- Consumes: `flasher::screenshot`, `flash_firmware_guarded`, backup/undo flow, `build_gradient_fade_patch`, encryption round-trip (`encrypt(_, EncryptionType::VandalProof)`).
- Produces: hardware-verified animation; resolved screenshot quirks.

- [ ] **Step 1: Fix the screenshot quirks first** (all-zero-when-asleep: wake via button press + capture immediately; PNG top garbage: re-check `test_screenshot_hardware` packing against `unpack_block1` semantics — byte `x+(y/8)*width`, bit `y%8`). Unit-test the packer against a synthetic framebuffer; hardware-test the wake path.

- [ ] **Step 2: Write the ignored hardware test**

Sequence: backup current image → apply gradient-fade patch → VP re-encrypt → `flash_firmware_guarded` (product-ID guard M041) → put device on charge screen → `screenshot()` ×3 at ~250 ms → assert successive frames differ AND the changing region is a moving vertical band (compare against `gradient_fade_apply` prediction per elapsed frames) → flash backup (undo) → screenshot asserts static & matches pre-patch capture. Every flash aborts on checksum failure (global constraint).

- [ ] **Step 3: Run on the Pico**

```bash
cd /var/home/j/cloudy-af/src-tauri
cargo test --offline --lib firmware::tests::flasher_test -- --ignored --nocapture
```

Requires the Pico connected, charged enough to show the charge screen. Rescue kit is staged at `test-fixtures/rescue/` if the flash goes wrong (battery-out + Plus procedure in its README).

- [ ] **Step 4: Update goals.md + CHANGELOG + version bump (1.17.0, user-facing feature) + commit**

Per AGENTS.md, all version files move together. CHANGELOG `## Unreleased` → `## 1.17.0 — 2026-09-04` (or current date).

```bash
cd /var/home/j/cloudy-af
git add -A
git commit -m "feat(anim): hardware-verified Gradient Fade on af_190602; screenshot quirks fixed (1.17.0)"
```

---

## Out of scope (deliberate)

- Swirl / Rippling Wave / clock hook — Phase 4.
- Animations tab UI, Appearance dropdown enablement (`index.html:423-435` stays `disabled`), config-byte UI — Phase 4. (The config byte is still honored by the patch; set it via `flasher::write_dataflash` in tests.)
- STM32 (af_211009) — Phase 5.
- Systick-based pacing — documented follow-up only (spec).

## Self-Review

**Spec coverage:** renderer RE charge screen → Task 1; Thumb bytecode builder → Task 4; Gradient Fade descriptor + injection → Tasks 1/5; screenshot verification → Task 8; emu layer 4 → Tasks 1–3 (gate green); layer 5 golden frames + differential → Task 6; "generated patch appears in Patches tab for inspection" → Task 7 committed `.patch` artifact; config byte at fixed dataflash offset → Tasks 1 (offset selection) / 5 (emission reads it); integer math + 16-entry LUT → Task 5; per-frame pacing → Task 5 (phase bumps once per render call); safety (guarded flash, checksum abort) → Task 8 Step 2 + Global Constraints. UI dropdowns/Animations tab deliberately deferred to Phase 4 per spec phasing.

**Placeholder scan:** RE outputs (addresses) are genuinely unknown until Task 1 runs — those fields carry `<addr>` markers inside the descriptor template with explicit discovery procedures (Task 1 Steps 3–5), which is the investigative core of this phase, not an omission. All code-producing tasks carry concrete code/tests.

**Type consistency:** `Descriptor`/`load_descriptor`/`Harness::run_frame`/`frames_differ`/`dump_pgm` match `emu/harness.rs`; `Patch{ id, name, version, author, description, modifications, applied }`/`PatchModification{ offset, original, patched }`/`apply_patch`/`parse_patch` match `patch.rs`; `EmuError::Undefined { pc, instr }` matches `emu/mod.rs`; `bw_pair`/`Asm`/`gradient_fade_apply`/`build_gradient_fade_patch`/`AnimationDesc` are defined in Tasks 4–5 and used identically in Tasks 6–8. IT-state semantics in Task 2 reference the `thumb.rs:145` Nop arm and `cpu.rs:69-88` BL precedent accurately.
