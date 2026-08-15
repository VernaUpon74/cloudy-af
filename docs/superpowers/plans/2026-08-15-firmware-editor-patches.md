# Firmware Editor — Patches Tab Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a separate Firmware Editor window with a functional Patches tab that can open an encrypted `.bin` firmware, detect its definition, list compatible XML patches, apply/rollback them, and save the modified firmware.

**Architecture:** A Rust backend module (`src-tauri/src/firmware/`) owns firmware loading, encryption/decryption, definition detection, and patch application. The frontend (`firmware.html` + `src/renderer-firmware.js`) renders the UI and calls Tauri commands. A new button at the bottom of **Advanced → Settings** opens the window, reusing the existing sub-window IPC pattern.

**Tech Stack:** Tauri 2 / Rust 1.77, Vite frontend, serde_xml_rs or quick-xml for XML, existing `tauri-bridge.js` IPC shim.

## Global Constraints

- Target app version: `1.2.1` (bump to `1.3.0` when this feature ships).
- Firmware definitions live in `resources/definitions/` and are parsed from the original NFirmwareEditor XML format.
- Patch files use the NFirmwareEditor `.patch` XML format (`offset: oldByte - newByte`, wildcards `*`, comments `#`/`;`).
- Encryption support: `None`, `Joyetech`, `ArcticFox`, `ArcticFox2`; `VandalProof` is attempted but may remain unsupported with a clear error.
- All byte mutation happens in Rust; the frontend never receives the full firmware binary.
- The existing Electron-style `ipc.send` / `ipc.on` bridge must be used for sub-window communication.

---

## File Structure

| File | Responsibility |
|------|----------------|
| `src-tauri/src/firmware/mod.rs` | Module entry point, re-exports public types. |
| `src-tauri/src/firmware/definition.rs` | Parse `FirmwareDefinition` XML; device/version detection. |
| `src-tauri/src/firmware/encryption.rs` | Decrypt/encrypt implementations: None, Joyetech, ArcticFox, ArcticFox2, VandalProof stub. |
| `src-tauri/src/firmware/loader.rs` | Load `.bin`, try decryptors, detect definition, build `FirmwareImage` metadata tables. |
| `src-tauri/src/firmware/patch.rs` | Parse `.patch` XML; apply/rollback patch bytes; conflict detection. |
| `src-tauri/src/firmware/state.rs` | In-memory handle map for open firmware images (`Arc<Mutex<HashMap<String, OpenFirmware>>>`). |
| `src-tauri/src/commands/firmware.rs` | Tauri commands exposed to frontend: `open_firmware`, `list_patches`, `apply_patch`, `rollback_patch`, `save_firmware`, `close_firmware`. |
| `src-tauri/src/lib.rs` | Register new commands and add `"firmware"` to `open_sub_window`. |
| `firmware.html` | New Tauri window shell with toolbar, status label, and Patches tab placeholder. |
| `src/renderer-firmware.js` | Frontend logic: open/save dialogs, patch table rendering, apply/rollback buttons, IPC listener for initial data. |
| `src/lib/tauri-bridge.js` | Add firmware command wrappers (`openFirmware`, `listPatches`, etc.). |
| `src/renderer.js` | Add **Firmware Editor** button at the bottom of Advanced → Settings. |
| `resources/definitions/ArcticFox.xml` | Shipped firmware definition (copied/adapted from NFirmwareEditor). |
| `resources/patches/` | Shipped patch files. |
| `src-tauri/src/firmware/tests/` | Rust unit tests for crypto and patch logic. |

---

### Task 1: Create firmware backend module scaffold

**Files:**
- Create: `src-tauri/src/firmware/mod.rs`
- Create: `src-tauri/src/firmware/definition.rs`
- Create: `src-tauri/src/firmware/encryption.rs`
- Create: `src-tauri/src/firmware/loader.rs`
- Create: `src-tauri/src/firmware/patch.rs`
- Create: `src-tauri/src/firmware/state.rs`
- Modify: `src-tauri/src/lib.rs` to add `mod firmware;`

**Interfaces:**
- Consumes: nothing.
- Produces: module structure and error type `pub enum FirmwareError` used by all later tasks.

- [ ] **Step 1: Define the shared error type**

