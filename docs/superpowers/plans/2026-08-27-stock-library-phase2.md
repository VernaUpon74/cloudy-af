# Stock Library & Pipeline (Phase 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bundle the four verified stock builds + `devices.json`, add a "Download Stock" flow that matches the connected device to a bundled build, and prove the patch → re-encrypt → flash → undo cycle end-to-end on hardware.

**Architecture:** The patch/encrypt/flash/undo pipeline already exists (`commands/firmware.rs`, `flasher::flash_firmware_guarded`, backup dir). What is missing is the acquisition half of gate B: a bundled stock library (`resources/firmware/`), a product-ID/version matcher (`firmware/stock.rs`), and a `download_stock` Tauri command + toolbar button that loads the matched build into the existing editor state. Exact version matching is best-effort: an unmatched device warns and falls back to explicit build selection (an unmatched build is never flashed silently).

**Tech Stack:** Rust (Tauri backend, `hidapi`), serde_json, cargo tests inside the Flatpak GNOME SDK.

**Spec:** `docs/superpowers/specs/2026-08-19-firmware-animation-pipeline-design.md` (Workflow §1 Download Stock, Decisions Log "Bundled stock set", Implementation Phasing Phase 2).

## Global Constraints

- All cargo commands run inside the Flatpak SDK, offline:
  `flatpak run --env=FLATPAK_ENABLE_SDK_EXT=rust-stable --filesystem=home --device=all --command=bash org.gnome.Sdk//49 -c 'export PATH=/usr/lib/sdk/rust-stable/bin:$PATH; cd /var/home/j/cloudy-af/src-tauri && cargo test --offline --lib ...'`
- Hardware tests are `#[ignore]`-gated and only run on the Pico (Product ID `M041`, VID `0x0416` / PID `0x5020`). Never touch the STM32 device (VID `0x0483` / PID `0x5750`).
- **The Pico now runs ArcticFox af_190602** (upgraded 2026-08-27). Stock Joyetech v1.00 is the rescue contingency, NOT the resting state. Anything that flashes must end by restoring `AF_fw/decrypted/af_190602.dec.bin` unless the test's whole point is stock.
- Rescue kit: `DecryptProject/rescue/` (stock image, dataflash backup, README with the battery-out + Plus recovery procedure). `DecryptProject/` is gitignored.
- `flasher::screenshot` (0xC1) works on ArcticFox only; stock firmware drops the command. Framebuffer reads all-zero while the display is asleep.
- Bundled firmware binaries are committed to `resources/firmware/` (they are app resources, not RE artifacts). `AF_fw/` stays gitignored.
- STM32 builds ship bundled but STM32 flash/read-back UI stays disabled (Phase 5).

---

### Task 1: Bundle the stock library + devices.json

**Files:**
- Create: `resources/firmware/af_170222.bin`, `resources/firmware/af_180913.bin`, `resources/firmware/af_190602.bin`, `resources/firmware/af_211009.bin` (copied from `AF_fw/nfeteam/`)
- Create: `resources/firmware/devices.json`
- Modify: `src-tauri/tauri.conf.json` (bundle resources)
- Test: `src-tauri/src/firmware/tests/stock_test.rs` (new), `src-tauri/src/firmware/tests/mod.rs` (register)

**Interfaces:**
- Consumes: existing `encryption::decrypt` cascade, `loader::load_firmware`.
- Produces: `resources/firmware/devices.json` schema consumed by Task 2:
  ```json
  {
    "lines": {
      "nuvoton": { "product_ids": ["M011","M037","M038","M041","M045","M046","M064","M065","M070","M073","M074","M077","M091","M095","M105","M111","M114","M972","M973"], "usb": { "vid": "0x0416", "pid": "0x5020" } },
      "stm32": { "product_ids": [], "usb": { "vid": "0x0483", "pid": "0x5750" } }
    },
    "builds": [
      { "id": "af_170222", "file": "af_170222.bin", "line": "nuvoton", "fw_versions": [] },
      { "id": "af_180913", "file": "af_180913.bin", "line": "nuvoton", "fw_versions": [] },
      { "id": "af_190602", "file": "af_190602.bin", "line": "nuvoton", "fw_versions": [] },
      { "id": "af_211009", "file": "af_211009.bin", "line": "stm32",    "fw_versions": [] }
    ]
  }
  ```
  (`fw_versions` starts empty; Task 5's hardware discovery fills in the
  dataflash version word each build reports — stock v1.00 reports `100`,
  af_190602 reports `110`.)

