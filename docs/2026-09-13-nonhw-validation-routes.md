# Non-Hardware Validation Routes (§3 Differencing, §4 Static Audit, §5 uhid Double) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the three viable no-new-hardware validation routes from `docs/firmware-validation.md`: §4 static hook/cave audit, §3 deterministic framebuffer differencing (convergence + goldens), §5 virtual-HID flasher end-to-end. §2 (full-boot PID-dispatch emulation) is explicitly OUT of scope — it is RE-blocked (dispatch addresses undocumented).

**Architecture:** §4 = a tiny Rust dumper binary that emits stock+patched images and the patch modification list, plus a host-side Python/capstone audit script (host has capstone 5.0.7; toolbox cargo is offline so no Rust capstone dep). §3 = extend the existing layer-5 emu gate with a gated-off patched-vs-stock convergence test and self-goldening frame files. §5 = a dependency-free Python `uhid` double of the Nuvoton LDROM updater plus an `#[ignore]`-gated Rust test driving the real `flash_firmware_guarded` against it.

**Tech Stack:** Rust (existing emu/patch code), Python 3 + capstone (host), Linux `/dev/uhid`.

**Spec:** `docs/firmware-validation.md` §3–§5; protocol facts from `src-tauri/src/firmware/flasher.rs` (command framing, write stream, retry policy) and §"Device USB operation modes".

## Global Constraints

- Toolbox cargo is OFFLINE — no new Rust crates. Python host tooling must use only stdlib + capstone (already installed).
- All new hardware/host-gated tests are `#[ignore]`d, matching the existing layer-4/5 gate convention (`cargo test --offline --release -- --ignored`).
- RE artifacts (`AF_fw/decrypted/af_190602.dec.bin`, goldens) stay gitignored; tests skip gracefully when absent.
- The real Pico 25 is plugged in at 0416:5020. The uhid double reuses that VID/PID — §5 tests MUST select the double by its unique serial string and MUST be run with the real device unplugged.
- Framebuffer packing is HORIZONTAL MSB-first since 2026-09-13 (`resources/re/af_190602-render.md` §"Packing correction") — golden frames are only meaningful under this decode.
- Protocol framing (from `flasher.rs:136-154`): 18-byte packet `[cmd, 0x0E, arg1 LE32, arg2 LE32, sig 4B, sum LE32 of bytes 0..14]`, sent as 65-byte report with leading report-ID 0. Reads return raw payload (no ID prefix). `REPORT_SIZE` = 64.
- LDROM command set (from `firmware-validation.md`): `0x35` = read dataflash (2048 B: 4-byte checksum + 2044 user; boot flag at raw offset 4+9, PID at 316..320, fw version LE32 at 4+256), `0xC3` = WriteData(arg1=start addr, arg2=len) followed by raw 64-byte data reports, `0xB4` = restart (clears boot flag → APROM).

---

### Task 1: §4 Static hook/cave audit

**Files:**
- Create: `src-tauri/src/bin/anim_patch_dump.rs`
- Create: `scripts/audit-cave.py`

**Interfaces:**
- Consumes: `firmware::anim::effects::{build_gradient_fade_patch, build_center_pulse_patch, build_diagonal_sweep_patch}`, `firmware::patch::{apply_patch, rollback_patch}`, `firmware::loader` (decrypt/load already used by `stock_test`), descriptor `resources/animations/af_190602.json`, image `AF_fw/decrypted/af_190602.dec.bin`
- Produces: `/tmp/audit/stock.bin`, `/tmp/audit/<effect>.bin`, `/tmp/audit/<effect>.mods.json` (`{"hook_site": int, "cave_start": int, "cave_size": int, "cave_body_len": int, "offsets": [int, ...], "config_offset": int}`) consumed by `scripts/audit-cave.py`; `audit-cave.py` exit 0 = PASS.

- [ ] **Step 1: Dumper binary**

