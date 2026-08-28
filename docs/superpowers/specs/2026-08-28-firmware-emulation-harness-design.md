# Firmware Emulation Harness Design

> Feeds Phase 3+ of `2026-08-19-firmware-animation-pipeline-design.md`.
> Purpose: test animated patches under emulation so no patched image is
> flashed to the Pico until it is proven non-bricking. Development-test
> tooling only — the app never emulates at runtime and nothing here ships.

## Overview

A dependency-free ARMv6-M (Cortex-M0) Thumb interpreter in Rust, plus a
function-level harness that loads the decrypted `af_190602` image, applies
the real generated animation patch, runs the charge-screen render function
for N frames, and asserts the execution is clean and the frames are correct.
Bricking classes caught before hardware: bad hook offset or branch encoding,
register/stack clobber at the hook, writes outside the display buffer / code
cave / stack, undefined or unaligned instructions, infinite loops, and
effect-math errors (wrong pixels).

## Decisions Log

- **Engine: own ARMv6-M interpreter, pure Rust, zero new crates.** Tests run
  `cargo test --offline` inside the Flatpak GNOME SDK, so Unicorn/QEMU would
  need vendoring a C library into an offline build. This mirrors the
  existing choice of a purpose-built Thumb bytecode emitter over a general
  assembler. Rejected: Unicorn (dependency/build cost), radare2 ESIL
  (partial ARM support, heavy dev-machine dep), QEMU (no Nuvoton M0 target).
- **Scope: function-level, not full-chip.** Execution starts at the render
  function entry from the descriptor — no reset vector, no boot flow, no
  peripheral modeling beyond scripted read stubs and log-and-drop writes.
  A boot-level smoke test is a documented later upgrade, not v1.
- **Integration: test-suite only.** The check runs in cargo tests during
  development. The app does not re-verify before flashing; the safety rule
  is procedural: no patched image goes to the Pico unless the layer-4 test
  (below) is green. Not wired into `flash_firmware_guarded`.
- **Interpreter is a correctness oracle, not a timing simulator.** No cycle
  counting, no pacing assertions — animation cadence stays a hardware
  question verified by the existing HID screenshot tests.

## Architecture

New module `src-tauri/src/firmware/emu/`, four units:

- **`cpu.rs`** — CPU state: r0–r15 (single privileged mode, no banked SP),
  N/Z/C/V flags, `step()` = fetch/decode/execute one instruction. Faults
  surface as typed `EmuError` (undefined instruction, unaligned access,
  budget exceeded). A fault is a test failure — that is the bricking signal.
- **`thumb.rs`** — v6-M Thumb decoder: the ~60-instruction 16-bit set
  (shifts, add/sub/cmp/mov, data-processing, multiply, literal/SP-relative
  loads, load/store word/half/byte, push/pop, B/BL/BX/BLX, SVC/BKPT,
  barrier/hint instructions as NOPs). No Thumb-2 beyond `BL`. Match-table
  decode; each opcode group has its own unit test.
- **`bus.rs`** — flat memory: flash region (patched image, read-only during
  execution), RAM region, ACL/watch layer. Every write checked against an
  allow-list {display buffer, code cave, stack, known firmware RAM globals};
  violations are recorded with address/value. Peripheral-address reads
  return 0 (or scripted stub values), writes are logged and dropped.
- **`harness.rs`** — scenario runner: decrypted image + descriptor
  (`resources/animations/af_190602.json`: render-function entry, hook
  offset, code-cave offset/size, display-buffer pointer/dimensions) + patch
  from the real patch engine. Applies the patch, seeds SP/registers, runs
  the render function under an instruction budget, returns per-frame
  display-buffer snapshots.

The patch under test goes through the real pipeline — bytecode builder →
patch engine → applied image — so hook encoding, cave fit, and effect code
are exercised exactly as they will be flashed.

## Execution Model & Data Flow

Per test run:

1. Load decrypted image into emulated flash; RAM zeroed except a seeded
   stack (top of RAM, canary values below SP to catch underflow).
2. Generate the animation patch via the real patch engine; apply to image.
3. Enter the render function at its descriptor entry with a sentinel return
   address (unmapped trap address): a clean return lands there and counts as
   success; a return anywhere else is a fault.
