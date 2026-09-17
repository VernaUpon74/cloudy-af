# Validation & animation-patch work — running handoff log

Running log for the non-hardware validation plan
(`docs/superpowers/plans/2026-09-13-nonhw-validation-routes.md`, routes from
`docs/firmware-validation.md`). Newest entries first within each section.

## Latest verification — SysTick model + EADC model (2026-09-17, later session)

Supersedes the LDMIA/IRQ42 entry below (that LDMIA fix is still uncommitted
in the working tree and still required).

- **SysTick register model landed** (`bus.rs`, private `SysTick` struct):
  CTRL (bits 0-2 latched; reads return CTRL | COUNTFLAG<<16 and CLEAR
  COUNTFLAG on read per ARMv7-M B3.3.3), RVR (24-bit), CVR (24-bit; write
  clears counter and COUNTFLAG). Ticking: `Bus::advance_core_tick` is called
  once per DECODED `Cpu::step` — failed fetch/decode costs no tick; IT-block
  members tick even when suppressed (matches the real core's fixed-cycle
  behavior). Counter semantics: 0 → RELOAD on tick; decrement otherwise;
  hitting 0 raises COUNTFLAG; RELOAD=0 never completes. One tick per step
  is a deliberate simplification (no cycle accuracy).
- **Result:** the boot gate's first blocker is gone — M041 boots past the
  0xE06 SysTick delay loop (200M budget) and now reaches the SECOND blocker:
  the `0x10B14..0x10B18` wait polling RAM 0x20000E6C (the EADC completion
  flag). Boot still fails at budget there. Full boot NOT verified.
- **EADC model landed** (new `emu/eadc.rs`, wired into `Bus`): M451 base
  0x40043000 — CTL +0x50 (ADCEN bit0 gates SWTRG), SWTRG +0x54 (19 module
  bits, pending), INTSRC0 +0xD0 (module→ADINT0 routing), STATUS2 +0xF8
  (ADIF0 bit0, write-1-to-clear), DAT0..18 +0x00..+0x48 (results).
  Deterministic completion after `EADC_COMPLETION_TICKS` (8) core ticks,
  lowest module first; ADIF0 set only when the completing module is routed
  via INTSRC0. Results come ONLY from test-configured samples
  (`Bus::set_eadc_sample`) — no invented sensor values. Latched writes are
  still recorded in `dropped_writes` (the record doubles as the peripheral-
  write log the IRQ42 handler-gate assertion relies on). NOT modeled: OV/
  VALID bits, compare flags, PDMA, scan sequencing, real analog inputs,
  conversion-latency accuracy (fixed 8 ticks; refine at the Nu-Link bench).
- **5 eadc.rs unit tests + 1 bus integration test** (SWTRG completion/
  DAT/ADIF0, ADCEN gating, unrouted module, W1C semantics, unmodeled-
  register reporting, SysTick coexistence). Full lib suite: **194 passed,
  47 ignored**. `cargo check --release --tests` clean; `git diff --check`
  clean. Clippy has a PRE-EXISTING deny-level error in bus.rs resolve()
  (`absurd_extreme_comparisons` on `addr >= FLASH_BASE` since FLASH_BASE=0)
  — NOT introduced by this session, NOT fixed here, runs `cargo check` not
  clippy for gates until that is addressed.
- **NVIC/exception delivery NOT implemented** — attempted, reverted. Honest
  post-mortem: the CPU has no exception entry/EXC_RETURN support; building
  it test-first round-trip-style is a real slice (vector fetch from
  4*(16+irq), 8-word frame stack, EXC_RETURN detection in BX/POP/ldr-pc
  epilogues, single-level delivery) that should be done in a FRESH session
  with the round-trip test written FIRST and no edits before it runs RED.
  The bus-side NVIC latch (ISER/ICER W1S/W1C) was built and removed with
  it; reuse the design sketch above when the slice is reattempted.
- Boot-path evidence note (from af190602_dis.txt 0x10AC8-0x10B28): the
  read-adc routine enables ADCIEN0 (CTL|=4), routes the module via
  INTSRC0|=1<<m, enables IRQ42 (ISER[1]|=0x400), SWTRG=1<<m, polls the
  flag at 0x20000E6C set by the IRQ42 handler 0x108B4 (writes flag=1,
  writes 1 to 0x400430F8 = EADC_STATUS2 ADIF0 W1C per SVD), then clears
  ADCIEN0 and reads DATm. So IRQ42 == ADINT0 on this silicon is plausible
  but UNVERIFIED (no SVD interrupt-number listing found in the tree);
  verify at the bench or from the device's startup file before trusting
  exception wiring.
- Next software work (in order): (1) exception entry/return slice in cpu.rs
  (round-trip test first, then delivery, then EXC_RETURN-aware BX/POP/
  ldr-pc epilogues), (2) rerun boot gate — expect either dispatcher reach
  or the next blocker, (3) wire EADC completion → NVIC request in the bus
  (one line: on ADIF0 set, request_irq(42) — the EADC model already knows
  when it fires), (4) static-audit/watchdog plan items remain open.
- Graph coverage: `check_index_coverage` reports bus.rs/cpu.rs/thumb.rs/
  emu_test.rs as `metadata_changed` (graph generation 2026-09-13 predates
  all recent commits) — all graph conclusions this session were qualified
  with direct source reads; re-index recommended before the next graph-
  driven session.

## Latest verification — LDMIA fix and IRQ42 dependency (2026-09-17)

This entry supersedes older stack-runaway/current-blocker claims below.

- The pending general-base LDMIA decoder/executor fix remains in the working
  tree. With it, the baseline boot test no longer returns into GPIO space.
- Removed the provisional constant SysTick COUNTFLAG stub. An always-complete
  delay is not a timer model and must not be mistaken for full-boot validation.
  The baseline M041 gate exhausts 200,000,000 instructions at PC `0xE06`,
  polling SysTick CTRL at `0xE000E010`. Disassembly shows CTRL is written with
  5 (ENABLE + CLKSOURCE, without TICKINT), then bit 16 is polled. This first
  blocker needs a COUNTFLAG/counter model, not SysTick interrupt delivery.
- The later wait observed with the provisional stub is `0x10B14..0x10B18`,
  polling RAM `0x20000E6C`. Direct binary inspection found vector-table entry
  `0xE8` = `0x108B5` (external IRQ42). Handler `0x108B4..0x108C0` writes 1
  to that RAM flag, writes 1 to peripheral address `0x400430F8`, then BX LR.
  This is a confirmed writer, not an exhaustive proof of all flag writers.