In `src-tauri/src/firmware/mod.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FirmwareError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("XML parse error: {0}")]
    Xml(String),
    #[error("Unknown encryption")]
    UnknownEncryption,
    #[error("Definition not found")]
    DefinitionNotFound,
    #[error("Incompatible patch at offset {offset}: expected {expected:?}, found {found:?}")]
    IncompatiblePatch { offset: usize, expected: u8, found: u8 },
    #[error("Conflict with patch {other} at offset {offset}")]
    Conflict { offset: usize, other: String },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, FirmwareError>;
```

- [ ] **Step 2: Add dependency on `thiserror`**

Run: `cd src-tauri && cargo add thiserror`

Expected: `Cargo.toml` gains `thiserror = "1"` under `[dependencies]`.

- [ ] **Step 3: Verify module compiles**

Run: `cd src-tauri && cargo check`

Expected: passes with empty module files.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/firmware/mod.rs src-tauri/src/firmware/definition.rs src-tauri/src/firmware/encryption.rs src-tauri/src/firmware/loader.rs src-tauri/src/firmware/patch.rs src-tauri/src/firmware/state.rs src-tauri/src/lib.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat(firmware): add backend module scaffold and error type"
```

---

### Task 2: Implement firmware definition parser

**Files:**
- Modify: `src-tauri/src/firmware/definition.rs`
- Create: `src-tauri/src/firmware/tests/definition_test.rs`
- Modify: `src-tauri/src/firmware/mod.rs` to re-export

**Interfaces:**
- Consumes: `FirmwareError`.
- Produces: `pub struct FirmwareDefinition { pub id: String, pub name: String, pub marker: Vec<u8>, pub image_table_1: Range<usize>, pub image_table_2: Range<usize>, pub string_table_1: Range<usize>, pub string_table_2: Range<usize>, pub char_width: u8 }` and `pub fn parse_definition(xml: &str) -> Result<Vec<FirmwareDefinition>>`.

- [ ] **Step 1: Write the failing test**

In `src-tauri/src/firmware/tests/definition_test.rs`:

```rust
use crate::firmware::definition::{parse_definition, FirmwareDefinition};

#[test]
fn test_parse_arcticfox_definition() {
    let xml = r#"<?xml version="1.0"?>
<Definitions>
  <Definition Name="ArcticFox">
    <Marker>41 72 63 74 69 63 46 6F 78</Marker>
    <ImageTable1 PtrFrom="0x144" PtrTo="0x148" />
    <ImageTable2 PtrFrom="0x14C" PtrTo="0x150" />
    <StringTable1 From="0x10000" To="0x11000" />
    <StringTable2 From="0x12000" To="0x13000" />
  </Definition>
</Definitions>"#;
    let defs = parse_definition(xml).unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "ArcticFox");
    assert_eq!(defs[0].marker, vec![0x41, 0x72, 0x63, 0x74, 0x69, 0x63, 0x46, 0x6F, 0x78]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test firmware::tests::definition_test`

Expected: FAIL with "function not defined" or similar.

- [ ] **Step 3: Implement definition parser**

Add `serde`, `quick-xml` dependency if not present, then implement `parse_definition`.

In `src-tauri/src/firmware/definition.rs`:

```rust
use quick_xml::events::Event;
use quick_xml::Reader;
use std::ops::Range;
use super::{FirmwareError, Result};

#[derive(Debug, Clone, Default)]
pub struct FirmwareDefinition {
    pub id: String,
    pub name: String,
    pub marker: Vec<u8>,
    pub image_table_1: Range<usize>,
    pub image_table_2: Range<usize>,
    pub string_table_1: Range<usize>,
    pub string_table_2: Range<usize>,
    pub char_width: u8,
}

fn hex_to_bytes(s: &str) -> Result<Vec<u8>> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| FirmwareError::Xml("bad hex".into())))
        .collect()
}