4. Run under an instruction budget (≈5M per frame; exceeding it = hang).
5. Snapshot the display buffer, decode Block1 vertical packing → 64×128
   frame.
6. Re-enter the function for the next frame (config byte / phase counter
   live in emulated memory, so frames advance naturally). N = 16 frames
   (≈2 s at the 8 fps target cadence).

**Subroutines & peripherals.** Helper calls that stay in flash just execute.
The display push writes SPI/GPIO registers — logged-and-dropped by the bus;
the frame is considered complete at the push (the hook fires before it). If
the render function reads a peripheral (status, buttons), the descriptor
carries a scripted per-address stub list — data, not code.

**Assertions per run:**

- Zero ACL violations (the anti-brick core).
- Clean return to the sentinel every frame; SP balanced.
- No undefined instructions, no unaligned access, budget never hit.
- Successive frames differ (animation actually runs); frame k matches a
  golden frame where the math is pinned, else a structural check (e.g.
  Gradient Fade's intensity band moves monotonically).
- Differential: the same effect math in plain Rust (shared 16-entry sine
  LUT) produces the same buffer as the emulated Thumb — catches interpreter
  bugs and builder bugs simultaneously.

PNG dumps of frames land in `target/emu-frames/` on failure or on demand.

## Error Handling

Every `EmuError` carries: faulting PC, raw instruction halfword, full
register dump, a ring buffer of the last ~32 executed PCs, and for ACL
violations the attempted address/value and the region it fell in.
Golden-frame mismatches dump expected/actual PNGs plus differing-pixel
count and bounding box. A failing test should identify whether the bug is
in the builder encodings, the descriptor offsets, or the interpreter,
without a debugger session.

## Testing

Layers, in build order:

1. **Decoder/opcode unit tests** — hand-assembled halfwords (cross-checked
   against the ARM ARM) → execute → assert registers/flags/memory. Edge
   cases: carry-out behavior, `PUSH {lr}`/`POP {pc}`, literal-pool
   alignment, negative offsets.
2. **Interpreter self-check** — synthetic functions assembled with the
   bytecode builder (array-sum loop, memcpy, sine-LUT lookup) run to known
   answers. Doubles as the builder's first integration test.
3. **Harness integration on a synthetic image** — fake render function
   (fills buffer, calls helper, writes fake SPI register, returns) +
   synthetic descriptor + real generated patch. Exercises patch
   application, hook branch, cave, watchpoints, frame extraction.
   No RE needed.
4. **Real descriptor test** — `#[ignore]`-gated until the Phase 3 Ghidra RE
   produces `resources/animations/af_190602.json`; then runs the actual
   render function from the actual firmware. **This test must be green
   before any animation image is flashed to the Pico.**
5. **Effect golden frames** — per effect (Gradient Fade first), pinned
   config, 16 frames, golden + structural assertions + differential vs the
   Rust reference math.

## Deliberate Exclusions

- No Thumb-2, no full-boot/reset-vector execution, no cycle accuracy.
- No peripheral modeling beyond scripted read stubs / log-and-drop writes.
- Not wired into the app or the flash path; dev-test tooling only.
- Does not replace on-device screenshot verification (complements it: emu
  golden frames can serve as the expected patterns for screenshot diffs).

## Sequencing

Layers 1–3 need no RE and can start immediately, in parallel with the
Phase 3 Ghidra renderer RE. Layer 4–5 unblock when
`resources/animations/af_190602.json` exists. The harness thereby splits
Phase 3's risk: interpreter correctness is proven on synthetic code before
real firmware ever touches it.

## Dependencies (already in tree)

- Thumb bytecode builder and patch engine (`firmware::patch`) — the code
  under test.
- Decrypted `af_190602` image (`AF_fw/decrypted/af_190602.dec.bin`,
  gitignored RE artifact; tests that need it skip when absent).
- Block1 vertical-packing decoder — exists for the screenshot feature
  (`flasher::screenshot` / PNG packing path); reuse, and fix the known
  top-of-frame garbage quirk noted in `docs/goals.md` when touched.