`src-tauri/src/bin/anim_patch_dump.rs`: loads descriptor + decrypted image (skip with exit 0 and a stderr note when absent, like the layer-5 gate), builds all three effect patches, writes stock + patched images, and per-effect JSON: hook site, cave start/size, cave body length (max cave offset − cave start + 1), all modification offsets, config byte offset. Also applies then rolls back each patch on a scratch copy and asserts byte-equality with stock (belt-and-braces rollback check in the same run).

- [ ] **Step 2: Run the dumper**

```bash
toolbox run -c arcticfox-build sh -c 'cd src-tauri && cargo run --offline --release --bin anim_patch_dump'
```
Expected: `/tmp/audit/{stock,gradient,center,diagonal}.bin` + 3 JSON files; stderr prints rollback OK per effect.

- [ ] **Step 3: Audit script** (draft via local ollama `qwen3-coder:latest` per AGENTS.md; human review before use)

`scripts/audit-cave.py <effect.bin> <mods.json> <stock.bin>` checks, using capstone Thumb disassembly of the cave body:
1. **Diff confinement:** `set(diff_offsets(effect.bin, stock.bin)) == set(mods.offsets)` — no stray writes.
2. **Hook detour:** 4 bytes at `hook_site` decode as a single `b.w` whose target == `cave_start`.
3. **Mnemonic whitelist:** every instruction in `cave[0:cave_body_len]` is in `{tst.w, beq, bne, bge, b, b.w, ldr, ldrb, str, strb, movs, adds, subs, cmp, lsrs, lsls, ands, eors, nop}` — anything else (esp. `bl`, `svc`, `push/pop`, `bx`) fails. (Whitelist = the exact emitter vocabulary in `effects.rs`; pool data after `cave_body_len` is not disassembled.)
4. **Store discipline:** every `str`/`strb` is either `strb rX, [r1, r0]` (framebuffer byte clear) or `str rX, [r0]` / `str rX, [r0, #0]` (phase global bump). No other store forms.
5. **Branch bounds:** every conditional/unconditional branch target is inside `[cave_start, cave_start+cave_body_len)` or == `hook_resume` (the final `b.w`).
6. **Termination:** the last instruction before the literal pool is `b.w hook_resume`.
7. **Config byte:** `effect.bin[config_offset]` ∈ {2,3,4} and `stock.bin` has 0xFF there (or is shorter).
8. **Erased tail:** bytes between cave body end and cave end (when inside the image) are all 0xFF.

- [ ] **Step 4: Run audit on all three effects**

```bash
for e in gradient center diagonal; do python3 scripts/audit-cave.py /tmp/audit/$e.bin /tmp/audit/$e.mods.json /tmp/audit/stock.bin; done
```
Expected: three `PASS <effect>` lines, exit 0.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/bin/anim_patch_dump.rs scripts/audit-cave.py
git commit -m "test: static hook/cave audit (validation route §4)"
```

---

### Task 2: §3 Differencing — gated-off convergence + self-goldening frames

**Files:**
- Modify: `src-tauri/src/firmware/tests/emu_test.rs` (extend `test_af_190602_animation_frames`, add `test_af_190602_patched_converges_when_gated_off`)

**Interfaces:**
- Consumes: existing `Harness`, `run_at(CLOCK_RENDERER=0x9a10, …)`, `STATUS_WORD=0x20002c34`, `PHASE_GLOBAL=0x20002cf8`, `read_buf` closure pattern from `test_af_190602_animation_frames`
- Produces: golden raw frames at `AF_fw/goldens/<effect>_phase<k>.bin` (gitignored, 1024 B each); convergence test entry point

- [ ] **Step 1: Self-goldening in the animation-frames test**

Inside the existing per-phase loop of `test_af_190602_animation_frames`: after the byte-exact reference assert, write `got` to `AF_fw/goldens/{name}_phase{k}.bin` when absent; when present, assert equality with the file (catching silent behavior drift of emitter/emulator/descriptor across commits). `mkdir -p` the goldens dir; skip cleanly when the decrypted artifact is absent (existing early-return covers this).

- [ ] **Step 2: Failing test — gated-off convergence**

New `#[ignore]`d test `test_af_190602_patched_converges_when_gated_off`: for ONE effect (gradient): apply patch, set config stub to 0 (no effect), drive `run_at(CLOCK_RENDERER, …)` 4 times with timeout bits set exactly like the animation test, and run an UNPATCHED stock harness through the identical drive. Assert after each call: patched framebuffer == stock framebuffer **byte-exact**, and phase global == 0 (cave ran but faded nothing and bumped nothing — wait: the cave bumps the phase global only after the config gate passes; gated off it resumes immediately, so phase stays 0). This is the "only the fade window may differ" property: with the effect deselected the detour must be transparent.

