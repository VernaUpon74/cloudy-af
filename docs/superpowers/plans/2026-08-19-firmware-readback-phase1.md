# Firmware Read-back (Phase 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Determine whether the device's LDROM bootloader supports reading APROM firmware back over HID, and if so implement a "Read from Device" flow; otherwise lock in the version-matched-stock fallback.

**Architecture:** Reverse-engineer the HID command sets of both the APROM firmware (af_190602, already decrypted) and the on-device LDROM (dumped via the proven dataflash-trampoline payload) using the slimmed headless Ghidra setup. The gate outcome decides between two small, concrete follow-ups: a streaming `read_flash` flasher command + Firmware Editor button (gate A), or a dataflash firmware-version parser that enables stock matching (gate B).

**Tech Stack:** Rust (Tauri backend, `hidapi`), Ghidra 12.1.2 headless + JDK 21 (`scripts/fetch-ghidra.sh`), cargo tests inside the Flatpak GNOME SDK.

**Spec:** `docs/superpowers/specs/2026-08-19-firmware-animation-pipeline-design.md` (Read-back Design section).

## Global Constraints

- All cargo commands run inside the Flatpak SDK, offline:
  `flatpak run --env=FLATPAK_ENABLE_SDK_EXT=rust-stable --filesystem=home --device=all --command=bash org.gnome.Sdk//49 -c 'export PATH=/usr/lib/sdk/rust-stable/bin:$PATH; cd /var/home/j/cloudy-af/src-tauri && cargo test --offline --lib ...'`
- All Ghidra commands run with:
  `export JAVA_HOME=/var/home/j/cloudy-af/DecryptProject/tools/jdk-21.0.12.1+1` (or `tools/jdk-21` if freshly fetched) and `-scriptPath /var/home/j/cloudy-af/DecryptProject/ghidra-scripts`.
- Hardware tests are `#[ignore]`-gated and only run on the Pico (Product ID `M041`, VID `0x0416` / PID `0x5020`). They must never touch the STM32 device (VID `0x0483` / PID `0x5750`).
- The Pico currently runs stock Joyetech v1.00 (`/var/home/j/pico_v100_plain.bin`, 35,240 bytes). Anything that flashes it must end by restoring this image via the recovery loop (`test_flash_recovery_loop_hardware`).
- LDROM base address on Nuvoton NUC220: `0x00100000`. APROM base: `0x00000000`.
- Do not modify `/etc/udev` rules; access is already granted for both VID/PIDs.
- RE artifacts (JSON notes, dumps) go to `DecryptProject/` (gitignored) EXCEPT the two deliverables explicitly listed as `resources/re/*.json`, which are committed.

---

### Task 1: RE the af_190602 APROM HID dispatcher (hardware-free)

**Files:**
- Create: `DecryptProject/ghidra-scripts/FindHidCommands.java`
- Create: `resources/re/af_190602-hid.json`

**Interfaces:**
- Consumes: `AF_fw/decrypted/af_190602.dec.bin` (already imported in Ghidra project `AFSamples`), existing `HeadlessAFPre.java` / `HeadlessAFCount.java`.
- Produces: `resources/re/af_190602-hid.json` with keys `{ "dispatcher": "0x…", "tx_report": "0x…", "rx_report": "0x…", "commands": { "0x35": "0x…", … }, "notes": "…" }` — consumed by Task 5A (instrumented verify-read design) and by Phase 3 (renderer RE uses the same TX path).

- [ ] **Step 1: Write the Ghidra analysis script**

Create `DecryptProject/ghidra-scripts/FindHidCommands.java`:

```java
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.symbol.Reference;

// Finds the HID command dispatcher in a decrypted ArcticFox APROM image.
// Strategy: locate the ASCII "HIDC" signature (used in command packets),
// walk xrefs to find the parser, then list every CMP-immediate in that
// function and its callees — those immediates are the command bytes.
public class FindHidCommands extends GhidraScript {
    @Override
    public void run() throws Exception {
        var mem = currentProgram.getMemory();
        var space = currentProgram.getAddressFactory().getDefaultAddressSpace();
        byte[] sig = { 'H', 'I', 'D', 'C' };

        Address hit = mem.findBytes(space.getMinAddress(), sig, null, true, monitor);
        while (hit != null) {
            println("HIDC at " + hit);
            for (Reference ref : getReferencesTo(hit)) {
                Function f = currentProgram.getFunctionManager()
                    .getFunctionContaining(ref.getFromAddress());
                println("  xref from " + ref.getFromAddress()
                    + " in " + (f == null ? "<no func>" : f.getName() + " @ " + f.getEntryPoint()));
                if (f != null) {
                    dumpCompares(f);
                    for (Function callee : f.getCalledFunctions(monitor)) {
                        println("  callee " + callee.getName() + " @ " + callee.getEntryPoint());
                        dumpCompares(callee);
                    }
                }
            }
            hit = mem.findBytes(hit.add(1), sig, null, true, monitor);
        }
    }

    private void dumpCompares(Function f) {
        var listing = currentProgram.getListing();
        var body = f.getBody();
        for (Instruction ins : listing.getInstructions(body, true)) {
            String mn = ins.getMnemonicString();
            if (mn.equalsIgnoreCase("cmp") || mn.equalsIgnoreCase("mov")) {
                for (int i = 0; i < ins.getNumOperands(); i++) {
                    for (Object o : ins.getOpObjects(i)) {
                        if (o instanceof ghidra.program.model.scalar.Scalar s) {
                            long v = s.getUnsignedValue();
                            if (v >= 0x20 && v <= 0xFF) {
                                println("    " + ins.getAddress() + ": " + ins
                                    + "  ; imm=0x" + Long.toHexString(v));
                            }
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 2: Run it against the existing AFSamples project**

```bash
export JAVA_HOME=/var/home/j/cloudy-af/DecryptProject/tools/jdk-21.0.12.1+1
cd /var/home/j/cloudy-af/DecryptProject/ghidra_12.1.2_PUBLIC
./support/analyzeHeadless /var/home/j/cloudy-af/DecryptProject/ghidra-projects AFSamples \
  -process af_190602.dec.bin \
  -scriptPath /var/home/j/cloudy-af/DecryptProject/ghidra-scripts \
  -postScript FindHidCommands.java 2>&1 | grep -E "HIDC|xref|callee|imm=|ERROR"
```

Expected: at least one `HIDC` hit with an xref into a function whose CMP
immediates include `0x35`, `0x53`, `0xc3`, `0xb4` (the known commands from
`flasher.rs`: ReadDataflash, WriteDataflash, WriteData, Restart).

- [ ] **Step 3: Identify TX/RX report functions**

Re-run with the GUI-free decompiler to name the report send/receive helpers:

```bash
./support/analyzeHeadless /var/home/j/cloudy-af/DecryptProject/ghidra-projects AFSamples \
  -process af_190602.dec.bin -scriptPath /var/home/j/cloudy-af/DecryptProject/ghidra-scripts \
  -postScript FindHidCommands.java 2>&1 | tee /tmp/hid_af190602.txt | tail -40
```

From the output, record the dispatcher function address, the addresses of the
callees that copy data into/out of the 64-byte HID report buffer (look for
constants `0x40`/`64` and the report buffer pointer), and every command byte
handled, including any NOT in the known set (`0x35/0x53/0xc3/0xb4/0xc1/0x64`).

- [ ] **Step 4: Write the deliverable JSON**

Write `resources/re/af_190602-hid.json` with the discovered addresses:

```json
{
  "build": "af_190602",
  "dispatcher": "0x<addr>",
  "tx_report": "0x<addr>",
  "rx_report": "0x<addr>",
  "commands": { "0x35": "0x<handler>", "0x53": "0x<handler>", "0xc3": "0x<handler>", "0xb4": "0x<handler>" },
  "extra_commands": [ "0x.." ],
  "notes": "<one paragraph: how dispatch works, where the report buffer lives>"
}
```

- [ ] **Step 5: Commit**

`DecryptProject/` is gitignored, so only the JSON deliverable is committed;
the Ghidra script stays in the local scratch dir alongside the other scripts.

```bash
git add resources/re/af_190602-hid.json
git commit -m "re(firmware): map af_190602 HID command dispatcher"
```

---

### Task 2: Disassemble and verify the LDROM dump payload

**Files:**
- Create: `DecryptProject/ghidra-scripts/DumpDisassembly.java`
- Create: `DecryptProject/ldrom/payload-notes.md`

**Interfaces:**
- Consumes: `/var/home/j/ldrom_dump_payload.bin` (228 bytes, APROM Thumb code).
- Produces: human-verified statement of the payload's protocol (chunk index
  location, copy source/dest, boot-flag write, reset mechanism), consumed by
  Task 3. If the payload is broken, this task produces a fixed
  `DecryptProject/ldrom/ldrom_dump_payload_fixed.bin` and Task 3 uses it.

- [ ] **Step 1: Write a disassembly-dump script**

Create `DecryptProject/ghidra-scripts/DumpDisassembly.java`:

```java
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Instruction;

