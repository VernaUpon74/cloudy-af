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
5. **VandalProof** — AES-128-CBC; key is the ASCII string `FA89412D87B0EFD9`, IV is the first 16 bytes of the file, PKCS7 padding. Implemented and tested against real 2018+ builds; format and key provenance are documented in `docs/vandalproof-encryption.md`.

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


## Deep Dive: Images, Resource Packs, and On-Device Display

This section documents the firmware image system so future work on the Images, Strings, Resource Packs tabs — and a planned animation feature — has a solid reference.

### Firmware Image Block Formats

Every stored image begins with a 2-byte header:

| Offset | Field  | Type   | Meaning                    |
|--------|--------|--------|----------------------------|
| 0      | Width  | `byte` | Image width in pixels      |
| 1      | Height | `byte` | Image height in pixels     |

The pixel payload follows immediately after the header.

#### Block1 — SSD1306-style vertical packing

- Used by many ArcticFox monochrome OLED displays.
- `DataLength = Width * ceil(Height / 8)`.
- Byte layout: each byte stores 8 vertical pixels in one column.
  - `byteIndex = (y / 8) * Width + x`
  - `bitIndex  = y % 8`
  - `imageBytes[byteIndex] |= (1 << bitIndex)` sets pixel `(x, y)`
  - Bit 0 is the top pixel of the 8-pixel group.

#### Block2 — SSD1327-style horizontal packing

- Used by some larger/color-compatible displays.
- `DataLength = ceil(Width / 8) * Height`.
- Byte layout: each byte stores 8 horizontal pixels in one row.
  - `byteIndex = y * stride + x / 8`, where `stride = ceil(Width / 8)`
  - `bitIndex  = 7 - (x % 8)`
  - `imageBytes[byteIndex] |= (1 << bitIndex)` sets pixel `(x, y)`
  - The most significant bit is the leftmost pixel.

#### Image table layout

Definitions specify `ImageTable1` and `ImageTable2` as either absolute ranges (`From`/`To`) or pointer-table bounds (`PtrFrom`/`PtrTo`). For ArcticFox the pointer-table form is used:

```xml
<ImageTable1 PtrFrom="0x144" PtrTo="0x148" />
<ImageTable2 PtrFrom="0x14C" PtrTo="0x150" />
```

The loader reads a table of 32-bit image-data offsets. A zero offset means “no image.” Image indices are 1-based (`0x01`, `0x02`, …). The loader seeks to each offset and reads the 2-byte header to obtain width and height.

### Resource Pack Format (`.respack`)

A `.respack` is an XML file describing replacement glyphs/images for a given definition.

Root attributes:

- `Definition` — comma-separated list of compatible firmware definitions
- `Name`, `Version`, `Author`

Structure:

```xml
<ResourcePack Definition="ArcticFox" Name="Neo" Version="1.0" Author="...">
  <Description>...</Description>
  <Images>
    <Image Index="01" Width="12" Height="32">
      <Data>
.......XXX....
......XX.XX...
...
      </Data>
    </Image>
  </Images>
</ResourcePack>
```

- `TrueChar = 'X'`, `FalseChar = '.'`; `'1'` is also accepted as true.
- Rows are separated by `\r` or `\n`; empty lines are ignored.
- Each row is read left-to-right into the bitmap.

When applied, the importer looks up the same `Index` in **both** Block1 and Block2. If found, it pastes the imported glyph over the original, cropping to the original size. An optional “Resize original images” mode overwrites the original width/height metadata as well.

### Image Editor Capabilities

The upstream image editor supports these operations on `bool[,]` bitmaps:

- `Clear`, `Invert`
- `FlipHorizontal`, `FlipVertical`
- `ShiftUp`, `ShiftDown`, `ShiftLeft`, `ShiftRight` (wrap-around)
- `Rotate` (clockwise or counter-clockwise; swaps width/height)
- `PasteImage`, `ResizeImage`, `MergeImages`

UI features include a pixel grid with zoom/block size, grid toggle, left/right mouse editing, line drawing with Ctrl/Shift, and an undo/redo stack up to 128 levels.

Import/export formats:

- Import: BMP/PNG/JPG via threshold or Floyd-Steinberg dithering; TTF/OTF font rendering into glyph slots.
- Export: BMP, raw `.bin`, `.s` assembly resource file, `.respack`.

### How Images Are Used on Device

Image indices are numeric and firmware-specific; definitions do not carry human-readable names. Known consumers from the ArcticFox configuration model include:

- Startup logo (`UIConfiguration.IsLogoEnabled`, `ShowLogoDelay`)
- Screensaver / screen protection timeout (`ScreenProtectionTime`)
- Charge screen (`ChargeExtraType` can be set to `Logo`)
- Main screen skins (Classic, Circle, Foxy, Lite)
- Clock type (analog/digital)
- Strings composed as sequences of glyph indices

Because image usage is hard-coded by index in the firmware, the editor must operate on numeric indices and let the user infer semantic meaning from context or external documentation.

### Animation Planning Notes

The current firmware and editor contain **no native animation support**:

- Image blocks are flat arrays of single images.
- Resource packs replace one static image at a time.
- Patches are static byte diffs, not animation descriptors.
- ArcticFox configuration has no frame-rate, frame-count, or animation-index fields.

To add animations in a future feature, a planner would need to:

1. Reserve a contiguous or known set of image indices for animation frames.
2. Generate frames as standard Block1/Block2 images.
3. Either:
   - Replace existing glyphs at the reserved indices (keeping identical dimensions), or
   - Append new frame data and patch the image pointer table to point at it.
4. Provide a patch or custom binary edit that cycles the display through those indices on a timer.
5. Maintain tool-level metadata (outside the firmware formats) describing frame order, duration, and target index.

This is intentionally out of scope for the first Patches milestone, but the image-block decode/encode code implemented for the Images tab should be designed so it can be reused for animation frame generation later.