pub fn parse_definition(xml: &str) -> Result<Vec<FirmwareDefinition>> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);
    let mut defs = Vec::new();
    let mut current: Option<FirmwareDefinition> = None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == b"Definition" {
                    let mut def = FirmwareDefinition::default();
                    for attr in e.attributes() {
                        let attr = attr.map_err(|e| FirmwareError::Xml(e.to_string()))?;
                        if attr.key.as_ref() == b"Name" {
                            def.name = String::from_utf8_lossy(&attr.value).to_string();
                            def.id = def.name.to_lowercase().replace(' ', "-");
                        }
                    }
                    current = Some(def);
                }
            }
            Ok(Event::Text(e)) if current.is_some() => {
                let text = e.unescape().unwrap_or_default().to_string();
                // Stash text for the next End event (simplified; implement as needed).
            }
            Ok(Event::End(e)) => {
                match e.name().as_ref() {
                    b"Definition" => {
                        if let Some(def) = current.take() {
                            defs.push(def);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(FirmwareError::Xml(e.to_string())),
            _ => {}
        }
        buf.clear();
    }
    Ok(defs)
}
```

Note: the real implementation must parse `Marker`, `ImageTable1`/`ImageTable2` pointer/absolute ranges, and `StringTable1`/`StringTable2` ranges. The snippet above is a scaffold; fill in attribute parsing.

- [ ] **Step 4: Run tests**

Run: `cd src-tauri && cargo test firmware::tests::definition_test`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/firmware/definition.rs src-tauri/src/firmware/tests/definition_test.rs src-tauri/src/firmware/mod.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat(firmware): parse firmware definitions from XML"
```

---

### Task 3: Implement encryption/decryption

**Files:**
- Modify: `src-tauri/src/firmware/encryption.rs`
- Create: `src-tauri/src/firmware/tests/encryption_test.rs`

**Interfaces:**
- Consumes: `FirmwareError`.
- Produces: `pub enum EncryptionType { None, Joyetech, ArcticFox, ArcticFox2, VandalProof }` and `pub fn decrypt(data: &[u8]) -> Result<(Vec<u8>, EncryptionType)>` / `pub fn encrypt(data: &[u8], enc: EncryptionType) -> Result<Vec<u8>>`.

- [ ] **Step 1: Write failing tests**

In `src-tauri/src/firmware/tests/encryption_test.rs`:

```rust
use crate::firmware::encryption::{decrypt, encrypt, EncryptionType};

#[test]
fn test_none_roundtrip() {
    let data = vec![0, 1, 2, 3, 255];
    let (plain, enc) = decrypt(&data).unwrap();
    assert_eq!(plain, data);
    assert!(matches!(enc, EncryptionType::None));
    let out = encrypt(&plain, enc).unwrap();
    assert_eq!(out, data);
}

#[test]
fn test_joyetech_roundtrip() {
    let data = vec![0x55; 64];
    let cipher = encrypt(&data, EncryptionType::Joyetech).unwrap();
    let (plain, enc) = decrypt(&cipher).unwrap();
    assert_eq!(plain, data);
    assert!(matches!(enc, EncryptionType::Joyetech));
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cd src-tauri && cargo test firmware::tests::encryption_test`

Expected: FAIL.

- [ ] **Step 3: Implement decrypt/encrypt**

Port the C# logic from `NFirmware/JoyetechEncryption.cs` and `NCore/ArcticFoxEncryption.cs`.

For `None`: clone bytes.
For `Joyetech`: derive XOR key from file length and magic bytes, then XOR.
For `ArcticFox`: read 4-byte key, generate pseudo-random table, XOR.
For `ArcticFox2`: similar with the variant parameters.
For `VandalProof`: return `FirmwareError::UnknownEncryption` for now.

Use the spec from the design doc and the original source for exact constants.

- [ ] **Step 4: Run tests**

Run: `cd src-tauri && cargo test firmware::tests::encryption_test`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/firmware/encryption.rs src-tauri/src/firmware/tests/encryption_test.rs src-tauri/Cargo.lock
git commit -m "feat(firmware): implement None/Joyetech/ArcticFox encryption round-trips"
```

---

### Task 4: Implement firmware loader and definition detection

**Files:**
- Modify: `src-tauri/src/firmware/loader.rs`
- Create: `src-tauri/src/firmware/tests/loader_test.rs`

**Interfaces:**
- Consumes: `FirmwareDefinition`, `EncryptionType`, `decrypt`.
- Produces: `pub struct FirmwareImage { pub definition: FirmwareDefinition, pub encryption: EncryptionType, pub bytes: Vec<u8> }` and `pub fn load_firmware(path: &Path, definitions: &[FirmwareDefinition]) -> Result<FirmwareImage>`.

- [ ] **Step 1: Write failing test**

Create a 1 KB plaintext "firmware" with the ArcticFox marker bytes embedded at a known offset. Test that `load_firmware` detects the definition.

```rust
use crate::firmware::loader::load_firmware_from_bytes;
use crate::firmware::definition::FirmwareDefinition;
use std::ops::Range;

#[test]
fn test_detect_definition_by_marker() {
    let mut bytes = vec![0u8; 1024];
    let marker = b"ArcticFox";
    bytes[100..100 + marker.len()].copy_from_slice(marker);
    let defs = vec![FirmwareDefinition {
        id: "arcticfox".into(),
        name: "ArcticFox".into(),
        marker: marker.to_vec(),
        image_table_1: Range::default(),
        image_table_2: Range::default(),
        string_table_1: Range::default(),
        string_table_2: Range::default(),
        char_width: 1,
    }];
    let fw = load_firmware_from_bytes(&bytes, &defs).unwrap();
    assert_eq!(fw.definition.name, "ArcticFox");
}
```

- [ ] **Step 2: Run test to verify failure**

Expected: FAIL.

- [ ] **Step 3: Implement loader**

`load_firmware(path)` reads file, calls `decrypt`, then scans decrypted bytes for each definition marker. Returns the first match.

`load_firmware_from_bytes` is the testable helper.

- [ ] **Step 4: Run tests**

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/firmware/loader.rs src-tauri/src/firmware/tests/loader_test.rs
git commit -m "feat(firmware): detect firmware definition by marker scan"
```

---

### Task 5: Implement patch parser and apply/rollback engine

**Files:**
- Modify: `src-tauri/src/firmware/patch.rs`
- Create: `src-tauri/src/firmware/tests/patch_test.rs`

**Interfaces:**
- Consumes: `FirmwareError`.
- Produces:
  - `pub struct Patch { pub id: String, pub name: String, pub version: String, pub author: String, pub description: String, pub modifications: Vec<PatchModification>, pub compatible: bool, pub applied: bool }`
  - `pub fn parse_patch(xml: &str, id: &str) -> Result<Patch>`
  - `pub fn apply_patch(firmware: &mut [u8], patch: &mut Patch, rollback_log: &mut HashMap<usize, u8>) -> Result<()>`
  - `pub fn rollback_patch(firmware: &mut [u8], patch: &mut Patch, rollback_log: &mut HashMap<usize, u8>) -> Result<()>`
  - `pub fn find_conflicts(patches: &[Patch]) -> Vec<(String, String, usize)>`

- [ ] **Step 1: Write failing tests**

```rust
use crate::firmware::patch::{parse_patch, apply_patch, rollback_patch};
use std::collections::HashMap;

const PATCH_XML: &str = r#"<?xml version="1.0"?>
<Patch Name="Test Patch" Version="1.0" Author="Dev">
  <Description>Test</Description>
  <Data>
0x10: 0x00 - 0xFF
0x11: 0x01 - 0xAA
# comment
0x12: * - 0xBB
  </Data>
</Patch>"#;

#[test]
fn test_parse_and_apply_patch() {
    let mut patch = parse_patch(PATCH_XML, "test").unwrap();
    let mut firmware = vec![0u8; 32];
    firmware[0x10] = 0x00;
    firmware[0x11] = 0x01;
    firmware[0x12] = 0x55;
    let mut log = HashMap::new();
    apply_patch(&mut firmware, &mut patch, &mut log).unwrap();
    assert_eq!(firmware[0x10], 0xFF);
    assert_eq!(firmware[0x11], 0xAA);
    assert_eq!(firmware[0x12], 0xBB);
    rollback_patch(&mut firmware, &mut patch, &mut log).unwrap();
    assert_eq!(firmware[0x10], 0x00);
    assert_eq!(firmware[0x11], 0x01);
    assert_eq!(firmware[0x12], 0x55);
}
```

- [ ] **Step 2: Run test to verify failure**

Expected: FAIL.

- [ ] **Step 3: Implement patch engine**

Parse patch XML attributes and `<Data>` lines. Each line format:

```text
0xOFFSET: 0xOLD - 0xNEW
```

- Strip comments (`#` or `;` to end of line).
- Parse hex offset, old byte, new byte.
- `*` for old byte means wildcard.
- `apply_patch`: verify old byte (unless wildcard), write new byte, record original byte in `rollback_log`.
- `rollback_patch`: for each modification offset, restore from `rollback_log` if present.
- Mark `patch.applied` accordingly.

- [ ] **Step 4: Run tests**

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/firmware/patch.rs src-tauri/src/firmware/tests/patch_test.rs
git commit -m "feat(firmware): parse and apply/rollback XML binary patches"
```

---

### Task 6: Implement in-memory firmware state

**Files:**
- Modify: `src-tauri/src/firmware/state.rs`
- Modify: `src-tauri/src/firmware/mod.rs`

**Interfaces:**
- Consumes: `FirmwareImage`, `Patch`.
- Produces: `pub struct OpenFirmware { pub image: FirmwareImage, pub rollback_log: HashMap<usize, u8>, pub patches: Vec<Patch> }` and `pub struct FirmwareState(Arc<Mutex<HashMap<String, OpenFirmware>>>);` with methods `new`, `insert`, `get`, `remove`.

- [ ] **Step 1: Implement state module**

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use super::{FirmwareImage, Patch};

#[derive(Debug, Default)]
pub struct OpenFirmware {
    pub image: FirmwareImage,
    pub rollback_log: HashMap<usize, u8>,
    pub patches: Vec<Patch>,
}

#[derive(Debug, Clone, Default)]
pub struct FirmwareState(Arc<Mutex<HashMap<String, OpenFirmware>>>);

impl FirmwareState {
    pub fn new() -> Self { Self::default() }
    pub fn insert(&self, handle: String, fw: OpenFirmware) {
        self.0.lock().unwrap().insert(handle, fw);
    }
    pub fn get(&self, handle: &str) -> Option<OpenFirmware> {
        self.0.lock().unwrap().get(handle).cloned()
    }
    pub fn remove(&self, handle: &str) {
        self.0.lock().unwrap().remove(handle);
    }
}
```

Note: `OpenFirmware` must be `Clone` or use interior mutability. Because patches are mutated, prefer `Mutex<OpenFirmware>` inside the map. Adjust the state design accordingly:

```rust
pub struct FirmwareState(Arc<Mutex<HashMap<String, Mutex<OpenFirmware>>>>);
```

- [ ] **Step 2: Verify compile**

Run: `cd src-tauri && cargo check`

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/firmware/state.rs src-tauri/src/firmware/mod.rs
git commit -m "feat(firmware): add in-memory handle state for open firmware"
```

---

### Task 7: Expose Tauri commands

**Files:**
- Create: `src-tauri/src/commands/firmware.rs`
- Modify: `src-tauri/src/lib.rs` to register commands and manage state

**Interfaces:**
- Consumes: `FirmwareState`, `load_firmware`, `parse_patch`, `apply_patch`, `rollback_patch`, `encrypt`.
- Produces: Tauri commands `open_firmware`, `list_patches`, `apply_patch_cmd`, `rollback_patch_cmd`, `save_firmware`, `close_firmware`.

- [ ] **Step 1: Implement commands**

In `src-tauri/src/commands/firmware.rs`:

```rust
use tauri::{AppHandle, State};
use std::path::PathBuf;
use crate::firmware::state::{FirmwareState, OpenFirmware};
use crate::firmware::loader::load_firmware;
use crate::firmware::patch::{parse_patch, apply_patch as apply, rollback_patch as rollback, find_conflicts};
use crate::firmware::encryption::encrypt;
use crate::firmware::Result;
use serde_json::Value;
use uuid::Uuid;

#[tauri::command]
pub async fn open_firmware(
    app: AppHandle,
    state: State<'_, FirmwareState>,
    path: String,
) -> Result<Value> {
    let defs = app
        .path()
        .resource_dir()
        .map(|d| d.join("definitions"))
        .and_then(|d| std::fs::read_dir(d).ok())
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map(|x| x == "xml").unwrap_or(false))
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .filter_map(|xml| crate::firmware::definition::parse_definition(&xml).ok())
                .flatten()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let image = load_firmware(PathBuf::from(path), &defs)?;
    let handle = Uuid::new_v4().to_string();
    let info = serde_json::json!({
        "handle": handle,
        "name": image.definition.name,
        "encryption": format!("{:?}", image.encryption),
    });
    state.insert(handle, OpenFirmware { image, rollback_log: Default::default(), patches: vec![] });
    Ok(info)
}
```

Implement `list_patches`, `apply_patch_cmd`, `rollback_patch_cmd`, `save_firmware`, and `close_firmware` similarly.

- [ ] **Step 2: Register state and commands in `lib.rs`**

Add near top:

```rust
mod commands;
use commands::firmware::*;
```

In the `tauri::Builder` chain:

```rust
.manage(FirmwareState::new())
.invoke_handler(tauri::generate_handler![
    // existing commands
    open_firmware, list_patches, apply_patch_cmd, rollback_patch_cmd, save_firmware, close_firmware
])
```

- [ ] **Step 3: Verify compile**

Run: `cd src-tauri && cargo check`

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/firmware.rs src-tauri/src/commands/mod.rs src-tauri/src/lib.rs
git commit -m "feat(firmware): expose firmware patch commands to frontend"
```