- [ ] **Step 3: Run both gates**

```bash
toolbox run -c arcticfox-build sh -c 'cd src-tauri && cargo test --offline --release --lib firmware::tests::emu_test -- --ignored --nocapture'
```
Expected: `test_af_190602_render_gate`, `test_af_190602_animation_frames` (writes/compares 12 goldens), `test_af_190602_patched_converges_when_gated_off` — all pass. Second run must pass against existing goldens.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/firmware/tests/emu_test.rs
git commit -m "test: gated-off convergence + golden frames (validation route §3)"
```

---

### Task 3: §5 uhid LDROM double + end-to-end flasher gate

**Files:**
- Create: `scripts/uhid_ldrom.py`
- Modify: `src-tauri/src/firmware/tests/flasher_test.rs` (add `test_flash_uhid_double`)
- Host setup: udev rule for `/dev/uhid`

**Interfaces:**
- Consumes: `flasher::flash_firmware_guarded(bytes, expected_pid, on_progress)` (`flasher.rs:507`), `flasher::list_devices()` (`flasher.rs:106`, returns `DeviceInfo{path, serial, product}`)
- Produces: `scripts/uhid_ldrom.py [--nak-delay-ms N] [--die-at-offset N] [--serial S]`; Rust test `test_flash_uhid_double` (`#[ignore]`)

- [ ] **Step 1: Host permission for uhid**

```bash
echo 'KERNEL=="uhid", MODE="0666"' | sudo tee /etc/udev/rules.d/49-uhid.rules
sudo udevadm control --reload && sudo udevadm trigger
ls -l /dev/uhid   # expect crw-rw-rw-
```

- [ ] **Step 2: The double** — `scripts/uhid_ldrom.py` (stdlib only, raw `/dev/uhid` UHID_CREATE2/UHID_INPUT/UHID_OUTPUT events)

