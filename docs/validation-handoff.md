# Validation & animation-patch work — running handoff log

Running log for the non-hardware validation plan
(`docs/superpowers/plans/2026-09-13-nonhw-validation-routes.md`, routes from
`docs/firmware-validation.md`). Newest entries first within each section.

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