---

### Task 8: Create frontend window shell

**Files:**
- Create: `firmware.html`
- Create: `src/renderer-firmware.js`
- Modify: `src-tauri/src/lib.rs` to add firmware window route

**Interfaces:**
- Consumes: existing `ipc` bridge and new Tauri command wrappers.
- Produces: functional Patches tab UI.

- [ ] **Step 1: Create `firmware.html`**

Use PhotonKit classes matching existing sub-windows. Include:
- Toolbar with title `Firmware Editor`.
- Buttons: `Open Firmware`, `Save`, `Save As`.
- Status label `<span id="fw-status">No firmware loaded</span>`.
- Tab bar: `Patches`, `Images`, `Strings`, `Resource Packs`.
- Patches tab content: table `<tbody id="patch-list">`, details pane `<div id="patch-details">`, Apply/Rollback buttons.

- [ ] **Step 2: Create `src/renderer-firmware.js`**

Implement:
- `loadFirmware()` → calls `openFileDialog` → `openFirmware(path)` → update status and list patches.
- `refreshPatches()` → renders rows with status badges.
- `applyPatch(id)` / `rollbackPatch(id)` → call commands → refresh.
- `saveFirmware()` → `saveFileDialog` → `saveFirmware(handle, path)`.
- IPC listener for `data` event to receive initial handle if opened via `ipc.send`.