- Added an explicitly ignored, firmware-artifact-gated handler regression in
  `/var/home/j/cloudy-af/src-tauri/src/firmware/tests/emu_test.rs`. It starts
  at the real vector, checks both cleared and pre-set flag states, verifies
  the single RAM write and MMIO acknowledgment record, unchanged SP, return
  within 32 instructions, and clean RAM ACL. Missing firmware fails this
  explicitly invoked gate instead of silently passing.
- No interrupt injection/completion service was implemented. The isolated
  handler uses a return sentinel; it does not validate exception entry/return,
  NVIC enable/pending/masking, peripheral conversion results, or timing.
- Fresh validation: handler gate **1 passed**; library **183 passed, 47 ignored**;
  baseline boot **failed as above**, first PID M041 (other PIDs not reached).
  `git diff --check` passed. Logs: `/tmp/cloudy-af-irq42-regression.log` and
  `/tmp/cloudy-af-irq42-boot-confirm.log`.
- Graph service unavailable; evidence came from direct source reads and host
  Capstone disassembly, not a current graph or exhaustive call-graph review.
- Next software work: implement/test bounded SysTick register semantics, then
  identify the peripheral register contract and model IRQ42 delivery without
  forcing the RAM flag. Static-audit/watchdog plan items remain open. Hardware
  probing stays deferred; no device access or flashing occurred.


## Ground truth settled earlier (do not reopen)

- Framebuffer packing is **horizontal MSB-first** (proven via live 0xC1
  screenshot on the Pico 25 + NToolbox decompile).
- Diagonal sweep fixed: `xg=(i&7)<<1`, emitter `lsls 29`.
- 162/162 Rust tests pass, both `#[ignore]`d pre-flash gates pass.
- SWD hardware plan (`docs/superpowers/plans/2026-09-13-swd-hardware-hacking.md`)
  written but **DEFERRED** — user has no physical tools yet.
- §2 (full-boot PID emulation) was RE-blocked (2026-09-13) — **UNBLOCKED
  2026-09-14**: `resources/boot/af_190602.json` (the M0 RE freeze recorded in
  goals.md) now documents the whole reset→main path. Work resumed — see
  Task 5 (2026-09-14).

## Environment constraints

- Rust builds/tests run offline in the toolbox:
  `toolbox run -c arcticfox-build sh -c 'cd /var/home/j/Documents/GitHub/cloudy-af/src-tauri && cargo test --offline --release …'`
- Host python3 has capstone 5.0.7 (no `insn.asm()` — use
  `insn.mnemonic + " " + insn.op_str`; wide mnemonics carry the `.w` suffix —
  strip with `.removesuffix(".w")` before comparing).
- RE artifact (gitignored): `AF_fw/decrypted/af_190602.dec.bin`.
- Nothing git-committed yet for this plan.

## Task 5 — §2 full-boot per-PID dispatch emulation — IN PROGRESS (2026-09-14)

Plan written: `docs/superpowers/plans/2026-09-14-fullboot-pid-dispatch-emulation.md`
(225 lines, 5 tasks; untracked, uncommitted). §2 is unblocked by the boot
descriptor — no new RE required. Open-items review this session also
reconfirmed: SWD plan deferred (hardware), uhid double LOW/orthogonal,
macOS CI waiting on a push + manual workflow_dispatch, FW-Editor tabs +
AppImage flicker gated on delegate output that does not exist
(`/tmp/cline-queue.log`, `/tmp/cline-tasks/T3-report.md` both absent).

RE findings (this session, from `AF_fw/decrypted/af_190602.dec.bin`):

- The dispatch function at `0x302C` is NOT the boot dispatch — it has no BL
  callers anywhere in the image. The PID→panel decision happens earlier:
  `dataflash_load_settings` (`0x16C64`, listed in the boot descriptor's
  `main.init_calls`) reads the dataflash, then `main` reaches the dispatcher
  `0xD684`. The TBH at `0x3040` is a 37-entry indirect table whose targets
  (`0x3048 + 2*N`) land in the `0x30A0+` region; treat `0x302C` as unmapped
  for the boot gate.
- The hang target `0x2FF8` is a real infinite loop: `push.w {r4-r7,lr}`;
  `movs r2,#1`; store to a flag; `b 0x3004`. Reaching `0x2FF8` == dispatch
  failure (the Pico Dual hang mode).
- Combined-image trick: no new Bus plumbing. Place the 2048-byte dataflash at
  `0x1F000` in the flash image; the existing `Bus::read` flash path serves
  dataflash reads. PID at dataflash offset 316 = `img[0x1F13C..0x1F140]`.
- Descriptor note: the boot json has a DIFFERENT schema than the emulator's
  `Descriptor` (no top-level `render_entry`/`display_buffer`), so
  `load_descriptor(boot.json)` fails. The boot test must load
  `resources/animations/af_190602.json` as the Descriptor and take the
  reset/SP/hang constants from the boot json (hardcoded in the test for the
  first cut).
- ACL note: boot writes will far exceed the animation descriptor's allow
  lists — bss zeroing spans `0x20000058..0x200031C0` while `ram_globals` only
  covers `0x20000000..0x20000E00` + `0x20002C00..0x20002D00`, and the stack
  allow region depends on the descriptor's `ram_size` (verify it contains
  boot SP `0x200031C0`). `boot_until_settle` intentionally does NOT check
  `acl_violations` for the first cut; expect recorded (benign, applied)
  violations during boot. Later: a boot-specific descriptor with allow lists
  derived from the boot json's memory map.

Implementation state (`src-tauri/src/firmware/tests/emu_test.rs`) — LANDED,
FILE DOES NOT COMPILE YET (fix the defects below first):

- Plan Task 1 (done): consts `COMBINED_IMAGE_SIZE` (0x1F800),
  `DATACFLASH_OFFSET` (0x1F000), `DATACFLASH_SIZE` (2048), `FIRMWARE_SIZE`
  (0x1C7EC); `make_combined_image` (stock fw + 0xFF gap padding + dataflash
  at 0x1F000); `make_dataflash` (PID @316, fw version 110 @256 LE32 per the
  goals.md fw_versions=[110] observation, boot flag 0 @9);
  `test_combined_image_pins_pid_at_expected_offset` (PID @0x1F13C..0x1F140,
  dataflash byte 0 @0x1F000, gap is 0xFF).
- Plan Task 2 (partial): `boot_until_settle(h, budget, stop_pcs, hang_pc)` —
  sets PC=`0x1659|1`, SP=`0x200031C0`, LR=RETURN_SENTINEL|1, `min_sp`=SP;
  steps until PC ∈ stop_pcs (Ok(true) = settled), PC == `0x2FF8` (Ok(false)
  = hang), or BudgetExceeded. First-cut stop_pcs: {`0xD684` dispatcher,
  `0x8CD1` render entry} — the first run reveals which (if either) the boot
  path actually reaches; refine then.