- [ ] **Step 1: Copy the builds and write devices.json**

```bash
cd /var/home/j/cloudy-af
mkdir -p resources/firmware
cp AF_fw/nfeteam/stable/af_170222.bin AF_fw/nfeteam/stable/af_180913.bin \
   AF_fw/nfeteam/stable/af_190602.bin AF_fw/nfeteam/stm32/af_211009.bin \
   resources/firmware/
```

Write `resources/firmware/devices.json` exactly as the schema above.

- [ ] **Step 2: Register the bundle in tauri.conf.json**

In `src-tauri/tauri.conf.json`, add to `bundle.resources` (next to the
existing `definitions`/`patches` entries):

```json
"../resources/firmware": "firmware"
```

- [ ] **Step 3: Write the failing test**

Create `src-tauri/src/firmware/tests/stock_test.rs`:

```rust
//! Bundled stock library integrity: the four builds ship in
//! resources/firmware and pass the decrypt cascade.
use crate::firmware::loader::load_firmware;
use crate::firmware::definition::load_definitions_from;
use std::path::Path;

fn resources() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("resources")
}

#[test]
fn test_bundled_builds_present_and_decryptable() {
    let dir = resources().join("firmware");
    let defs = load_definitions_from(&resources().join("definitions"));
    for build in ["af_170222.bin", "af_180913.bin", "af_190602.bin", "af_211009.bin"] {
        let path = dir.join(build);
        let img = load_firmware(&path, &defs)
            .unwrap_or_else(|e| panic!("{build}: {e}"));
        assert!(img.bytes.len() > 30_000, "{build} too small after decrypt");
    }
}

#[test]
fn test_devices_json_parses() {
    let raw = std::fs::read_to_string(resources().join("firmware/devices.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["builds"].as_array().unwrap().len(), 4);
    assert!(v["lines"]["nuvoton"]["product_ids"].as_array().unwrap()
        .iter().any(|p| p == "M041"));
}
```

Check how definitions are loaded for tests — if `load_definitions_from` does
not exist, use whatever `loader_test.rs` already does to get definitions for
`load_firmware` (read `src-tauri/src/firmware/tests/loader_test.rs` first and
mirror its setup exactly). Register the module in
`src-tauri/src/firmware/tests/mod.rs` (`mod stock_test;`).

- [ ] **Step 4: Run tests**

Run: `cargo test --offline --lib firmware::tests::stock_test`
Expected: PASS (the resources exist by then; if Step 3 was done before
Step 1, observe the FAIL first).

- [ ] **Step 5: Commit**

```bash
git add resources/firmware src-tauri/tauri.conf.json \
  src-tauri/src/firmware/tests/stock_test.rs src-tauri/src/firmware/tests/mod.rs
git commit -m "feat(firmware): bundle stock build library + devices.json"
```

---

### Task 2: Stock matching module

**Files:**
- Create: `src-tauri/src/firmware/stock.rs`
- Modify: `src-tauri/src/firmware/mod.rs` (`pub mod stock;`)
- Test: extend `src-tauri/src/firmware/tests/stock_test.rs`