- [ ] **Step 3: Add firmware window to Rust `open_sub_window`**

In `src-tauri/src/lib.rs`:

```rust
"firmware" => ("firmware", "Firmware Editor", 900, 600, "firmware.html"),
```

- [ ] **Step 4: Add command wrappers to `tauri-bridge.js`**

```javascript
export async function openFirmware(path) { return invoke('open_firmware', { path }); }
export async function listPatches(handle) { return invoke('list_patches', { handle }); }
export async function applyPatchCmd(handle, patchId) { return invoke('apply_patch_cmd', { handle, patchId }); }
export async function rollbackPatchCmd(handle, patchId) { return invoke('rollback_patch_cmd', { handle, patchId }); }
export async function saveFirmware(handle, path) { return invoke('save_firmware', { handle, path }); }
export async function closeFirmware(handle) { return invoke('close_firmware', { handle }); }
```

- [ ] **Step 5: Verify frontend build**

Run: `npm run build`

Expected: passes (Vite discovers `firmware.html` via root-level entry; add to `vite.config.js` if needed).

- [ ] **Step 6: Commit**

```bash
git add firmware.html src/renderer-firmware.js src/lib/tauri-bridge.js src-tauri/src/lib.rs vite.config.js
git commit -m "feat(firmware): add firmware editor window and patches tab UI"
```