KNOWN DEFECTS (fix before anything else):

1. Missing closing braces after `test_af_190602_patched_converges_when_gated_off`
   — the Task 1/2 block was inserted inside the test fn (right after the
   `for call` loop's last assert), so the `for` loop and the fn body are
   unclosed and the helpers + pin test are nested inside it (not collected as
   tests; compile errors). Fix: `    }` + `}` before the
   `/// Build a combined firmware+dataflash image` comment block.
2. `boot_until_settle` has `use super::cpu::Cpu;` — bogus: in
   `tests/emu_test.rs` `super` is `crate::firmware::tests` (no `cpu` child),
   and `Cpu` is not referenced by name; drop the use.
3. `Harness::RETURN_SENTINEL` — wrong path: RETURN_SENTINEL is a module const
   in `firmware::emu::harness` (see `run_at`:
   `use crate::firmware::emu::harness::RETURN_SENTINEL;`). Use that path.

NEXT (plan Tasks 2–3): add `test_af_190602_boot_dispatch_by_pid`
(`#[ignore]`, same artifact gate as the layer-4/5 tests —
`resources/boot/af_190602.json` + `AF_fw/decrypted/af_190602.dec.bin`, skip
when absent). Per-PID matrix: M041/M065/M077/M045/M038/M037/M095/M105/M064 →
Ok(true); M177 → documented skip (STM32 line in a Nuvoton image); negatives
`XXXX` / `0xFFFFFFFF` / `0x00000000` → Ok(false) (hit `0x2FF8`). First-cut
budget 10M instructions. Plan Task 4 (watchdog model) is a stretch; plan
Task 5 is doc bookkeeping (`docs/firmware-validation.md` §2 + goals.md
entry).

Run: `toolbox run -c arcticfox-build sh -c 'cd src-tauri && cargo test
--offline --release --lib firmware::tests::emu_test -- --ignored'`

Doc drift to reconcile: goals.md's uhid entry still says NOT YET RUN LIVE
while Task 3 above records 5 consecutive live PASS runs — verify which is
current before trusting either. Nothing from this session git-committed yet.

## Task 9 — all four effects reworked as self-drawn animations — PAUSED MID-DEBUG (2026-09-16)

Goal per user: the fade family only *cleared* bytes below a sine threshold, so
the stock charge text stayed visible (dimmed/banded) — "manipulated text". Every
effect must instead be a **uniquely enumerated animation that draws its own
content**: clear the framebuffer, then paint a distinct motif. Wave (config 5)
already did this and is the model.

**Implemented (uncommitted, working tree):**

- `effects.rs` module doc rewritten: each effect is a self-contained drawing.
- Shared cave helpers replace the old `emit_effect_cave(emit_index)` closure:
  `emit_cave_prologue` (replay `tst.w`, config gate, phase bump into r4),
  `emit_cave_epilogue` (restore r2/r3 + replay `tst.w`, `b_abs(hook_resume)`),
  `emit_cave_pool`, `emit_clear_frame` (wipe `buf[0..len]`), and
  `emit_set_px(y_reg, x_reg)` (OR one bit at `((y>>3)<<6)+x`).
- Rust drawing references + emitters, one distinct motif each:
  - **Gradient Bar (2)**: solid 12-row horizontal bar, top =
    `((phase>>2)<<3)&127`, wraps; period 64 phases.
  - **Center Pulse (3)**: panel-scaled ellipse rings, half-width
    `a = 4 + (phase&7)*4` (4..=32), rows `64 ± isqrt(a²-dx²)*2`; period 8.
  - **Diagonal Sweep (4)**: thick diagonal bar, `c = (phase*6) mod 192`,
    rows `y = c + d - x`; period 32.
  - Wave (5) unchanged.
- `Asm::subs_reg` added (T1 `0x1A00` form) + golden/emu-decode regression tests —
  the wave had been emitting `0x41xx` op-nibble 6, which is **SBC**, not SUB.
- Test module reworked: old "fade" tests replaced with drawing-semantics tests
  (`test_gradient_bar_sweeps_vertically`, `test_center_rings_are_symmetric_and_pulse`,
  `test_diagonal_bar_marches_corner_to_corner`,
  `test_every_effect_draws_its_own_content`,
  `test_draw_is_independent_of_stock_content` — same phase from an all-on and an
  all-off screen must produce identical frames, which is the formal
  anti-"manipulated text" property).
- `src-tauri/src/bin/anim_dump.rs` (new dev helper, untracked): assembles a cave
  and disassembles it through the emulator's own decoder.

**Status (2026-09-17): RESOLVED at the emulator level — see Task 10.**

## Task 10 — Task 9 finished (all four effects green); boot runaway refined (2026-09-17)

Resumed the paused Task 9 work in the primary repo and drove it to green.

**Root causes fixed in the emitters (`src-tauri/src/firmware/anim/effects.rs`, uncommitted):**

1. `emit_set_px` computed the bit mask as `(y & 7) << (y & 7)` instead of
   `1 << (y & 7)` (the `movs r7, r7` "1" literal was the register index, not
   the value). Gradient wrote 0xFA/0x1A where the reference has 0xFF/0x0F.
   Now: shift count in r5, `lsls r7, r5`. Contract corrected: scratch
   r5/r6/r7, preserve r0-r4; `debug_assert!` now REQUIRES y/x in r0/r2/r3/r4.
   Regression `test_emit_set_px` (6144 cases: every row, boundary columns,
   4 initial byte patterns, both x-register conventions, guards, r0-r4).
2. Center Pulse: (a) the `dx² > a²` compare and the isqrt candidate compare
   used `cmp <regA>, <regB>`-INTENDED code emitted as CMP-immediate — the
   immediates were register numbers, so the branches were flag-garbage
   (caused the ACL out-of-frame writes 0x20000FDE..E2 via a bogus sqrt
   result); both are now real CMP-reg via `raw16(0x4280 | (Rm << 3) | Rn)`
   (capstone-verified `cmp r6, r5` / `cmp r0, r6`). (b) isqrt subtracted
   t² from the radicand (classic non-restoring bug) — replaced with the
   restoring high-bit-first loop seeded at 32. (c) mirror row used
   `64-(64+dy)` (wraps to 128-dy); now `128-(64+dy)` = 64-dy, and row 128
   is skipped (HS on cmp 128) — reference drops y=128 via set_px bounds.
3. Diagonal Sweep: (a) bounds check was `cmp y,128`+LO-store → row 128
   stored at 0x2000_0FDE (ACL catch); now `cmp 127`+HI skip. (b) The
   `(phase*6)%192` subtraction loop is O(phase) — phases ≥ ~30k exceeded
   the 100k budget. Replaced with `(phase & 31) * 6` (bounded, identical
   sequence since 32·6 = 192); Rust reference reduces before multiplying
   (overflow-parity). The earlier `cmp r4, r3` reg-encoding repair inside
   that loop is superseded (loop deleted).

**Harness (`effects.rs` tests):** one `#[test]` per effect
(`test_emitted_caves_{gradient,center,diagonal,wave}_matches_reference_in_emu`);
phases 1..=128 × {all-off, all-on} + two multi-megaphase cases; strict ACL
(only phase word + framebuffer allowed; setup writes pre-cleared), 64-byte
guards, dropped-write check, r0-r4 preservation, resume PC, r2/r3/flags
contract, single phase bump.

**Full-cycle gates:** `gate_animation_frames` now replays 1..=64 (full
gradient cycle; covers all shorter cycles incl. clipping edges) for both
builds; `anim_shots` PHASES = 64; GIFs regenerated under
`tmp/anim-shots/<build>/<effect>/` (gitignored).

**Results:** `firmware::anim` 30/30; full lib 182/182 (46 hardware-gated
`#[ignore]`d as designed); render gates af_190602+af_190624 PASS;
animation gates both builds PASS (64-phase byte-exact vs reference:
gradient 768 / center 16 / diagonal 55 / wave 128 final on-pixels,
identical across builds). Emitted bytes double-checked through host
capstone 5.0.7 decode (cmp encodings, `movs r5, #0x80`, `push {r3-r7,lr}`).

**§2 boot runaway — refined, NOT fixed (next session's entry point):**
stock-image boot gate (M041) still budget-fails: after 211,989 insns PC
leaves the image into the GPIO block and the budget dies. A TEMP value
watchpoint (added then REVERTED; bus.rs is clean again) caught the push
site: **pc 0x00000a80 = `push {r3-r7, lr}`** parks 0x40050020/0x40050000
(GPIO bases in r5/r6/r7) at 0x2000314C/0x20003150 with lr=0x00000a8d (the
bl 0xa88 return slot). At the stray transition sp=0x20003150 sits on that
push window and lr=0xa8d — so the runaway is an emulator RETURN-PATH bug
(pop / IT-block skip size / pushed-LR divergence per Task 7's suspect),
not a wild store and not animation-cave involvement (the boot gate runs
the UNPATCHED image). Next session: single breakpoint at the epilogue
that pops into 0x40050020 (candidates 0x9bd6-family / 0xa46-family),
compare pop+BX semantics vs ARM ARM.

**Doc reconciliation:** the "§4 static audit DONE" tooling
(`scripts/audit-cave.py`, `anim_patch_dump.rs`) and the
`test_af_190602_patched_converges_when_gated_off` golden gate exist only in
the older Documents mirror (`~/Documents/GitHub/cloudy-af`), not in this
primary repo, and that audit ran against the PRE-rework emitters — treat
Tasks 1/2 as needing a rerun against current code. §5 uhid double IS here
(scripts/uhid_ldrom.py) with 5 recorded live PASSes.

**Known review gaps (not blockers):** GIF previews use unpack_block1
(vertical packing); the handoff's 2026-09-13 "horizontal MSB-first"
ground-truth note contradicts both the current RE docs (pixel
`x+(y/8)*width`, bit y%8, incl. dispatcher-analy §5.4) and the live-Pico
gate history — re-verify at the bench before trusting visual orientation.
uhid gate has a TOCTOU race (list_devices once, then open_device() by
VID/PID) — fine as a dev gate, never wire it to CI against real devices.

**Bugs already found and fixed during this rework (worth remembering):**

1. `Asm::ls_reg(op, rt, rn, rm)` **swaps rn/rm** relative to the ARM ARM: the
   Thumb-1 T1 form is `0101 op L Rm Rn Rt` (Rm in bits 8:6). The emulator decodes
   it correctly, so the swap is invisible whenever the operands are interchangeable
   (the address is `r[rn]+r[rm]`, commutative) — which is why the old fades worked
   by accident. Only `rt` matters and that one is right. **Do not "fix" the
   encoder without re-verifying every effect**, but do not rely on rn/rm order.
2. `0x4180` in the 0x40xx data-processing group is **SBC (op 6), not SUB**; the
   correct T1 `subs rd, rn, rm` is `0x1A00`. This made the wave's lower-half
   `y = 64 - off` carry-dependent. `Asm::subs_reg` now exists for this.
3. `emit_set_px` originally clobbered **r2** (gradient/diagonal loop state) and
   then **r1** (the framebuffer base — the center pulse's `muls r1,r1` produced
   `Unmapped { addr: 0x1001df }`). It now scratches only r5/r6/r7 and preserves
   r0/r2/r3/r4, with a `debug_assert!` rejecting r1/r6/r7 as y/x.
4. Test-side: `gradient_fade_apply` legitimately fills whole bytes, so "no 0xFF
   may survive" was the wrong assertion; `test_wave_renders_new_content_not_fades_text`
   underflowed on `*b - 1` for zero bytes; the ring mirror is about y=64 so it is
   `128 - y`, not `127 - y`.

**Environment note:** the host disk hit 100% (`df`: 144M free) which broke the
toolbox container and produced a bogus `could not compile` until
`src-tauri/target/debug/incremental` (2.4G) was deleted. Reclaim that dir first
if cargo fails with a container/rollback error.

**Next steps:** (1) fix `emit_set_px`'s byte-index/bit-mask for the solid bar so
the gradient cave matches its reference; (2) re-run the full anim suite;
(3) run both `#[ignore]`d emu gates (af_190602 + af_190624) and the `anim_shots`
GIF capture for all four effects (user-requested visual check);
(4) commit; (5) update this log and the phase-3 plan's task checkboxes.

## Task 8 — af_190624 port: animations support the newer build (2026-09-16)

Goal per user: redevelop animations against the CURRENT AF firmware with
current knowledge + prepare the physical Nu-Link probe session.

- **Build ground truth:** `af_190624.dec.bin` (116720 B, decrypted, vectors
  SP=0x200031A8 reset=0x1659) is the newest Nuvoton-line build in hand.
  af_211009 is the STM32 line (out of scope for the Nuvoton animation path).
- **Porting method (pattern-anchored, no Ghidra needed):** the 19.06.24 code
  is the 19.06.02 code with (a) early functions shifted +0xBC (hook host
  0x9A10→0x9ACC, render 0x8CD1→0x8D8D, tst.w hook site 0x9A16→0x9AD2 — 240-246
  of 256 bytes match at the shift; only literal-pool words differ), (b) RAM
  globals shifted −0x18 (status base 0x20002C30→0x20002C18, framebuffer
  0x20002758→0x20002740; literal-count parity 221/222 and 10/10 confirms),
  (c) the JWEI marker at 0x1BF13 and the dataflash layout UNCHANGED (data
  regions don't shift), (d) code-cave scheme unchanged (cave appends past
  image end 0x1C7F0; config byte still dataflash offset 0x7F0 = 0x1F7F0).
  Phase global: 0x20002CE0 (the −0x18 analog of 0x20002CF8; zero literals).
- **Descriptor:** `resources/animations/af_190624.json` (committed). The
  effects/asm/patch code is build-agnostic given the descriptor — no Rust
  changes were needed for the port itself.
- **Gates made build-parametric:** `gate_render(build)` /
  `gate_animation_frames(build)` in emu_test.rs now derive CLOCK_RENDERER
  (hook_site−6), PHASE_GLOBAL and STATUS_WORD from the animation descriptor;
  new `test_af_190624_render_gate` + `test_af_190624_animation_frames`.
- **Empirical proof:** af_190624 passes BOTH gates first try — render 409
  on-pixels, and all three effects animate with on-pixel signatures identical
  to af_190602 (gradient → 0, center 409 @phase 4, diagonal 196 @phase 4).
- **Visuals:** `anim_shots af_190624` → 17 frames/effect under
  `tmp/anim-shots/af_190624/<effect>/` incl. animated GIFs.
- **Bench instructions:** `docs/2026-09-16-nulink-bench-probe-list.md` —
  safety, wiring, attach (OpenOCD + Nu-Link/Wine paths), and the nine
  READ-ONLY probes (on-device build identity, FMC identity, LDROM truth +
  SKU block, dataflash size, live framebuffer popcount 409 vs both
  descriptors, phase-global spare stability, config-byte cell, 0x4005_0000
  block, JWEI) with the emulator assumption each probe confirms or kills.
- Note: which build the user's Pico actually runs gets settled by probe D1
  (dataflash build stamp 516: `13 06 02` vs `13 06 24`); both are now
  supported by the patch pipeline either way.

## Task 7 — SKU scan root-caused: FMC LDROM scan, not dataflash; FMC model lands (2026-09-16)

Session goal: advance §2 as far as the emulator allows before the Nu-Link clip
session. Wins, in order of discovery:

1. **"Block-copy engine" re-identified: it is the Nuvoton FMC** (flash
   controller, M451 base 0x4000C000: ISPCON/ISPADR/ISPDAT/ISPCMD/ISPTRG at
   +0x00/+0x04/+0x08/+0x0C/+0x10). The Task-6 write pattern was ISP command
   sequences: [0x0C]=cmd, [0x04]=addr, [0x08]=data, [0x10]=trigger-then-poll.
2. **SKU scan re-identified: it reads the LDROM, not the dataflash** — both
   Task-6 hypotheses (dataflash walk; 0x20000CA4 RAM scan buffer) DISPROVEN
   (0 reads of that buffer ever happen). Scan function 0x302C: gates on the
   "JWEI" marker at APROM 0x1BF13, then FMC READs (ISPCMD=0) 0x00100000..
   0x00100FFF (1024 words = the r5→0x1000 count), comparing each word
   against ~60 4-char device-ID literals (E052/E115/E043 in r6-r8 + pools at
   0x32F4-0x3388 and 0x34B8-0x3528; **M041 at 0x3324**). The real LDROM
   carries "M041" at offset 0x878 (HIDC at 0x5C7) — no pool ID ever appears
   in the 2 KiB dataflash dump, so no dataflash content could have matched.
   This fully explains the "scan-exhaust" hang on real and synthetic images.
3. **FMC ISP model implemented** (`emu/bus.rs::Fmc`): word writes latch
   ISPADR/ISPDIN/ISPCMD, ISPTRG:=1 executes; ISPDAT reads serve the result;
   ISPTRG reads 0 (instant). Cmd 0 READ serves the LDROM (dump attached at
   0x00100000) or the flash image; cmds 4/0xB/0xC (UID/CID/DID) return
   deterministic dummies — the boot only STORES them (globals at
   0x20000DA4+0x10C.., disasm 0x3830-0x3892, nothing compares). Nu-Link
   bench: read real CID/DID/UID and refine.
4. **LDROM fixture**: `test-fixtures/ldrom/ldrom_m041_16k.bin` (16 KiB,
   de-shifted dump, vectors SP=0x200014a8 reset=0x165) — gitignored like
   the rescue kit (test-fixtures/), copied from
   `scratch-stash/DecryptProject/ldrom/ldrom_m041.bin`; artifact-gated in
   the boot test like the dataflash dump; the two old FMC stubs removed.
5. **Unaligned LDR/STR legalized** (ARMv7-M permits them; the boot genuinely
   loads a word at 0x20000161 — the old strict-alignment check was wrong).
   `check_align` retained but unused; 2 unit tests rewritten to the correct
   semantics (112/112 emu tests pass).
6. **Extend family added** (UXTB/UXTH/SXTB/SXTH, 0xFA0F-family op nibble
   0/1/4/5): the third honest signature was undefined `FA1F FC8C`
   (uxth.w ip, ip) at 0x2A42.
7. **Current honest signature (next session's input)**: M041 now boots
   through ALL init (clock, FMC identity reads, SKU scan match, init-table
   copy) and runs the main event loop to ~212K instructions, then a
   `pop {r4,r5,r6,pc}` at 0xA46 returns to **0x40050021** — the popped frame
   {r4=0xa, r5=0x40051020, r6=0x40051000, pc=0x40050021} (values match the
   accessor literals at 0xA68-0xA7C) is a pushed-LR divergence, likely
   another decode gap (IT-block skip sizes are prime suspect — the code
   around the caller uses `itete ne` over mixed 16/32-bit slots). Stray-pc
   probe is in `boot_until_settle` behind `CLOUDY_STRAY_TRACE` (logs pc,
   sp, lr, stack window, last 24 PCs).

### Emulator → Nu-Link bench list (updated)

- FMC ISP CID/DID/UID real values (currently dummies 0xDA / 0x0D421000 /
  0x13572468…) and confirm the boot never branches on them.
- 0x40050000 block semantics (accessor fns 0xA0C-0xA56: struct at +0x20,
  +0x1000, +0x1020) — read live registers before the clip session.
- The 0x1BF13 "JWEI" marker and the LDROM SKU-block provenance (factory-
  written per-unit? one M041 table for all PIDs?).
- Full LDROM length (16 KiB assumed from the dump; verify with ICP).

### Visual-check tooling (same session)

- `src-tauri/src/bin/anim_shots.rs`: dumps 16 emulator phases per effect
  (gradient/center/diagonal) as PGMs into a folder — same drive as
  `test_af_190602_animation_frames` (prime → timeout bits → config byte →
  run_at CLOCK_RENDERER per phase); on-pixel counts match the gate
  (gradient → 0 immediately, center 409 @phase 4, diagonal 196 @phase 4).
- `scripts/anim_shots_to_gif.py`: PGMs → PNG (+4x scaled) and one animated
  GIF per effect (Pillow; identical consecutive frames merged).
- Run: `toolbox run -c arcticfox-build sh -c 'cd src-tauri && cargo run
  --offline --release --bin anim_shots'` then `python3
  scripts/anim_shots_to_gif.py tmp/anim-shots` (output gitignored under
  tmp/).

## Task 6 — emulator advance: uhid gate green + STR-writeback decode defect FIXED (2026-09-16)

Session goal: advance the hardware plans as far as the emulator allows
(Nu-Link plugged; physical clip-probe session later). Everything below ran
in `/var/home/j/cloudy-af` (mirror `/var/home/j/Documents/GitHub/cloudy-af`
synced at the end).

### Wins

1. **§5 uhid flasher gate GREEN in the primary repo** —
   `test_flash_uhid_double` passes live: full 0xC3 WriteData stream
   (start=0x0, len=1024), stream complete → "update committed, boot flag
   cleared", then 0xB4 Restart with boot flag 0. This was previously only
   proven in the GitHub mirror; now green here, closing the goals.md doc
   drift noted under Task 5.
2. **Boot gate root cause found AND fixed** (see below) — the M041
   per-PID boot test no longer spins in the 0x5EC copy loop.

### Root cause of the boot copy-loop hang (emulator defect, not firmware)

`boot_until_settle` on PID M041 exceeded budget spinning at PC 0x5EC–0x60A.
Full-register dump at budget death: r0=0x20003148 (src ptr, NEVER
advancing), r1=0x20003150 (end), r2 toggling 0x00300000→0x00000000
(`[r3,#0xC]` endianness flag), r3=0x4000C000 (block-copy engine regs),
r11=junk 0x24003548, lr=0x38f1.

The loop body is `ldr r2,[r3,#8]; str r2,[r0],#4; b` — and the store
`str r2, [r0], #4` = **F840 2B04** (Thumb-2 LDR/STR word immediate, T4
writeback-shape encoding) matched NO store arm in `decode32`:

- `StrImm` (hw1 0x08C0) — different hw1 bits 7:4 (this is 0x0840);
- `StrbT4`/`LdrbT4` (byte forms) — wrong size;
- `LdrStrReg` — requires hw2 bits 11:6 = 0; here hw2 bit 11 = 1.

It fell through to the catch-all DpImm arm and decoded as
**`orr r11, r0, #4`** — which is exactly why r11 held garbage. The store
never fired, r0 never advanced, `cmp r0, r1` never passed. (Capstone
5.0.7 also mis-renders the STR-word pre-indexed form —
`str.w r7, [r1, #0xd08]` — so encodings were verified against the LOCAL
ARM ARM: `/var/home/j/Documents/DDI0403E_B_armv7m_arm.pdf`, A7.7.42.)

### The fix (thumb.rs + cpu.rs)

- New variant `Thumb2::LdrStrT4 { load, rt, rn, imm, pre, sub, wb }`:
  hw1 = 0xF840|Rn (STR) / 0xF850|Rn (LDR), hw2 = Rt 1 P U W imm8.
  P=1&&W=0 (LDRT/STRT space) left unclaimed; P=0&&W=0 → UNDEFINED
  (returns None) per ARM ARM.
- New halfword sibling `Thumb2::LdrhStrhT4` (same field shape): hw1 =
  0xF820|Rn (STRH) / 0xF830|Rn (LDRH) — the LDRH/STRH **T3** writeback
  encodings (ARM ARM DDI0403E.b A7.7.54 / A7.7.167). Firmware hit:
  `ldrh r2,[r3,#2]!` = **F833 2F02** at 0x2414 — the boot init-table copy
  (r3 0x2000026A → r4 0x20000366, 126 halfwords str'd into 0x40031004)
  previously fell through to DpImm garbage; SECOND budget death after the
  word-store fix, same registers every run.
- **Encoding-geometry lesson (cost one wrong turn):** the writeback forms
  are separated from the plain imm12 T2 forms by the **hw1 bits 7:4 size
  nibble** (byte 0x0/0x1 vs 0x8; half 0x2/0x3 vs 0xA/0xB; word 0x4/0x5 vs
  0xC/0xD) — NOT by hw2 bit 11. In the T2 imm12 spaces hw2 bit 11 is a
  plain offset bit (`str.w r0,[r8,#0x800]` is legal). A first pass added
  `hw2 & 0x0800 == 0` guards to StrImm/LdrImm/LdrhImm; after checking the
  PDF encodings they were REVERTED (they would have mis-routed legal
  imm ≥ 0x800 T2 forms into DpImm). Regression guards now pin the
  imm ≥ 0x800 case to the imm12 arms: F8B8 0A00 (ldrh.w r0,[r8,#0xa00]),
  F8D8 2000, F8CD 8014. The legacy `hw2 & 0x0800 == 0` guard on StrbImm
  (0xF880|Rn) is the same latent flaw — no firmware site hits imm ≥ 0x800
  so it is left, with an in-file NOTE to fix if one appears.
- Executor per A7.7.42 semantics: `offset_addr = Rn±imm8`;
  address = pre ? offset_addr : Rn; writeback (W=1) ALWAYS sets
  `Rn = offset_addr` (both P forms), memory access ordered before the
  register write; Rt == 15 load = branch (LoadWritePC, `v & !1` per the
  established Ldmia/pop convention) with writeback still applied —
  this un-breaks the `ldr pc, [sp], #4` exception-return epilogues at
  0x9bd6/0xa836/0xd12e/0xc086/0x113c8/0x17662. LdrhStrhT4 executor is
  identical with a zero-extended halfword access (no pc-target form).

### Firmware encodings this un-breaks (aligned walk of af_190602)

| Enc | Site(s) | Instruction |
|---|---|---|
| F840 2B04 | 0x606 | str r2,[r0],#4 — THE boot blocker |
| F841 0B04 | 0x23b4 | str r0,[r1],#4 |
| F841 2C04 | 0x6ade/0x17b8e/0x11564* | str r2,[r1,#-4] (negative offset — impossible in imm12 form) |
| F841 2D04 | 0x11564 | str r2,[r1,#-4]! |
| F850 5F04 | 0x628 | ldr r5,[r0,#4]! (sibling copy loop) |
| F851 3B04 / F854 1B04 | 0x3552/0x3538 | ldr r3/r1,[rn],#4 |
| F85D EB04/FB04 | 6 sites | ldr lr/pc,[sp],#4 epilogues |
| F833 2F02 | 0x2414 | ldrh r2,[r3,#2]! — SECOND boot blocker (init-table copy into 0x40031004) |

### Tests added

- Decode (thumb.rs): word post-add-wb / pre-sub-no-wb / pre-sub-wb /
  ldr-pre-add-wb / ldr-pc-post; halfword T3 pre-add-wb (the F833 2F02
  blocker) and post-add-wb; anti-shadowing guards that pin imm ≥ 0x800 T2
  forms to StrImm/LdrImm/LdrhImm (the reverted-guard regression).
- Executor (cpu.rs): post-indexed store advances base; pre-indexed no-wb
  leaves base; ldr pc,[sp],#4 branches AND advances sp; ldrh [r3,#2]!
  loads zero-extended and writes back r3 = r3+2.

### Budget note

`test_af_190602_boot_dispatch_by_pid` BUDGET raised 10M→200M with rationale
comment: the 0x4000C000 engine handshake costs ~13 emulated insns per copied
word (program [0xC]=0, [0x10]=1, poll [0x10]==0, copy [8], loop at 0x5EC)
vs a few cycles on silicon. ~30M insn/s emulated → 200M ≈ 7 s wall clock
(observed 6.49 s at 200M). 10M died mid-copy with the OLD decode bug; that
specific exhaustion is now moot (copy terminates), but the raised budget
stays for the remaining boot path (bss zeroing etc.).

### Emulator-verified probe points for the physical Nu-Link session

Silicon addresses the emulator now confirms are load-bearing during boot
(clip-probe / Nu-Link memory-read targets, `docs/firmware-validation.md`):

- LDROM dispatch table @ **0x110** (PID-keyed).
- Block-copy engine regs @ **0x4000C000/04/08/0C/10** (program/data/src/dst/
  trigger-poll) — copy helper at 0x5DC drives them.
- Boot copy bounds observed: src **0x20003148**, end **0x20003150**.
- Dispatch-failure hang loop @ **0x2FF8** (Pico Dual hang mode).
- Exception-return epilogues now correct: `ldr pc,[sp],#4` @ 0x9bd6 et al.

### Third honest signature: LDROM SKU scan over the dataflash (2026-09-16 later)

With both decode fixes in, boot ran through ALL init (clock, GPIO, ADC,
0x40031000 table upload) and died INSTANTLY (no budget) at the dispatch
decision: `beq.w 0x30BC → bl 0x2FF8` at 0x34B2. Full regs decode the path:

- 0x309C–0x34B0 is a **SKU scan loop** over the dataflash image: r1 walks
  the dataflash (start 0x1F00xx), r5 counts to **0x1000**, Eleaf SKU-string
  ASCII prefix literals sit in the pool at 0x34B8+ ("E052"/"E115"-style
  chunks left in r6/r7/r8 at death) — the LDROM scans the dataflash config
  block for a matching SKU prefix, not just the PID dword at offset 316.
- A sparse synthetic dataflash (PID + fw-version + bootflag only) therefore
  **exhausts the scan → hang even with a valid PID**. Real rescue dumps
  (test-fixtures/rescue/dataflash_stock_v1.00.bin, real M041) carry a full
  config block (e.g. printable "U25TE" at df[0x124] beside M041 at 0x13C).
- Fix in test (not emu): the known-PID matrix now boots the REAL dump,
  PID-patched at df[316..320] per matrix entry (artifact-gated: skipped
  with an eprintln when the dump is absent); the negative cases (XXXX etc.)
  deliberately keep the sparse synthetic image, where scan-exhaust → 0x2FF8
  is exactly the expected honest outcome.

### Next

Re-run the M041 gate (expect pass or a NEW honest signature — next MMIO
stub or decode gap); then remaining PIDs per Task 5's matrix.
(Updated: the matrix now runs on the real dump; see SKU-scan section above.)

### SKU-scan outcome on the real dump + 4 KiB hypothesis (2026-09-16 latest)

The real-dump matrix run still exhausts the scan for M041 (same instant
0x2FF8 hang, no budget death). New observation from the loop shape:
0x30AE counts r5 0→**0x1000** in **+4** steps = **1024 word reads = a
4 KiB dataflash window**, but every artifact we hold is 2 KiB
(`dataflash_backup.bin`, `dataflash_stock_v1.00.bin` both 2048 B, and
`DATACFLASH_SIZE` = 2048). The upper 2 KiB of the scanned window is 0xFF
padding in the combined image. Hypothesis (next session's first probe):
the real device dataflash is 4 KiB and the rescue dumps are truncated
readbacks; the SKU block the scan matches lives in the missing half.
Bench check for the Nu-Link session: read the full dataflash region via
Nu-Link ICP (config-reg width first) and diff against the 2 KiB dumps —
if bytes 2048..4096 are real data, extend `DATACFLASH_SIZE` to 4096 and
place the dump accordingly. If the device flash really is 2 KiB, the scan
window must start elsewhere (r1's initial value is the thing to trace) —
capture r1/r5 at scan entry with a breakpoint at 0x30AE.

## Task 1 — §4 static audit — DONE (2026-09-13)

- `src-tauri/src/bin/anim_patch_dump.rs`: dumps patched firmware +
  modification metadata. Run:
  `toolbox run -c arcticfox-build sh -c 'cd src-tauri && cargo run --offline --release --bin anim_patch_dump'`
  → `/tmp/audit/{stock,gradient,center,diagonal}.bin` + `*.mods.json`.
  Rollback byte-exact for all 3 effects (gradient/center: 121 mods, 116 B
  body; diagonal: 125 mods, 120 B body).
- `scripts/audit-cave.py`: capstone static audit of the emitted Thumb cave.
  Checks: diff confinement vs stock, hook is a single `b.w` to cave_start,
  contiguous code decode up to the terminating `b.w hook_resume`,
  instruction whitelist, store discipline (`strb rN,[rN,rN]` framebuffer /
  `str rN,[rN]` phase only), branch bounds, literal-pool containment
  (every pc-relative `ldr` lands in the aligned pool after the terminator),
  config byte ∈ {2,3,4} on an erased flash cell, erased cave tail.
- Run: `python3 scripts/audit-cave.py /tmp/audit/<e>.bin /tmp/audit/<e>.mods.json /tmp/audit/stock.bin`
- **Result: PASS ×3** (gradient, center, diagonal).
- Cave layout discovered (matters for future emitter changes): code is
  contiguous from cave_start and ends at the `b.w hook_resume`; then an
  optional 0x46C0 alignment nop; then a 4-byte-aligned literal pool
  (config byte addr 0x0001F7F0, phase-global addr, framebuffer base,
  sine-table base, MMIO reg addr, saved-reg addrs). body_len includes the
  pool — linear disassembly past the terminator desyncs, which is why the
  audit splits code/pool at the terminator.
- Ollama (`qwen3-coder`) drafted the first version of audit-cave.py; it was
  reviewed and heavily corrected by hand (real store regex, insn.size
  summing, md.detail, capstone-5 API). Do not trust unreviewed local-model
  output — per AGENTS.md.

## Task 2 — §3 convergence + goldens — DONE (2026-09-13)

- `src-tauri/src/firmware/tests/emu_test.rs`:
  (a) self-goldening in `test_af_190602_animation_frames` — writes
      `AF_fw/goldens/{name}_phase{k}.bin` (12 files, under the already-
      gitignored `AF_fw/`) when absent, byte-exact-compares when present;
  (b) new `#[ignore]` gate `test_af_190602_patched_converges_when_gated_off`
      — gradient patch with config stub 0 vs stock harness, identical drive
      (prime, status bits 0x20000|0x80000, 4× run_at CLOCK_RENDERER 0x9a10):
      framebuffer byte-exact after every call, phase global stays 0.
- Verified with two consecutive runs of
  `cargo test --offline --release --lib firmware::tests::emu_test -- --ignored --nocapture`:
  3 passed (render_gate, animation_frames, converges_when_gated_off) both
  times — second run validated against the recorded goldens.

## Task 3 — §5 uhid double + flasher gate — WRITTEN + LIVE TEST PASSING (2026-09-13)

- `scripts/uhid_ldrom.py` (stdlib-only /dev/uhid LDROM double: VID 0416
  PID 5020, manufacturer `Nuvoton`, serial suffix `-uhid`, serves 0x35
  dataflash with boot flag user[9]=1, PID `M041` at user offset 312, fw ver
  110 at user 256; 0xC3 consume arg2 bytes with `--nak-delay-ms` /
  `--die-at-offset`; 0xB4 reboot no-op — the boot flag is cleared by the
  completed 0xC3 stream, not by 0xB4).
- uhid CREATE2 struct corrected to match linux/uhid.h **exactly** (checked
  against `/usr/include/linux/uhid.h` in the toolbox):
  `struct uhid_create2_req` (packed) =
  `__u8 name[128]; __u8 phys[64]; __u8 uniq[64]; __u16 rd_size; __u16 bus;
  __u32 vendor; __u32 product; __u32 version; __u32 country;
  __u8 rd_data[HID_MAX_DESCRIPTOR_SIZE=4096];`
  sizeof = 4368; +4 (uhid_event.type) = 4372 bytes total.
  Offsets (relative to start of uhid_event buffer):
  name=+4, phys=+132, uniq=+196, rd_size=+260, bus=+262, vendor=+264,
  product=+268, version=+272, country=+276, rd_data=+280.
  The report descriptor goes into `rd_data` (not a separate field), and
  `rd_size` = len(report_descriptor) must be sent or the kernel rejects the
  CREATE2 with EINVAL and the device never enumerates.
  name/phys/uniq are now populated (the real Nuvoton reports name="HID Transfer",
  phys="uhid-ldrom-double", uniq="A02015081302-uhid").
- udev rule: `KERNEL=="uhid", MODE="0666"` (sudo, host).
- Rust test `test_flash_uhid_double` in `flasher_test.rs` (exact code in the
  file). Gate: `list_devices()` must return exactly the double — the real
  Pico 25 (0416:5020) must be UNPLUGGED for this test. Verify `open_device()`
  in flasher.rs picks the first VID/PID match (it does: iterates
  SUPPORTED_DEVICES and returns on first `api.open(vid,pid)` success).
- Root cause of the early duplicate-device failure diagnosed: the Rust `hidapi`
  crate v2.6.6 on Linux defaults to the `linux-native` backend (libudev +
  hidraw, NOT libusb). The native backend reads the report descriptor from
  `sysfs/class/hidraw/HID/raw/device/report_descriptor` and creates one
  `DeviceInfo` per usage in the descriptor. The uhid double's report descriptor
  has ONE usage (vendor 0xFF00, usage 0x01, inside one Application collection),
  so the native backend creates exactly ONE DeviceInfo entry for it. The earlier
  "found 2 devices" failure was transient (no `hidraw6` exists after the test,
  and a standalone diagnostic mirroring the test's spawn+sleep+enumerate sequence
  consistently sees exactly 1 matching device). The flaky failure was most likely
  a leftover hidraw node from a prior run before the CREATE2 fix, or a brief
  kernel/duplicate-minor window during first creation — both resolved now that
  the CREATE2 event is correct.
- uhid event-loop semantics written against kernel header docs: UHID_OUTPUT
  events carry `struct uhid_output_req { __u8 data[4096]; __u16 size; __u8
  rtype; }`; offsets data=+4, size=+4+4096. The kernel gives NO backpressure:
  it silently drops OUT reports past its 31-deep ring (uhid_queue in
  drivers/hid/uhid.c), so host streams must stay below that per burst. The
  test's 1024-byte image = 16 data reports + 1 command = 17 events, safely
  under the limit. The write-retry path (50×50 ms backoff for real-hardware
  NAKs) cannot be exercised through uhid at all, for the same reason.
- LIVE TEST: 5 consecutive runs PASS (gateway check + full `flash_firmware_guarded`
  against the double, including 0xC3 stream, boot-flag-clear on stream complete,
  0xB4 restart no-op). Command:
  `toolbox run -c arcticfox-build sh -c 'cd src-tauri && cargo test --offline --release --lib firmware::tests::flasher_test::test_flash_uhid_double -- --ignored --nocapture'`
- NOTA BENE — value for animation firmware development: LOW/ORTHOGONAL. This
  test validates the **flasher protocol** (LDROM 0x35/0xC3/0xB4 semantics,
  stream pacing, boot-flag lifecycle), not the animation firmware itself. The
  animation firmware is validated by §1–§4 (effect property tests, full-boot
  per-PID dispatch emulation, deterministic framebuffer differencing + goldens,
  static cave audit), all of which are DONE and directly exercise the effect
  code/cave/framebuffer. The uhid double is a CI gate for flasher changes that
  must not touch real hardware; it is useful if the flasher protocol is being
  changed alongside animation patches, but adds zero coverage of the animation
  effects/cave/framebuffer/dispatch. Keeping it is reasonable as a safety net
  for the delivery path, but it is not a substitute for the §1–§4 animation
  validation and should not be mistaken for such.

## Task 4 — doc bookkeeping — DONE (2026-09-13)

- Marked §3/§4/§5 ✅ in `docs/firmware-validation.md` (also refreshed stale §1
  "index math (i & 63)" remark — `^7` is the per-row mirror, flat index is
  `(row << 6) | col`).
- Added `docs/goals.md` entry.
- Noted SWD plan deferred (`docs/superpowers/plans/2026-09-13-swd-hardware-hacking.md`
  written but DEFERRED — no physical tools yet).