Behavior:
- Report descriptor: vendor page 0xFF00, 64-byte IN/OUT reports, report ID 0 (mirrors the Nuvoton updater; the flasher treats every returned byte as payload).
- USB strings: manufacturer `Nuvoton`, product `HID Transfer`, serial from `--serial` (default `A02015081302-uhid`).
- On `0x35` (read dataflash): respond with 2048 bytes — LE checksum of user data, then 2044 user bytes with boot flag `user[9]=1` (LDROM mode), PID `M041` at user offset 312 (raw 316), fw version `110` LE32 at user offset 256.
- On `0xC3 cmd`: record `arg1`/`arg2`, then consume exactly `arg2` payload bytes from subsequent OUT reports (no responses, like real LDROM). `--nak-delay-ms` sleeps between event reads to force host-side write retries (exercises the 50×50 ms backoff in `write_firmware_stream`). `--die-at-offset N` closes `/dev/uhid` after N payload bytes (simulates the Pico Dual mid-flash death; the flasher must retry the whole stream per the 15 s deadline — the double's parent harness then relaunches it).
- On `0xB4` (restart): flip boot flag to 0 and keep serving (APROM-mode stand-in), so `flash_firmware_guarded`'s post-flash poll sees `data[4+9]==0` and succeeds.
- Unknown commands: log and ignore (matches LDROM's silent-drop behavior).

- [ ] **Step 3: Rust test** — `test_flash_uhid_double` in `flasher_test.rs`, `#[ignore]` + skip unless `/dev/uhid` is writable and the REAL device is absent (guard: `flasher::list_devices()` must contain the `-uhid` serial and no other entry — fail loudly otherwise so the test can never touch real hardware):

```rust
#[test]
#[ignore]
fn test_flash_uhid_double() {
    if !std::path::Path::new("/dev/uhid").exists() { eprintln!("no uhid; skipping"); return; }
    let devs = crate::firmware::flasher::list_devices().unwrap();
    assert_eq!(devs.len(), 1, "uhid gate: exactly one device (the double) may be present, found {devs:?}");
    assert!(devs[0].serial.ends_with("-uhid"), "uhid gate: device is not the double: {devs:?}");
    let mut child = std::process::Command::new("python3")
        .args(["../scripts/uhid_ldrom.py", "--nak-delay-ms", "5"])
        .spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(800));
    // 20 KiB of deterministic plaintext (small = fast; size guard needs >= 1024)
    let img: Vec<u8> = (0..20 * 1024u32).map(|i| (i % 251) as u8).collect();
    let res = crate::firmware::flasher::flash_firmware_guarded(&img, None, &|_| {});
    child.kill().ok();
    res.unwrap();
}
```

Note: `flash_firmware_guarded` opens by VID/PID via `open_device` — the double is the only matching device present (gate above). `expected_product_id=None` skips the dataflash PID pre-check.

- [ ] **Step 4: Run the gate (real device UNPLUGGED)**

```bash
toolbox run -c arcticfox-build sh -c 'cd src-tauri && cargo test --offline --release --lib firmware::tests::flasher_test::test_flash_uhid_double -- --ignored --nocapture'
```
Expected: double logs the full 0xC3 stream (20 KiB padded to 20480 = whole 512-rows already), restart, boot-flag-0 poll, test PASS. Run twice: once with `--nak-delay-ms 5` (retry path), once without (fast path).

- [ ] **Step 5: Commit**

```bash
git add scripts/uhid_ldrom.py src-tauri/src/firmware/tests/flasher_test.rs
git commit -m "test: uhid LDROM double end-to-end flasher gate (validation route §5)"
```

---

### Task 4: Doc bookkeeping

**Files:**
- Modify: `docs/firmware-validation.md`, `docs/goals.md`

- [ ] **Step 1:** Mark §3/§4/§5 ✅ with entry points (`scripts/audit-cave.py`, `test_af_190602_patched_converges_when_gated_off`, `test_flash_uhid_double`); refresh the stale §1 "index math (`i & 63`)" remark to the post-packing-fix reality; note §2 remains RE-blocked; note SWD plan `docs/superpowers/plans/2026-09-13-swd-hardware-hacking.md` is deferred (no physical tools yet).
- [ ] **Step 2:** `docs/goals.md` entry linking this plan.
- [ ] **Step 3:** Commit `docs: validation routes §3-§5 implemented, SWD deferred`.

---

## Self-Review notes

- Spec coverage: §3 (Task 2 — convergence + goldens; the clock/battery variant idea needs RAM addresses that aren't RE'd, explicitly deferred), §4 (Task 1), §5 (Task 3), bookkeeping (Task 4). §2 stays blocked, documented.
- Type consistency: `flash_firmware_guarded(&[u8], Option<&str>, &dyn Fn(&str)) -> Result<()>` matches `flasher.rs:507-511`; `list_devices() -> Result<Vec<DeviceInfo>>` with `serial: String` matches `flasher.rs:99-103`; dataflash raw offsets (boot flag 4+9, PID 316..320, fw ver 4+256) match `flasher.rs:410/422/522` and `parse_fw_version`.
- Known soft spots (verified at execution, not hand-waved): the exact uhid event loop is implemented against `linux/uhid.h` semantics in Task 3 Step 2; the report descriptor is validated by the flasher itself connecting — the test IS the check.