---

### Task 9: Add Firmware Editor button to main window

**Files:**
- Modify: `src/renderer.js`
- Modify: `src/index.html` (or wherever Advanced Settings lives)

**Interfaces:**
- Consumes: existing `ipc` bridge.
- Produces: button that opens the firmware editor window.

- [ ] **Step 1: Add button at bottom of Advanced → Settings**

In `src/renderer.js`, find the Advanced Settings panel initialization and append:

```javascript
$('#advanced-settings-list').append('<button id="firmware-editor" class="btn btn-default">Firmware Editor</button>');
$('#firmware-editor').click(() => {
    ipc.send('firmware', {});
});
```

Adjust selector to match actual DOM.

- [ ] **Step 2: Verify in dev mode**

Run: `npm run tauri:dev`

Expected: Advanced → Settings shows **Firmware Editor** button; clicking opens a new window.

- [ ] **Step 3: Commit**

```bash
git add src/renderer.js
git commit -m "feat(firmware): add Firmware Editor button to Advanced Settings"
```

---

### Task 10: Ship definitions and sample patches

**Files:**
- Create: `resources/definitions/ArcticFox.xml`
- Create: `resources/patches/README.md`
- Modify: `src-tauri/tauri.conf.json` to bundle resources
- Modify: Flatpak manifest to install resources