// Prints full disassembly of a tiny Thumb payload to the headless log.
public class DumpDisassembly extends GhidraScript {
    @Override
    public void run() throws Exception {
        var listing = currentProgram.getListing();
        for (Instruction ins : listing.getInstructions(true)) {
            println(ins.getAddress() + "  " + ins);
        }
        println("RESULT instructions=" + listing.getNumInstructions());
    }
}
```

- [ ] **Step 2: Import the payload and disassemble**

```bash
mkdir -p /var/home/j/cloudy-af/DecryptProject/ldrom /tmp/ghidra-payload
export JAVA_HOME=/var/home/j/cloudy-af/DecryptProject/tools/jdk-21.0.12.1+1
cd /var/home/j/cloudy-af/DecryptProject/ghidra_12.1.2_PUBLIC
./support/analyzeHeadless /tmp/ghidra-payload Payload \
  -import /var/home/j/ldrom_dump_payload.bin \
  -processor ARM:LE:32:Cortex -loader-baseAddr 0x0 \
  -scriptPath /var/home/j/cloudy-af/DecryptProject/ghidra-scripts \
  -preScript HeadlessAFPre.java -postScript DumpDisassembly.java 2>&1 | tee /tmp/payload_dis.txt | tail -60
```

- [ ] **Step 3: Verify the payload protocol against the test's expectations**

Check `/tmp/payload_dis.txt` for this exact behavior (from
`test_ldrom_dump_hardware`): chunk index read from dataflash `data[8]`, copy
of a 2028-byte LDROM chunk (`chunk_index * 2028 + 0x00100000` region) into
dataflash `data[16..2044]`, then `data[9] = 1` (boot LDROM flag), then
software reset. Record findings in `DecryptProject/ldrom/payload-notes.md`.
If any step is missing/wrong, fix the payload bytes (small Thumb edits,
document each byte change in the notes) and save as
`DecryptProject/ldrom/ldrom_dump_payload_fixed.bin`.

- [ ] **Step 4: No commit (gitignored artifacts)** — notes stay in `DecryptProject/`.

---

### Task 3: Dump the Pico LDROM (hardware)

**Files:**
- Modify: `src-tauri/src/firmware/tests/flasher_test.rs` (test_ldrom_dump_hardware)
- Create: `DecryptProject/ldrom/ldrom_m041.bin` (output, gitignored)

**Interfaces:**
- Consumes: payload from Task 2 (`/var/home/j/ldrom_dump_payload.bin` or the
  fixed one), the working Pico (M041) on stock v1.00.
- Produces: a verified 16 KB LDROM image consumed by Task 4.

- [ ] **Step 1: Point the dump test at the right paths**

In `test_ldrom_dump_hardware`, change:

```rust
let payload = std::fs::read("/var/home/j/ldrom_dump_payload.bin").expect("read payload");
```

to read `DecryptProject/ldrom/ldrom_dump_payload_fixed.bin` if it exists,
else `/var/home/j/ldrom_dump_payload.bin`:

```rust
let payload = std::fs::read("/var/home/j/cloudy-af/DecryptProject/ldrom/ldrom_dump_payload_fixed.bin")
    .or_else(|_| std::fs::read("/var/home/j/ldrom_dump_payload.bin"))
    .expect("read payload");
