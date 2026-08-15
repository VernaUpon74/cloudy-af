# Firmware Editor Design

## Overview

Add a firmware resource editor to Cloudy AF, accessible from a new **Firmware Editor** button at the bottom of **Advanced → Settings**. It opens a separate Tauri window that will eventually host four sub-tabs matching NFirmwareEditor: **Patches**, **Images**, **Strings**, and **Resource Packs**.

The first implementation milestone is the **Patches** tab. It loads an ArcticFox/Joyetech `.bin` firmware file, decrypts it, detects the device definition, lists compatible XML patch files, and lets the user apply/rollback binary patches before saving the modified firmware back to disk.

## Entry Point

- Location: bottom of the **Advanced → Settings** panel in the main window.
- Element: button labeled `Firmware Editor`.
- Action: open a new Tauri window via the existing `ipc` bridge, similar to how the TFR editor is opened.

## Window Layout

A new Tauri window (`firmware.html` + `src/renderer-firmware.js`) contains:

1. A toolbar with **Open Firmware**, **Save**, **Save As** buttons.
2. A status label showing the loaded firmware name/definition.
3. A tab bar with four tabs: **Patches**, **Images**, **Strings**, **Resource Packs**.
4. A content area for the active tab.

For the first milestone, only the **Patches** tab is functional. The other three tabs render placeholder text/links to their future implementation.

## Architecture

### Backend responsibilities (Rust Tauri commands)

All byte-level, crypto, and filesystem operations live in the Rust backend so the frontend stays thin and safe.

- `open_firmware(path: String) -> Result<FirmwareHandle, String>`
  - Read the `.bin` file into memory.
  - Try decryptors in order: `None`, `Joyetech`, `ArcticFox`, `ArcticFox2`, `VandalProof`.
  - Detect the firmware definition by scanning device/version markers.
  - Return a handle/ID, definition metadata, encryption type, and list of available patches.
- `list_patches(definition: FirmwareDefinition, custom_dirs: Vec<String>) -> Vec<PatchInfo>`
  - Scan the bundled `resources/patches/` directory.
  - Scan user patch directories (e.g. `~/.config/cloudy-af/patches`).
  - Parse each `.patch` XML file and report compatibility/conflict status.
- `apply_patch(handle: String, patch_id: String) -> Result<PatchStatus, String>`
  - Apply the patch bytes to the in-memory firmware image.
  - Update status and conflict information.
- `rollback_patch(handle: String, patch_id: String) -> Result<PatchStatus, String>`
  - Restore original bytes for the patch.
- `save_firmware(handle: String, path: String, encryption: Option<String>) -> Result<(), String>`
  - Re-encrypt if requested and write the firmware to disk.
- `close_firmware(handle: String)`
  - Drop the in-memory firmware image.

### Frontend responsibilities

- Render the window chrome, toolbar, and tab bar.
- Load firmware via the Tauri dialog plugin.
- Display the patch table with status, description, author, version.
- Show patch details and Apply/Rollback controls.
- Call backend commands and reflect results/errors.

## Data Models

### FirmwareDefinition

```rust
struct FirmwareDefinition {
    id: String,
    name: String,
    marker: Vec<u8>,
    image_table_1: Range<usize>,
    image_table_2: Range<usize>,
    string_table_1: Range<usize>,
    string_table_2: Range<usize>,
    char_width: u8, // 1 or 2
}
```

Definitions are stored as XML in `resources/definitions/` and parsed at startup. The format mirrors the original NFirmwareEditor `.xml` definition files.

### Patch

```rust
struct Patch {
    id: String,
    name: String,
    version: String,
    author: String,
    description: String,
    modifications: Vec<PatchModification>,
}

struct PatchModification {
    offset: usize,
    original: Option<u8>, // None means wildcard
    patched: u8,
}
```

Patch files are XML with a `<Data>` block containing hex lines in `offset: oldByte - newByte` form. Wildcards (`*`) and comments (`#`/`;`) are supported.

## Encryption Support

The backend implements the same encryption/decryption algorithms used by NFirmwareEditor:

1. **None** — plain binary.
2. **Joyetech** — XOR with a length+magic-derived function.
3. **ArcticFox** — 4-byte key + pseudo-random XOR table.
4. **ArcticFox2** — variant of the above if distinct in the original source.
5. **VandalProof** — attempt a clean-room reimplementation of the obfuscated loader logic found in `NCore/VandalProofEncryption.cs`. If the obfuscation or external-assembly dependency proves impractical to port safely, the backend returns a clear error and VandalProof firmwares remain unsupported.

## Patch Application Logic

1. For each modification in a patch:
   - Read the byte at the offset.
   - If `original` is `Some(byte)`, verify it matches; otherwise fail with a compatibility error.
   - If `original` is `None` (wildcard), accept any current byte.
   - Write `patched` to the offset and record the previous byte for rollback.
2. Track applied patches per handle.
3. Detect conflicts: two patches modifying the same byte with different `patched` values.
4. Rollback restores previously recorded original bytes.

## Error Handling

- Invalid/corrupt firmware files show a clear error dialog.
- Incompatible patches (original byte mismatch) are marked incompatible and cannot be applied.
- Conflicting patches are highlighted; applying one disables the conflicting one until rolled back.
- Save failures (permissions, disk full) report the underlying error.

## Future Tabs

The window is designed so the following can be added later without changing the backend/frontend contract:

- **Images**: canvas-based pixel editor for Block1/Block2 packed bitmaps, import/export BMP/PNG, resource pack import.
- **Strings**: string table editor with glyph picker and live preview.
- **Resource Packs**: `.respack` discovery, preview diff, apply to Block1/Block2.

## Testing

- Unit tests for each decryptor using small known-ciphertext samples.
- Unit tests for patch apply/rollback/conflict logic.
- Manual end-to-end test: open a known ArcticFox `.bin`, apply a known patch, save, and verify the byte change with a hex dump.