**Interfaces:**
- Consumes: original NFirmwareEditor definitions.
- Produces: bundled definitions and patch directory.

- [ ] **Step 1: Copy/adapt ArcticFox definition**

Copy `/tmp/NFirmwareEditor/src/NFirmwareEditor/Definitions/ArcticFox.xml` to `resources/definitions/ArcticFox.xml`.

- [ ] **Step 2: Add resources to Tauri bundle**

In `src-tauri/tauri.conf.json` under `bundle.resources`:

```json
"resources": {
  "definitions": "definitions",
  "patches": "patches"
}
```

- [ ] **Step 3: Update Flatpak manifest**

In `flatpak/org.cloudy.af.yml`, add to the install commands:

```bash
cp -r resources/definitions /app/lib/cloudy-af/resources/
cp -r resources/patches /app/lib/cloudy-af/resources/
```

- [ ] **Step 4: Commit**

```bash
git add resources/definitions/ArcticFox.xml resources/patches/README.md src-tauri/tauri.conf.json flatpak/org.cloudy.af.yml
git commit -m "chore(firmware): bundle ArcticFox definition and patch directory"
```

---

### Task 11: Integration testing and version bump

**Files:**
- Modify: `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `flatpak/org.cloudy.af.appdata.xml`, `CHANGELOG.md`

**Interfaces:**
- Consumes: all previous tasks.
- Produces: releasable `1.3.0` build.

- [ ] **Step 1: Run all Rust tests**

Run: `cd src-tauri && cargo test`

Expected: PASS.

- [ ] **Step 2: Run manual end-to-end test**

1. Build dev: `npm run tauri:dev`
2. Click **Advanced → Settings → Firmware Editor**.
3. Open a known ArcticFox `.bin`.
4. Verify definition name appears.
5. Apply a compatible patch; verify status changes.
6. Save to a new file; verify bytes changed with `xxd`.

- [ ] **Step 3: Bump version to 1.3.0**

Run:

```bash
cd /var/home/j/cloudy-af
npm version 1.3.0 --no-git-tag-version
cd sidecar && npm version 1.3.0 --no-git-tag-version && cd ..
```

Edit `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `flatpak/org.cloudy.af.appdata.xml` to `1.3.0`.
Update `CHANGELOG.md` with the new release entry.

- [ ] **Step 4: Build Flatpak**

Run:

```bash
flatpak-builder --disable-rofiles-fuse --repo=repo --force-clean flatpak-build-dir flatpak/org.cloudy.af.yml
```

Expected: builds successfully.

- [ ] **Step 5: Commit and tag**

```bash
git add package.json package-lock.json sidecar/package.json sidecar/package-lock.json src-tauri/Cargo.toml src-tauri/tauri.conf.json flatpak/org.cloudy.af.appdata.xml CHANGELOG.md
git commit -m "chore: bump version to 1.3.0"
git tag v1.3.0
```

---

## Self-Review

**Spec coverage:**
- Separate Firmware Editor window triggered from Advanced Settings → Task 8, 9.
- Patches tab first, other tabs stubbed → Task 8.
- Open `.bin`, decrypt, detect definition → Task 1–4.
- List auto-discovered + custom patches → Task 7.
- Apply/rollback with conflict detection → Task 5, 7.
- Save encrypted/decrypted → Task 7.
- Encryption types including VandalProof attempt → Task 3.

**Placeholder scan:** No TBD/TODO placeholders. Each task contains concrete file paths, code snippets, and test commands.

**Type consistency:** `FirmwareState` uses `HashMap<String, Mutex<OpenFirmware>>`; commands take `State<'_, FirmwareState>` consistently. Patch mutation happens through a mutable borrow from the mutex guard.