```

and change the output write from `/var/home/j/ldrom_dump.bin` to
`/var/home/j/cloudy-af/DecryptProject/ldrom/ldrom_m041.bin` (both the write
and the later verification read in the same test).

- [ ] **Step 2: Run the dump on hardware**

```bash
flatpak run --env=FLATPAK_ENABLE_SDK_EXT=rust-stable --filesystem=home --device=all \
  --command=bash org.gnome.Sdk//49 -c 'export PATH=/usr/lib/sdk/rust-stable/bin:$PATH; \
  cd /var/home/j/cloudy-af/src-tauri && \
  cargo test --offline --lib firmware::tests::flasher_test::test_ldrom_dump_hardware -- --ignored --nocapture'
```

Expected: `done: 8 / 8 chunks confirmed`, integrity checks pass, 16,384-byte
`DecryptProject/ldrom/ldrom_m041.bin`. If the dump stalls, iterate on the
payload per Task 2 notes (chunks are confirmed independently — partial dumps
are fine to debug from).

- [ ] **Step 3: Restore the Pico to stock v1.00**

The dump payload wiped APROM. Run:

```bash
flatpak run --env=FLATPAK_ENABLE_SDK_EXT=rust-stable --filesystem=home --device=all \
  --command=bash org.gnome.Sdk//49 -c 'export PATH=/usr/lib/sdk/rust-stable/bin:$PATH; \
  cd /var/home/j/cloudy-af/src-tauri && \
  cargo test --offline --lib firmware::tests::flasher_test::test_flash_recovery_loop_hardware -- --ignored --nocapture'
```

(Replug the Pico when the test prints "waiting for Pico".) Then verify with
`test_read_dataflash_hardware` → Product ID `M041`, checksum ok.

- [ ] **Step 4: Commit the test change**

```bash
git add src-tauri/src/firmware/tests/flasher_test.rs
git commit -m "test(firmware): LDROM dump reads fixed payload fallback, writes to DecryptProject"
```

---

### Task 4: RE the LDROM command set — the feasibility gate

**Files:**
- Create: `resources/re/ldrom-commands.json`

**Interfaces:**
- Consumes: `DecryptProject/ldrom/ldrom_m041.bin`, `FindHidCommands.java` (Task 1).
- Produces: `resources/re/ldrom-commands.json`:
  `{ "commands": { "0x35": "0x…", … }, "read_command": "0x…" | null, "gate": "A" | "B", "notes": "…" }`.
  The `gate` value selects Task 5A or 5B.

- [ ] **Step 1: Import the LDROM dump at its real base address**

```bash
mkdir -p /tmp/ghidra-ldrom
export JAVA_HOME=/var/home/j/cloudy-af/DecryptProject/tools/jdk-21.0.12.1+1
cd /var/home/j/cloudy-af/DecryptProject/ghidra_12.1.2_PUBLIC
./support/analyzeHeadless /tmp/ghidra-ldrom LDROM \
  -import /var/home/j/cloudy-af/DecryptProject/ldrom/ldrom_m041.bin \
  -processor ARM:LE:32:Cortex -loader-baseAddr 0x00100000 \
  -scriptPath /var/home/j/cloudy-af/DecryptProject/ghidra-scripts \
  -preScript HeadlessAFPre.java -postScript HeadlessAFCount.java 2>&1 | grep -E "RESULT|entry points|ERROR"
```

Expected: sane function/instruction counts (the LDROM is ~16 KB; expect
dozens of functions). If `HeadlessAFPre` finds no entry points, the dump is
bad — return to Task 3.

- [ ] **Step 2: Enumerate its HID commands**

```bash
./support/analyzeHeadless /tmp/ghidra-ldrom LDROM \
  -process ldrom_m041.bin \
  -scriptPath /var/home/j/cloudy-af/DecryptProject/ghidra-scripts \
  -postScript FindHidCommands.java 2>&1 | tee /tmp/hid_ldrom.txt | grep -E "HIDC|imm="