**Interfaces:**
- Consumes: `resources/firmware/devices.json` (Task 1), `flasher::parse_fw_version` (`pub fn parse_fw_version(dataflash: &[u8]) -> Result<i32>`), product id at dataflash raw offset `316..320`.
- Produces:
  ```rust
  pub struct StockLibrary { pub lines: Vec<Line>, pub builds: Vec<StockBuild> }
  pub struct Line { pub name: String, pub product_ids: Vec<String> }
  pub struct StockBuild { pub id: String, pub file: String, pub line: String, pub fw_versions: Vec<i32> }
  pub enum StockMatch { Exact(&'static? no —) }
  ```
  Concretely:
  ```rust
  pub fn load_library(json: &str) -> Result<StockLibrary>;
  pub fn line_for_product<'a>(lib: &'a StockLibrary, product_id: &str) -> Option<&'a Line>;
  pub enum MatchKind { ExactVersion, LineOnly, NoLine }
  pub fn match_build(lib: &StockLibrary, product_id: &str, fw_version: i32) -> (MatchKind, Option<&StockBuild>);
  ```
  `match_build` rules: unknown product ID → `(NoLine, None)`. Known line:
  prefer a build in that line whose `fw_versions` contains `fw_version`
  (→ `ExactVersion`); otherwise the newest build in the line
  (→ `LineOnly`, caller must warn).

- [ ] **Step 1: Write the failing tests**

Append to `stock_test.rs`:

```rust
use crate::firmware::stock::{load_library, line_for_product, match_build, MatchKind};

fn lib() -> crate::firmware::stock::StockLibrary {
    let raw = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
            .join("resources/firmware/devices.json")).unwrap();
    load_library(&raw).unwrap()
}

#[test]
fn test_line_lookup() {
    let lib = lib();
    assert_eq!(line_for_product(&lib, "M041").unwrap().name, "nuvoton");
    assert!(line_for_product(&lib, "X999").is_none());
}

#[test]
fn test_match_build_rules() {
    let lib = lib();
    // No fw_versions recorded yet -> LineOnly, newest nuvoton build.
    let (kind, build) = match_build(&lib, "M041", 110);
    assert!(matches!(kind, MatchKind::LineOnly));
    assert_eq!(build.unwrap().id, "af_190602");
    // Unknown product -> NoLine.
    let (kind, build) = match_build(&lib, "X999", 110);
    assert!(matches!(kind, MatchKind::NoLine));
    assert!(build.is_none());
}
```

Run: `cargo test --offline --lib firmware::tests::stock_test` — expected FAIL (module does not exist).

- [ ] **Step 2: Implement `stock.rs`**

serde derive structs matching the devices.json schema (`lines` is a map
name→line; preserve insertion order via `serde_json::Map` or collect into
Vec with name from the key). `match_build` "newest" = max by build id
string (the `af_YYMMDD` naming sorts chronologically).

- [ ] **Step 3: Run tests**

Run: `cargo test --offline --lib firmware::tests::stock_test`
Expected: PASS (all 4 tests).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/firmware/stock.rs src-tauri/src/firmware/mod.rs src-tauri/src/firmware/tests/stock_test.rs
git commit -m "feat(firmware): stock library matching (product id + version word)"
```

---

### Task 3: `download_stock` / `open_stock_build` Tauri commands

**Files:**
- Modify: `src-tauri/src/commands/firmware.rs`
- Modify: `src-tauri/src/lib.rs` (`generate_handler!` registration)

**Interfaces:**
- Consumes: Task 1 resources, Task 2 matcher, existing `open_firmware` internals, `flasher::read_dataflash`, `flasher::parse_fw_version`.
- Produces (registered commands):
  - `download_stock() -> Result<Value, String>` — reads device dataflash (product id at raw offset `316..320`, version via `parse_fw_version`), runs `match_build`, loads the chosen bundled build into `FirmwareState` exactly like `open_firmware` (same `OpenFirmware` insert, backup, patch list), returns `{ handle, name, encryption, build_id, match_kind, product_id, fw_version }`. `NoLine` → `Err` listing known lines (UI then calls `open_stock_build`).
  - `open_stock_build(build_id: String) -> Result<Value, String>` — explicit pick; same load path, `match_kind: "manual"`.
  - Both return an error if the resource dir or build file is missing.

- [ ] **Step 1: Refactor the open core**

Extract the body of `open_firmware` after `load_firmware` into
`fn insert_opened(app: &AppHandle, state: &FirmwareState, image: FirmwareImage) -> Value`
returning the info JSON (extended with the extra fields; `open_firmware`
fills `build_id: null, match_kind: "file"`). Keep `open_firmware`'s public
behavior unchanged.

- [ ] **Step 2: Resource dir helper**

Add `fn firmware_resource_dir(app: &AppHandle) -> Option<PathBuf>` mirroring
the candidate-list pattern of `load_definitions`, but for the `firmware`
subdir (resource_dir/`firmware`, exe-relative fallbacks, and the
`CARGO_MANIFEST_DIR/../resources/firmware` dev path).

- [ ] **Step 3: Implement the commands**

`download_stock` calls `flasher::read_dataflash()` (HID, may fail → propagate
as Err string), slices `df[316..320]` trimmed of NUL/space for the product
id, `parse_fw_version(&df)`, `match_build`, then `load_firmware` on the
matched file and `insert_opened`. `open_stock_build` looks up the build by
id in the library and loads it the same way. Register both in
`lib.rs` `generate_handler!`.

- [ ] **Step 4: Verify compile + unit suite**

Run: `cargo test --offline --lib`
Expected: all previous tests PASS (compile is the gate here; the HID path is
hardware-gated).

- [ ] **Step 5: Hardware smoke test**

With the Pico (running af_190602, reports version word `110`, product
`M041`) plugged in, add a temporary `#[ignore]` test in `flasher_test.rs`:

```rust
#[test]
#[ignore]
fn test_download_stock_hardware() {
    let df = crate::firmware::flasher::read_dataflash().unwrap();
    let pid = String::from_utf8_lossy(&df[316..320]).trim_matches(char::from(0)).trim().to_string();
    let ver = crate::firmware::flasher::parse_fw_version(&df).unwrap();
    println!("pid={pid} ver={ver}");
    assert_eq!(pid, "M041");
}
```

Run it (`-- --ignored --nocapture`), confirm `pid=M041 ver=110`. (The full
command path is exercised from the UI in Task 4 / Task 5.)

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/commands/firmware.rs src-tauri/src/lib.rs src-tauri/src/firmware/tests/flasher_test.rs
git commit -m "feat(firmware): download_stock and open_stock_build commands"
```

---

### Task 4: "Download Stock" UI

**Files:**
- Modify: `firmware.html` (toolbar)
- Modify: `src/lib/tauri-bridge.js`
- Modify: `src/renderer-firmware.js`

**Interfaces:**
- Consumes: Task 3 commands; existing `open-firmware` button flow in `renderer-firmware.js` (read it first and mirror its result handling — handle, patch list population, enabling Flash/Undo buttons).
- Produces: `downloadStock()` and `openStockBuild(buildId)` JS wrappers; a **Download Stock** button between **Open Firmware** and **Save**.

- [ ] **Step 1: Bridge wrappers**

In `src/lib/tauri-bridge.js`:

```javascript
export async function downloadStock() { return invoke('download_stock'); }
export async function openStockBuild(buildId) { return invoke('open_stock_build', { buildId }); }
```

- [ ] **Step 2: Button + handler**

In `firmware.html` toolbar:
`<button id="download-stock" class="btn btn-default">Download Stock</button>`

In `renderer-firmware.js`, wire `download-stock` to call `downloadStock()`,
then feed the result through the same post-open path as `open-firmware`.
On `match_kind === "line_only"` show a warning banner/dialog: "Device
firmware version not in the bundled library — the current image cannot be
preserved; Undo will restore this stock build, not your current firmware."
On Err containing the line list, prompt the user to pick a build and call
`openStockBuild`.

- [ ] **Step 3: Manual smoke test**

Run the app (`flatpak` dev flow already used for this project), open the
Firmware Editor, click Download Stock with the Pico connected: editor shows
`af_190602` / ArcticFox definition, Flash and Undo buttons enable.

- [ ] **Step 4: Commit**

```bash
git add firmware.html src/lib/tauri-bridge.js src/renderer-firmware.js
git commit -m "feat(firmware): Download Stock button in firmware editor"
```

---

### Task 5: Hardware end-to-end + AF version discovery

**Files:**
- Modify: `src-tauri/src/firmware/tests/flasher_test.rs`
- Modify: `resources/firmware/devices.json` (fill `fw_versions`)
- Modify: `docs/goals.md`

**Interfaces:**
- Consumes: everything above; the Pico on af_190602.
- Produces: recorded AF dataflash version word(s); a passing apply → flash → undo hardware cycle.

- [ ] **Step 1: AF version-word discovery**

The Pico on af_190602 reports version word `110` at dataflash offset 256
(Task 3 smoke test). Record `"fw_versions": [110]` for `af_190602` in
devices.json. (If other AF builds get flashed later, append their observed
words.) Rerun `test_match_build_rules`-style assertion mentally: with 110
recorded, `match_build("M041", 110)` now returns `ExactVersion` — add that
assertion to `stock_test.rs`:

```rust
#[test]
fn test_exact_version_match() {
    let lib = lib();
    let (kind, build) = match_build(&lib, "M041", 110);
    assert!(matches!(kind, MatchKind::ExactVersion));
    assert_eq!(build.unwrap().id, "af_190602");
}
```

- [ ] **Step 2: Full-cycle hardware test**

Add `test_stock_cycle_hardware` (`#[ignore]`) to `flasher_test.rs`:
1. read dataflash → pid/version; assert M041.
2. flash `resources/firmware/af_190602.bin` via
   `flasher::flash_firmware_guarded(&bytes, Some("M041"))` (the LDROM
   decrypts the package on-device; this is the same path the app uses).
   Note: flash the file **as shipped** (encrypted), not the decrypted copy.