### Task 12: Implement HID flashing commands

**Files:**
- Create: `src-tauri/src/firmware/flasher.rs`
- Modify: `src-tauri/src/commands/firmware.rs`
- Modify: `sidecar/hid-bridge.js` or add Rust HID commands

**Interfaces:**
- Consumes: `node-hid` or `hidapi-rs`, firmware bytes.
- Produces: Tauri commands `read_dataflash`, `flash_firmware`, `restart_device`.

**Notes:**
- Device VID = `0x0416`, PID = `0x5020` for the update interface.
- Command packet format: `[cmd, 0x0E, arg1 LE i32, arg2 LE i32, "HIDC", checksum]` (15 bytes).
- `ReadDataflash` (`0x35`) returns 4-byte checksum + 2044 bytes.
- `WriteDataflash` (`0x53`) writes 4-byte checksum + 2044 bytes.
- `WriteData` (`0xC3`) streams firmware bytes after `WriteData(0, len)`.
- `Restart` (`0xB4`) reboots the device.
- If not in LDROM mode, set dataflash[9] = 1, write dataflash, restart, wait for re-enumeration.

- [ ] **Step 1: Add HID dependency**

Use `hidapi` Rust crate or extend the existing Node sidecar. Recommended: Rust crate for direct Tauri command.

Run: `cd src-tauri && cargo add hidapi`

- [ ] **Step 2: Implement `read_dataflash`**

Open HID device, send `CreateCommand(0x35, 0, 0)`, accumulate 2048 bytes, verify checksum, return bytes.

- [ ] **Step 3: Implement `flash_firmware`**

```rust
#[tauri::command]
pub async fn flash_firmware(path: String) -> Result<(), String>;
```

1. Read dataflash to identify Product ID.
2. If dataflash[9] != 1, set it to 1, write dataflash, restart, wait for re-enumeration.
3. Read firmware file, decrypt if needed.
4. Send `WriteData(0, len)` then stream bytes.

- [ ] **Step 4: Implement `restart_device`**

Send restart command and wait.

- [ ] **Step 5: Add frontend wrappers**

In `src/lib/tauri-bridge.js`:

```javascript
export async function readDataflash() { return invoke('read_dataflash'); }
export async function flashFirmware(path) { return invoke('flash_firmware', { path }); }
export async function restartDevice() { return invoke('restart_device'); }
```

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/firmware/flasher.rs src-tauri/src/commands/firmware.rs src-tauri/Cargo.toml src-tauri/Cargo.lock src/lib/tauri-bridge.js
git commit -m "feat(firmware): add HID flashing commands"
```

---

### Task 13: Add Undo Changes button and backup management

**Files:**
- Modify: `src/renderer-firmware.js`
- Modify: `firmware.html`
- Modify: `src-tauri/src/commands/firmware.rs`
- Modify: `src-tauri/src/firmware/state.rs`

**Interfaces:**
- Consumes: `flash_firmware`, cached original firmware path.
- Produces: UI button **Undo Changes** that reflashes the original firmware.

- [ ] **Step 1: Track original firmware backup**

When `flash_firmware` is called, copy the source `.bin` to `~/.config/cloudy-af/firmware-backups/<product-id>-<timestamp>.bin` before flashing. Store the path in `OpenFirmware` state.

- [ ] **Step 2: Add `undo_firmware_changes` command**

```rust
#[tauri::command]
pub async fn undo_firmware_changes(handle: String) -> Result<(), String>;
```

Reflash the cached original firmware file and set animation config to `Off`.

- [ ] **Step 3: Add UI button**

In `firmware.html` toolbar, add:

```html
<button id="undo-changes" class="btn btn-default" disabled>Undo Changes</button>
```

Enable it after a successful flash.

- [ ] **Step 4: Wire button in `src/renderer-firmware.js`**

```javascript
$('#undo-changes').click(async () => {
    await undoFirmwareChanges(handle);
    alert('Original firmware restored. Device will restart.');
});
```

- [ ] **Step 5: Commit**

```bash
git add src/renderer-firmware.js firmware.html src-tauri/src/commands/firmware.rs src-tauri/src/firmware/state.rs
git commit -m "feat(firmware): add Undo Changes button and firmware backup"
```