```

- [ ] **Step 3: Decide the gate**

In `/tmp/hid_ldrom.txt`, look for CMP immediates beyond the known set
(`0x35/0x53/0xc3/0xb4`). A **read command** is one whose handler writes
APROM-region data (`0x00000000`–`0x0001FFFF`) into the HID TX buffer —
confirm by following the handler in the disassembly. Write
`resources/re/ldrom-commands.json`:

- gate A (read command found): `"read_command": "0xNN"`, plus the command's
  argument layout (offset/length semantics) in `notes`.
- gate B (no read command): `"read_command": null`.

- [ ] **Step 4: Commit**

```bash
git add resources/re/ldrom-commands.json
git commit -m "re(firmware): enumerate LDROM HID command set, gate <A|B>"
```

---

### Task 5A (gate A only): Implement device read-back

**Files:**
- Modify: `src-tauri/src/firmware/flasher.rs`
- Modify: `src-tauri/src/commands/firmware.rs`
- Modify: `src/renderer-firmware.js`, `firmware.html`, `src/lib/tauri-bridge.js`
- Test: `src-tauri/src/firmware/tests/flasher_test.rs`

**Interfaces:**
- Consumes: `resources/re/ldrom-commands.json` → `read_command` byte and arg layout.
- Produces: `pub fn read_flash(device: &mut hidapi::HidDevice, offset: u32, len: u32) -> Result<Vec<u8>>` in `flasher.rs`; Tauri command `read_firmware_from_device() -> Result<Vec<u8>, String>`; JS wrapper `readFirmwareFromDevice()`.

- [ ] **Step 1: Write the failing hardware test**

Append to `flasher_test.rs`:

```rust
#[test]
#[ignore]
fn test_read_flash_hardware() {
    //! Gate A: LDROM read-back. Device must be in LDROM mode holding stock
    //! v1.00 in APROM; the read must byte-match pico_v100_plain.bin.
    use crate::firmware::flasher as f;
    f::ensure_ldrom_mode().expect("ensure_ldrom_mode");
    let mut dev = f::open_device().expect("open failed");
    let expect = std::fs::read("/var/home/j/pico_v100_plain.bin").unwrap();
    let got = f::read_flash(&mut dev, 0, expect.len() as u32).expect("read_flash failed");
    assert_eq!(got.len(), expect.len());
    assert_eq!(got, expect, "read-back differs from on-disk stock image");
}
```

Run it: expected FAIL (`read_flash` does not exist → compile error).

- [ ] **Step 2: Implement `read_flash` in `flasher.rs`**

Following the command layout from `ldrom-commands.json`. Template (adjust
arg order/size to the discovered layout):

```rust
/// Read APROM flash via the LDROM read command discovered by RE (gate A).
pub fn read_flash(device: &mut hidapi::HidDevice, offset: u32, len: u32) -> Result<Vec<u8>> {
    send_command(device, READ_CMD, offset as i32, len as i32)?; // READ_CMD from ldrom-commands.json
    read_exact(device, len as usize)
}
```

Add `const READ_CMD: u8 = 0xNN; // from resources/re/ldrom-commands.json`.

- [ ] **Step 3: Run the hardware test**

Expected: PASS — byte-exact match with `pico_v100_plain.bin`.

- [ ] **Step 4: Wire the Tauri command and UI button**

In `commands/firmware.rs`:

```rust
#[tauri::command]
pub async fn read_firmware_from_device() -> Result<Vec<u8>, String> {
    crate::firmware::flasher::ensure_ldrom_mode().map_err(|e| e.to_string())?;
    let mut dev = crate::firmware::flasher::open_device().map_err(|e| e.to_string())?;
    crate::firmware::flasher::read_flash(&mut dev, 0, 128 * 1024)
        .map_err(|e| e.to_string())
}
```

Register in `lib.rs` `generate_handler!`. Add to `tauri-bridge.js`:

```javascript
export async function readFirmwareFromDevice() { return invoke('read_firmware_from_device'); }
```

Add a **Read from Device** button to `firmware.html` toolbar; in
`renderer-firmware.js` call it, then feed the bytes through the existing
open-firmware path (decrypt → detect definition).

- [ ] **Step 5: Verify + commit**

`cargo test --offline --lib` (unit suite green), hardware test passed in
Step 3. Commit:

```bash
git add src-tauri/src/firmware/flasher.rs src-tauri/src/commands/firmware.rs \
  src-tauri/src/lib.rs src/renderer-firmware.js firmware.html src/lib/tauri-bridge.js \
  src-tauri/src/firmware/tests/flasher_test.rs
git commit -m "feat(firmware): read device APROM over HID (LDROM read command)"
```

---

### Task 5B (gate B only): Version-match fallback helper

**Files:**
- Modify: `src-tauri/src/firmware/flasher.rs`
- Test: `src-tauri/src/firmware/tests/flasher_test.rs`

**Interfaces:**
- Consumes: nothing (gate B means no read command exists).
- Produces: `pub fn read_fw_version() -> Result<i32>` in `flasher.rs` —
  consumed by Phase 2 (stock library matching).

- [ ] **Step 1: Write the failing unit test (no hardware)**

The dataflash layout (from `test_check_state_hardware`): firmware version is
a little-endian i32 at dataflash data offset 256 (raw offset 260 including
the 4-byte checksum prefix). Add to `flasher_test.rs`:

```rust
#[test]
fn test_parse_fw_version() {
    let mut df = vec![0u8; 2048];
    df[4 + 256..4 + 260].copy_from_slice(&210103i32.to_le_bytes());
    assert_eq!(f::parse_fw_version(&df).unwrap(), 210103);
}
```

Run: FAIL (`parse_fw_version` undefined).

- [ ] **Step 2: Implement**

In `flasher.rs`:

```rust
/// Firmware version from a raw dataflash buffer (data offset 256).
pub fn parse_fw_version(dataflash: &[u8]) -> Result<i32> {
    if dataflash.len() < 4 + 260 {
        return Err(FirmwareError::Other("dataflash too short".into()));
    }
    Ok(i32::from_le_bytes(dataflash[4 + 256..4 + 260].try_into().unwrap()))
}

/// Read the device firmware version over HID.
pub fn read_fw_version() -> Result<i32> {
    parse_fw_version(&read_dataflash()?)
}
```

- [ ] **Step 3: Run tests**

`cargo test --offline --lib firmware::tests::flasher_test::test_parse_fw_version`
Expected: PASS.

- [ ] **Step 4: Hardware sanity (Pico)**

Run `test_check_state_hardware` (existing, `#[ignore]`) and confirm the
printed fw version matches stock v1.00's version word.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/firmware/flasher.rs src-tauri/src/firmware/tests/flasher_test.rs
git commit -m "feat(firmware): parse firmware version from dataflash (stock matching)"
```

---

### Task 6: Record the gate outcome in the spec

**Files:**
- Modify: `docs/superpowers/specs/2026-08-19-firmware-animation-pipeline-design.md`

- [ ] **Step 1: Update the Decisions Log**

Replace the read-back bullet with the measured outcome, e.g.:

```markdown
- **Read-back approach (resolved 2026-08-2x):** LDROM command set dumped and
  enumerated (`resources/re/ldrom-commands.json`). Gate <A|B>: <one sentence —
  either "LDROM implements read command 0xNN; device read-back implemented"
  or "no LDROM read command exists; version-matched stock is the only
  acquisition path, instrumented verify-read retained for post-flash checks">.
```

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/specs/2026-08-19-firmware-animation-pipeline-design.md
git commit -m "docs: record LDROM read-back gate outcome"
```

---

## Self-Review

**Spec coverage:** Read-back Design (Phase 1a gate, outcomes A/B) → Tasks
1–6. Phase 2+ items (stock library, effects, UI dropdowns, STM32) are
deliberately out of scope — they get their own plans after this gate.

**Placeholder scan:** Addresses in JSON deliverables are discovered values by
design; every command/step is otherwise concrete, including exact scripts and
shell invocations. No TBDs.

**Type consistency:** `read_flash(device, offset: u32, len: u32) -> Result<Vec<u8>>`
(Task 5A) matches its test call; `parse_fw_version(&[u8]) -> Result<i32>` and
`read_fw_version() -> Result<i32>` (Task 5B) match their test; Ghidra script
names referenced in commands match the files created.