3. Re-open, read dataflash, screenshot (AF: must respond), print nonzero
   count.
4. Undo: flash the backup made in Step 1's session — for the test, restore
   `AF_fw/decrypted/af_190602.dec.bin` via the recovery loop path, verify
   version word 110 again.

- [ ] **Step 3: Run the cycle on hardware**

```bash
flatpak run --env=FLATPAK_ENABLE_SDK_EXT=rust-stable --filesystem=home --device=all \
  --command=bash org.gnome.Sdk//49 -c 'export PATH=/usr/lib/sdk/rust-stable/bin:$PATH; \
  cd /var/home/j/cloudy-af/src-tauri && \
  cargo test --offline --lib firmware::tests::flasher_test::test_stock_cycle_hardware -- --ignored --nocapture'
```

Expected: PASS; device ends on af_190602. If anything wedges mid-flash, the
rescue kit (`DecryptProject/rescue/README.md`) recovers: battery out, hold
Plus, plug, flash stock, then reflash af_190602.

- [ ] **Step 4: Update goals.md and commit**

Mark the Phase 2 item done in `docs/goals.md`.

```bash
git add src-tauri/src/firmware/tests/flasher_test.rs src-tauri/src/firmware/tests/stock_test.rs \
  resources/firmware/devices.json docs/goals.md
git commit -m "test(firmware): stock match + flash + undo hardware cycle on AF"
```

---

## Out of scope (deliberate)

- **Instrumented verify-read** (HID `ReadFlash` injection into the af_190602
  dispatcher): requires the dispatcher RE (former readback-plan Task 1,
  `resources/re/af_190602-hid.json` was never produced). It belongs with the
  Phase 3 renderer-RE effort, which uses the same Ghidra session.
- STM32 enablement (Phase 5), animation effects (Phase 3+).

## Self-Review

**Spec coverage:** Workflow §1 Download Stock → Tasks 1–4; "bundle the four
builds + devices.json" → Task 1; "patch → re-encrypt → flash → undo wired
end-to-end" → existing pipeline + Task 5 hardware proof; "an unmatched build
is never flashed automatically" → Task 2 `NoLine` + Task 4 explicit-pick
flow; "STM32 ships greyed out" → Global Constraints + no STM32 UI task.
Gaps: instrumented verify-read deferred (see Out of scope).

**Placeholder scan:** devices.json schema, test code, commands, and shell
invocations are concrete. Task 2 implementation step describes logic in
prose but pins exact signatures; acceptable (implementation freedom, tested
behavior).

**Type consistency:** `match_build` returns `(MatchKind, Option<&StockBuild>)`
and is used that way in tests; `parse_fw_version(&[u8]) -> Result<i32>`
matches its committed implementation; `insert_opened` reuses the existing
`OpenFirmware` struct field-for-field.
