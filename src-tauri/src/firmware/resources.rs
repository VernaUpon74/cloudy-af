//! Firmware image tables, string tables, and resource packs.
//!
//! The ArcticFox firmware stores its logo/preview glyphs and menu strings
//! behind indirection tables. The host tool of record — **NFE Toolbox**
//! (`NFirmwareEditor` / the `NfeFirmware` decoder shipped with it) — reads and
//! writes these through a header pointer-table indirection that this module
//! reproduces exactly. The wire format below was verified by decompiling NFE
//! and cross-checking against the real bundled `af_190602.bin` (see the unit
//! tests for the reference offsets).
//!
//! # Layout: uses handles, not paths
//!
//! Nothing here opens files. Callers hand in the decrypted firmware byte
//! buffer (held in memory as `OpenFirmware.image.bytes`) plus the *slot
//! ranges* already carried on the matched `FirmwareDefinition`. All writes
//! mutate that buffer in place; the existing Save flow persists it. See
//! `commands/firmware.rs` for the `state.with(&handle, ...)` command plumbing.
//!
//! # Why the indirection
//!
//! A single header location stores two u32 pointers (`PtrFrom`/`PtrTo`) that
//! delimit a table of more pointers. Editing a glyph therefore never shifts
//! the file layout: the pointer-slot keeps a fixed position and the record it
//! points at is overwritten in place. This is what lets firmware editors
//! tweak a logo without breaking the `0x140` marker scan used for definition
//! detection.

use std::ops::Range;

use super::{FirmwareError, Result};

/// Block-1 glyphs are stored **column-major, LSB == top row**: each byte holds
/// the 8 vertical pixels of one column, bit 0 being the topmost row.
pub const BLOCK1: u8 = 1;
/// Block-2 glyphs are stored **row-major, MSB == leftmost pixel**.
pub const BLOCK2: u8 = 2;

fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn u16le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

/// Number of pixel data bytes a `width x height` glyph occupies in `block`.
fn record_len(block: u8, width: usize, height: usize) -> usize {
    match block {
        // Block1: column-major, one byte per column, ceil(h/8) vertical bytes.
        BLOCK1 => width * ((height + 7) / 8),
        // Block2: row-major, stride bytes per row.
        _ => ((width + 7) / 8) * height,
    }
}
/// One non-empty slot in an image table.
///
/// `reference_offset` is the position of the u32 *slot* in the table (its own
/// file offset); `data_offset` is the u32 value (the raw file offset of the
/// `[width, height, ...pixels]` record it points at) — the identity mapping,
/// pointers are raw file offsets, never encrypted/relocated.
#[derive(Debug, Clone, Copy)]
pub struct ImageSlot {
    /// 1-based slot index. Empty (`0`) pointer slots are skipped but still
    /// consume an index, so indices are stable across a table.
    pub index: usize,
    pub reference_offset: usize,
    pub data_offset: usize,
    pub width: u8,
    pub height: u8,
    pub block: u8,
}

/// Parse an image table. A table is **absent** (empty, not an error) when
/// either pointer is `0` or the from pointer lies past the to pointer.
///
/// `range` comes straight off the definition (`image_table_1`/`image_table_2`).
/// `range.start` points at the `PtrFrom` slot, `range.end` at the `PtrTo` slot;
/// read the u32 at each to get the pointer values. Pointers are raw file
/// offsets (identity mapping). A table is contiguous u32 record pointers
/// scanned while the slot position `<= table_to - 4`. `0` slots are empty and
/// skipped, but still consume a 1-based index. Every non-zero reference is
/// bounds-checked (`ref + 2 + len <= bytes.len()`); out-of-bounds slots are
/// skipped (NFE does the same defensive skip).
pub fn parse_image_table(
    bytes: &[u8],
    range: Range<usize>,
    block: u8,
) -> Result<Vec<ImageSlot>> {
    // The range itself provides the header slots; both pointers must be readable.
    if range.start + 4 > bytes.len() || range.end + 4 > bytes.len() {
        return Ok(Vec::new());
    }
    let table_from = u32le(&bytes[range.start..range.start + 4]) as usize;
    let table_to = u32le(&bytes[range.end..range.end + 4]) as usize;

    // Pointer 0 => absent. from > to - 4 => no room for even one slot => absent.
    if table_from == 0 || table_to == 0 || table_from > table_to.saturating_sub(4) {
        return Ok(Vec::new());
    }

    let mut slots = Vec::new();
    let mut index = 1usize;
    let mut pos = table_from;
    while pos <= table_to - 4 {
        if pos + 4 <= bytes.len() {
            let data = u32le(&bytes[pos..pos + 4]) as usize;
            if data != 0 && data + 2 <= bytes.len() {
                let width = bytes[data] as usize;
                let height = bytes[data + 1] as usize;
                let len = record_len(block, width, height);
                if data + 2 + len <= bytes.len() {
                    slots.push(ImageSlot {
                        index,
                        reference_offset: pos,
                        data_offset: data,
                        width: width as u8,
                        height: height as u8,
                        block,
                    });
                }
                // Out-of-bounds record: skip the slot (NFE does the same).
            }
        }
        index += 1;
        pos += 4;
    }
    Ok(slots)
}

/// Read an image slot's pixels as a row-major `Vec<bool>` (`y * width + x`).
pub fn read_image_pixels(bytes: &[u8], slot: &ImageSlot) -> Result<Vec<bool>> {
    let width = slot.width as usize;
    let height = slot.height as usize;
    let data_start = slot.data_offset + 2;
    let len = record_len(slot.block, width, height);
    if data_start + len > bytes.len() {
        return Err(FirmwareError::Other(format!(
            "image slot {} record out of bounds ({} + {} > {})",
            slot.index,
            data_start,
            len,
            bytes.len()
        )));
    }
    let data = &bytes[data_start..data_start + len];
    let mut pixels = vec![false; width * height];
    for y in 0..height {
        for x in 0..width {
            pixels[y * width + x] = match slot.block {
                // Column-major: byte = (row/8)*width + col, bit = row%8.
                BLOCK1 => (data[(y / 8) * width + x] >> (y % 8)) & 1 == 1,
                // Row-major MSB-first: byte = row*stride + col/8, bit = 7-col%8.
                _ => (data[y * ((width + 7) / 8) + x / 8] >> (7 - (x % 8))) & 1 == 1,
            };
        }
    }
    Ok(pixels)
}

/// Write `pixels` (row-major, `width * height`) into the slot at `index`,
/// replacing the `[width, height, ...pixels]` record in place.
///
/// # SAFETY DIVERGENCE FROM NFE
///
/// NFE blindly memcpy's the new record over the slot, **silently clobbering the
/// next record (or past end of file)** when the new glyph grows. That is a
/// corrupt-file bug in the reference implementation. We deliberately reject the
/// write instead, refusing to extend past the next record's offset (or the
/// image end). Same-size/smaller writes are fine; larger ones error out.
pub fn write_image(
    bytes: &mut Vec<u8>,
    slots: &[ImageSlot],
    index: usize,
    width: usize,
    height: usize,
    pixels: &[bool],
) -> Result<()> {
    let slot = slots
        .iter()
        .find(|s| s.index == index)
        .ok_or_else(|| FirmwareError::Other(format!("image slot {index} not found")))?;
    if pixels.len() != width * height {
        return Err(FirmwareError::Other(format!(
            "image slot {index}: expected {} pixels for {width}x{height}, got {}",
            width * height,
            pixels.len()
        )));
    }

    let new_len = record_len(slot.block, width, height);
    // Neighbor-clobber guard: the record may extend up to the next record's
    // offset (or the image end), never into it.
    let end = bytes.len();
    let next_start = slots
        .iter()
        .filter(|s| s.data_offset > slot.data_offset)
        .map(|s| s.data_offset)
        .min()
        .unwrap_or(end);
    if slot.data_offset + 2 + new_len > next_start {
        return Err(FirmwareError::Other(format!(
            "image slot {index}: new {width}x{height} record needs {} bytes at {:#x} \
             but that would clobber the next record at {:#x} (refusing, unlike NFE's \
             silent overwrite)",
            2 + new_len,
            slot.data_offset,
            next_start
        )));
    }

    let mut record = Vec::with_capacity(2 + new_len);
    record.push(width as u8);
    record.push(height as u8);
    let mut data = vec![0u8; new_len];
    for y in 0..height {
        for x in 0..width {
            if pixels[y * width + x] {
                if slot.block == BLOCK1 {
                    data[(y / 8) * width + x] |= 1 << (y % 8);
                } else {
                    data[y * ((width + 7) / 8) + x / 8] |= 1 << (7 - (x % 8));
                }
            }
        }
    }
    record.extend_from_slice(&data);

    bytes[slot.data_offset..slot.data_offset + 2 + new_len].copy_from_slice(&record);
    Ok(())
}

/// Pack a row-major `pixels` array into 1bpp, MSB-first within each byte
/// (screen-buffer framing). Trailing bits of the final partial byte are 0.
pub fn pack_1bpp(pixels: &[bool]) -> Vec<u8> {
    let n = pixels.len();
    let mut out = vec![0u8; (n + 7) / 8];
    for (i, &bit) in pixels.iter().enumerate() {
        if bit {
            out[i / 8] |= 1 << (7 - (i % 8));
        }
    }
    out
}

/// Unpack a 1bpp, MSB-first payload into `count` row-major booleans.
pub fn unpack_1bpp(bytes: &[u8], count: usize) -> Result<Vec<bool>> {
    if count > bytes.len() * 8 {
        return Err(FirmwareError::Other(format!(
            "need {count} bits but the payload has only {} bytes",
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        out.push((bytes[i / 8] >> (7 - (i % 8))) & 1 == 1);
    }
    Ok(out)
}
/// One string in a string table.
///
/// String tables are contiguous NUL-terminated char entries with *no* index
/// table: entry boundaries are found by scanning for the terminator. Each
/// entry's `data_offset` is its raw file offset; `byte_length` includes the
/// NUL terminator (and any absorbed trailing zero padding for the last string).
#[derive(Debug, Clone, Copy)]
pub struct FwString {
    /// 1-based sequential index.
    pub index: usize,
    pub data_offset: usize,
    pub byte_length: usize,
    /// True when glyphs are LE u16 (definition `TwoBytesPerChar`), false for bytes.
    pub two_byte: bool,
}

/// Parse a string table.
///
/// Same pointer-slot indirection as image tables, but the valid `to` bound is
/// `u32le(range.end) - 4` (the `-4` excludes the u32 at the PtrTo slot). It is
/// *absent* when either pointer is `0` or `table_from > table_to`. Entries are
/// contiguous NUL-terminated char data; each ends when a `\0` char is followed
/// by a non-`\0` char (terminator counts toward the byte length). The **last**
/// string absorbs trailing zero padding up to the table end.
///
/// `char_width` comes from `FirmwareDefinition::char_width` (`1` or `2`).
pub fn parse_string_table(
    bytes: &[u8],
    range: Range<usize>,
    char_width: u8,
) -> Result<Vec<FwString>> {
    if range.start + 4 > bytes.len() || range.end + 4 > bytes.len() {
        return Ok(Vec::new());
    }
    let table_from = u32le(&bytes[range.start..range.start + 4]) as usize;
    let table_to = u32le(&bytes[range.end..range.end + 4]).saturating_sub(4) as usize;

    if table_from == 0 || table_to == 0 || table_from > table_to {
        return Ok(Vec::new());
    }

    let two_byte = char_width == 2;
    let mut strings = Vec::new();
    let mut index = 1usize;
    let mut p = table_from;
    while p <= table_to {
        let start = p;
        if two_byte {
            while p + 1 <= table_to {
                let c = u16le(&bytes[p..p + 2]);
                let at_end = p + 3 > table_to;
                if c == 0 && !at_end && u16le(&bytes[p + 2..p + 4]) != 0 {
                    p += 2;
                    break;
                }
                p += 2;
            }
        } else {
            while p <= table_to {
                let c = bytes[p];
                if c == 0 && p + 1 <= table_to && bytes[p + 1] != 0 {
                    p += 1;
                    break;
                }
                p += 1;
            }
        }
        strings.push(FwString {
            index,
            data_offset: start,
            byte_length: p - start,
            two_byte,
        });
        index += 1;
    }
    Ok(strings)
}

/// Read a string's glyph IDs, stopping before (and excluding) the NUL terminator.
pub fn read_string_glyphs(bytes: &[u8], s: &FwString) -> Vec<u16> {
    let unit = if s.two_byte { 2 } else { 1 };
    let count = s.byte_length / unit;
    let mut out = Vec::new();
    for i in 0..count {
        let pos = s.data_offset + i * unit;
        let g = if s.two_byte {
            u16le(&bytes[pos..pos + 2])
        } else {
            bytes[pos] as u16
        };
        if g == 0 {
            break;
        }
        out.push(g);
    }
    out
}

/// Write a string's glyphs in place. A string's total byte length NEVER changes
/// in this format, so the new glyphs must fit: `glyphs.len() <= byte_length /
/// unit - 1` (room for the terminator is always preserved). Shorter sequences
/// are zero-padded up to `byte_length`.
pub fn write_string_glyphs(bytes: &mut Vec<u8>, s: &FwString, glyphs: &[u16]) -> Result<()> {
    let unit = if s.two_byte { 2 } else { 1 };
    let max_chars = s.byte_length / unit; // capacity including the terminator slot
    if glyphs.len() > max_chars.saturating_sub(1) {
        return Err(FirmwareError::Other(format!(
            "string {}: {} glyphs do not fit in a {}-byte string (max {})",
            s.index,
            glyphs.len(),
            s.byte_length,
            max_chars.saturating_sub(1)
        )));
    }
    for (i, &g) in glyphs.iter().enumerate() {
        let pos = s.data_offset + i * unit;
        if s.two_byte {
            bytes[pos] = (g & 0xff) as u8;
            bytes[pos + 1] = (g >> 8) as u8;
        } else {
            bytes[pos] = (g & 0xff) as u8;
        }
    }
    // Zero padding preserves the fixed byte length (and the terminator).
    for i in glyphs.len()..max_chars {
        let pos = s.data_offset + i * unit;
        if s.two_byte {
            bytes[pos] = 0;
            bytes[pos + 1] = 0;
        } else {
            bytes[pos] = 0;
        }
    }
    Ok(())
}
/// Resource packs (`.respack`): an XML manifest of logo glyphs to paste onto
/// the firmware's image tables. This is a first-party file format (no NFE
/// analog); see the `respack` submodule.
pub mod respack {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    use super::{parse_image_table, write_image, BLOCK1, BLOCK2};
    use super::{FirmwareError, Result};
    use crate::firmware::definition::FirmwareDefinition;

    /// One glyph in a resource pack, pixels row-major (`y * width + x`).
    #[derive(Debug, Clone)]
    pub struct ResourcePackImage {
        /// 1-based image table slot index to target.
        pub index: u32,
        pub width: u8,
        pub height: u8,
        pub pixels: Vec<bool>,
    }

    /// A parsed `.respack` manifest.
    #[derive(Debug, Clone)]
    pub struct ResourcePack {
        pub definition: String,
        pub name: String,
        pub version: String,
        pub author: String,
        pub description: String,
        pub images: Vec<ResourcePackImage>,
    }

    struct ImageBuilder {
        index: u32,
        width: u8,
        height: u8,
        pixels: Vec<bool>,
    }

    fn parse_rows(text: &str, index: u32, width: u8, height: u8) -> Result<Vec<bool>> {
        let w = width as usize;
        let h = height as usize;
        let mut pixels = vec![false; w * h];
        let rows: Vec<&str> = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        for (y, row) in rows.iter().take(h).enumerate() {
            for (x, ch) in row.chars().take(w).enumerate() {
                pixels[y * w + x] = match ch {
                    'X' | '1' => true,
                    '.' | '0' => false,
                    _ => {
                        return Err(FirmwareError::Xml(format!(
                            "respack image {index}: bad pixel char {ch:?}"
                        )))
                    }
                };
            }
        }
        Ok(pixels)
    }
/// Parse a `.respack` XML manifest.
    ///
    /// Format: root `<ResourcePack Definition="ArcticFox" Name="..." Version="..."
    /// Author="...">`, optional `<Description>`, then `<Images>` of `<Image
    /// Index="01" Width="6" Height="8">` whose `<Data>` text is one row per line,
    /// `X`/`1` = set, `.`/`0` = clear. `Index` is a HEX string without `0x`.
    /// There is **no** Strings section in this format — do not invent one. A
    /// UTF-8 BOM is tolerated. Reuses the existing quick-xml dependency.
    pub fn parse_respack(xml: &str) -> Result<ResourcePack> {
        // Tolerate a leading UTF-8 BOM.
        let xml = xml.trim_start_matches('\u{feff}');

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut pack = ResourcePack {
            definition: String::new(),
            name: String::new(),
            version: String::new(),
            author: String::new(),
            description: String::new(),
            images: Vec::new(),
        };

        let mut cur: Option<ImageBuilder> = None;
        let mut in_data = false;
        let mut text_buf = String::new();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.name().as_ref() {
                    b"ResourcePack" => {
                        for attr in e.attributes() {
                            let attr = attr.map_err(|e| FirmwareError::Xml(e.to_string()))?;
                            let value = String::from_utf8_lossy(&attr.value).to_string();
                            match attr.key.as_ref() {
                                b"Definition" => pack.definition = value,
                                b"Name" => pack.name = value,
                                b"Version" => pack.version = value,
                                b"Author" => pack.author = value,
                                _ => {}
                            }
                        }
                    }
                    b"Description" => text_buf.clear(),
                    b"Image" => {
                        let mut index = 0u32;
                        let mut width = 0u8;
                        let mut height = 0u8;
                        for attr in e.attributes() {
                            let attr = attr.map_err(|e| FirmwareError::Xml(e.to_string()))?;
                            let value = String::from_utf8_lossy(&attr.value).to_string();
                            match attr.key.as_ref() {
                                b"Index" => {
                                    index = u32::from_str_radix(value.trim(), 16).map_err(|e| {
                                        FirmwareError::Xml(format!("bad Index {value:?}: {e}"))
                                    })?
                                }
                                b"Width" => {
                                    width = value.trim().parse::<u8>().map_err(|e| {
                                        FirmwareError::Xml(format!("bad Width {value:?}: {e}"))
                                    })?
                                }
                                b"Height" => {
                                    height = value.trim().parse::<u8>().map_err(|e| {
                                        FirmwareError::Xml(format!("bad Height {value:?}: {e}"))
                                    })?
                                }
                                _ => {}
                            }
                        }
                        cur = Some(ImageBuilder {
                            index,
                            width,
                            height,
                            pixels: Vec::new(),
                        });
                    }
                    b"Data" => {
                        in_data = true;
                        text_buf.clear();
                    }
                    _ => {}
                },
                Ok(Event::Text(e)) => {
                    let t = e.html_content().unwrap_or_default().to_string();
                    text_buf.push_str(&t);
                }
                Ok(Event::End(e)) => match e.name().as_ref() {
                    b"Data" => {
                        in_data = false;
                        if let Some(builder) = cur.as_mut() {
                            let pixels =
                                parse_rows(&text_buf, builder.index, builder.width, builder.height)?;
                            builder.pixels = pixels;
                        }
                    }
                    b"Image" => {
                        if let Some(builder) = cur.take() {
                            pack.images.push(ResourcePackImage {
                                index: builder.index,
                                width: builder.width,
                                height: builder.height,
                                pixels: builder.pixels,
                            });
                        }
                    }
                    b"Description" => pack.description = text_buf.trim().to_string(),
                    _ => {}
                },
                Ok(Event::Eof) => break,
                Err(e) => return Err(FirmwareError::Xml(e.to_string())),
                _ => {}
            }
            buf.clear();
        }

        Ok(pack)
    }
/// Apply a resource pack to an opened firmware image, mutating `bytes` in
    /// place. For each pack image the matching 1-based index slot is found in
    /// Block1, else Block2; if found, the pack glyph is paste-cropped onto a
    /// canvas of the *slot's* dimensions (copy `min(pack_w,slot_w) x
    /// min(pack_h,slot_h)` from the top-left, rest clear) and written. Missing
    /// indices are silently skipped. Returns `(applied, skipped)`.
    pub fn apply_respack(
        bytes: &mut Vec<u8>,
        pack: &ResourcePack,
        definition: &FirmwareDefinition,
    ) -> Result<(usize, usize)> {
        let slots1 = parse_image_table(bytes, definition.image_table_1.clone(), BLOCK1)?;
        let slots2 = parse_image_table(bytes, definition.image_table_2.clone(), BLOCK2)?;

        let mut applied = 0usize;
        let mut skipped = 0usize;

        for img in &pack.images {
            let idx = img.index as usize;
            let (slot, table) = if let Some(s) = slots1.iter().find(|s| s.index == idx) {
                (s, slots1.as_ref())
            } else if let Some(s) = slots2.iter().find(|s| s.index == idx) {
                (s, slots2.as_ref())
            } else {
                skipped += 1;
                continue;
            };

            let cw = slot.width as usize;
            let ch = slot.height as usize;
            let mut canvas = vec![false; cw * ch];
            let copy_w = (img.width as usize).min(cw);
            let copy_h = (img.height as usize).min(ch);
            for y in 0..copy_h {
                for x in 0..copy_w {
                    canvas[y * cw + x] = img.pixels[y * (img.width as usize) + x];
                }
            }
            // The write replaces the record at the SLOT's own dimensions, so
            // the neighbor-clobber guard (unlike NFE) stays satisfied.
            write_image(bytes, table, idx, cw, ch, &canvas)?;
            applied += 1;
        }

        Ok((applied, skipped))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware::definition::{parse_definition, FirmwareDefinition};
    use crate::firmware::resources::respack::{
        apply_respack, parse_respack, ResourcePack, ResourcePackImage,
    };
    use std::path::Path;

    fn resources() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("resources")
    }

    fn af190602() -> (Vec<u8>, FirmwareDefinition) {
        // Parse the SHIPPED definition so the table ranges match the real file
        // exactly (an inline copy once masked a definition mismatch in tests).
        let xml =
            std::fs::read_to_string(resources().join("definitions/ArcticFox.xml")).unwrap();
        let defs = parse_definition(&xml).unwrap();
        let def = defs
            .into_iter()
            .find(|d| d.image_table_1.start != 0)
            .expect("ArcticFox definition with an image table");
        let bytes = std::fs::read(resources().join("firmware/decrypted/af_190602.bin")).unwrap();
        (bytes, def)
    }

    #[test]
    fn test_af190602_block1_has_210_slots_with_reference_offsets() {
        let (bytes, def) = af190602();
        let slots = parse_image_table(&bytes, def.image_table_1, BLOCK1).unwrap();
        assert_eq!(slots.len(), 210, "af_190602 must have exactly 210 Block1 slots");

        let table_from = u32le(&bytes[0x144..0x148]) as usize;
        assert_eq!(table_from, 0x1AA80);
        assert_eq!(slots[0].reference_offset, table_from);
        for (i, s) in slots.iter().enumerate() {
            assert_eq!((s.index, s.reference_offset), (i + 1, table_from + 4 * i));
        }
        // Tight packing: ref[i+1] == ref[i] + 2 + len.
        for w in slots.windows(2) {
            let len = record_len(BLOCK1, w[0].width as usize, w[0].height as usize);
            assert_eq!(w[1].data_offset, w[0].data_offset + 2 + len);
        }
    }

    #[test]
    fn test_af190602_slot1_decodes_zero_glyph() {
        let (bytes, def) = af190602();
        let slots = parse_image_table(&bytes, def.image_table_1, BLOCK1).unwrap();
        let slot = &slots[0];
        assert_eq!((slot.width, slot.height), (6, 8));
        assert_eq!(
            &bytes[slot.data_offset..slot.data_offset + 8],
            &[0x06, 0x08, 0x7c, 0xfe, 0x82, 0xfe, 0x7c, 0x00]
        );

        let pixels = read_image_pixels(&bytes, slot).unwrap();
        assert_eq!(pixels.len(), 48);
        // Left vertical bar (col 0 = 0x7c): lit rows 2..6.
        assert!(!pixels[0 * 6 + 0] && !pixels[1 * 6 + 0]);
        assert!(pixels[2 * 6 + 0] && pixels[3 * 6 + 0] && pixels[6 * 6 + 0]);
        assert!(!pixels[7 * 6 + 0]);
        // Col 5 (0x00) fully clear.
        assert!((0..8).all(|r| !pixels[r * 6 + 5]));

        // encode(decode(slot)) is a byte no-op for the record region.
        let mut buf = bytes.clone();
        write_image(&mut buf, &slots, slot.index, 6, 8, &pixels).unwrap();
        assert_eq!(
            &buf[slot.data_offset..slot.data_offset + 8],
            &bytes[slot.data_offset..slot.data_offset + 8]
        );
    }
#[test]
    fn test_encode_decode_identity_both_packings() {
        for block in [BLOCK1, BLOCK2] {
            for (w, h) in [(6usize, 8usize), (7usize, 5usize), (9usize, 3usize)] {
                let data_offset = 64usize;
                let slot = ImageSlot {
                    index: 1,
                    reference_offset: 0,
                    data_offset,
                    width: w as u8,
                    height: h as u8,
                    block,
                };
                let mut bytes = vec![0u8; data_offset + 2 + record_len(block, w, h) + 16];
                let pixels: Vec<bool> = (0..h)
                    .flat_map(|y| (0..w).map(move |x| (x + y) % 3 != 0 || x == 0))
                    .collect();
                write_image(&mut bytes, &[slot], 1, w, h, &pixels).unwrap();
                let read = read_image_pixels(&bytes, &slot).unwrap();
                assert_eq!(read, pixels, "roundtrip failed for block {block} {w}x{h}");
            }
        }
    }

    #[test]
    fn test_oversized_write_image_rejected() {
        let block = BLOCK1;
        let len = record_len(block, 6, 8); // 6
        let s1 = ImageSlot { index: 1, reference_offset: 0, data_offset: 16, width: 6, height: 8, block };
        let s2 = ImageSlot { index: 2, reference_offset: 0, data_offset: 16 + 2 + len, width: 6, height: 8, block };
        let mut bytes = vec![0u8; s2.data_offset + 2 + len];
        let zero: Vec<bool> = vec![false; 6 * 8];
        write_image(&mut bytes, &[s1, s2], 1, 6, 8, &zero).unwrap();

        let bigger: Vec<bool> = vec![true; 6 * 16]; // 6x16 => len 12 > 6
        let r = write_image(&mut bytes, &[s1, s2], 1, 6, 16, &bigger);
        assert!(r.is_err(), "growing a record into the next slot must be rejected");
    }

    #[test]
    fn test_af190602_strings_164_and_glyph_offsets() {
        let (bytes, def) = af190602();
        let strings =
            parse_string_table(&bytes, def.string_table_1.clone(), def.char_width).unwrap();
        assert_eq!(strings.len(), 164, "af_190602 must have exactly 164 strings");

        let s1 = &strings[0];
        assert_eq!((s1.index, s1.data_offset, s1.byte_length), (1, 0x1BAD4, 9));
        assert!(!s1.two_byte);
        let glyphs = read_string_glyphs(&bytes, s1);
        assert_eq!(glyphs, vec![0x33, 0x61, 0x60, 0x5B, 0x52, 0x55, 0x5E, 0x51]);

        // The last string absorbs trailing zero padding.
        let last = strings.last().unwrap();
        assert_eq!((last.index, last.data_offset, last.byte_length), (164, 0x1BEF7, 8));
        assert_eq!(
            &bytes[last.data_offset..last.data_offset + 8],
            &[0x40, 0x55, 0x4F, 0x57, 0x51, 0x58, 0x00, 0x00]
        );
    }
#[test]
    fn test_write_string_identity_and_zero_pad() {
        let (bytes, def) = af190602();
        let strings =
            parse_string_table(&bytes, def.string_table_1.clone(), def.char_width).unwrap();
        let s1 = &strings[0];
        let glyphs = read_string_glyphs(&bytes, s1);

        // Identical glyphs: byte no-op.
        let mut buf = bytes.clone();
        write_string_glyphs(&mut buf, s1, &glyphs).unwrap();
        assert_eq!(buf, bytes, "writing identical glyphs must be a byte no-op");

        // Shorter sequence zero-pads with byte length preserved.
        let mut buf2 = bytes.clone();
        write_string_glyphs(&mut buf2, s1, &[0x33]).unwrap();
        assert_eq!(buf2[s1.data_offset], 0x33);
        assert_eq!(buf2.len(), bytes.len());
        assert!(
            buf2[s1.data_offset + 1..s1.data_offset + s1.byte_length]
                .iter()
                .all(|&b| b == 0),
            "padding (including terminator) must be zeroed"
        );
        assert_eq!(&buf2[..s1.data_offset], &bytes[..s1.data_offset]);
        assert_eq!(
            &buf2[s1.data_offset + s1.byte_length..],
            &bytes[s1.data_offset + s1.byte_length..]
        );

        // Too many glyphs: rejected.
        let too_many: Vec<u16> = vec![0x41; 9];
        assert!(write_string_glyphs(&mut buf2, s1, &too_many).is_err());
    }

    #[test]
    fn test_parse_respack_inline_sample() {
        let xml = concat!(
            "\u{feff}",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<ResourcePack Definition="ArcticFox" Name="TestPack" Version="1.0" Author="AF Team">
  <Description>Just a test</Description>
  <Images>
    <Image Index="01" Width="6" Height="8">
      <Data>
XXXXXX
......
XXXXXX
XXXXXX
......
XXXXXX
XXXXXX
......
      </Data>
    </Image>
  </Images>
</ResourcePack>"#
        );
        let pack = parse_respack(xml).unwrap();
        assert_eq!(pack.definition, "ArcticFox");
        assert_eq!(pack.name, "TestPack");
        assert_eq!(pack.version, "1.0");
        assert_eq!(pack.author, "AF Team");
        assert_eq!(pack.description, "Just a test");
        assert_eq!(pack.images.len(), 1);
        let im = &pack.images[0];
        assert_eq!((im.index, im.width, im.height), (1, 6, 8));
        assert_eq!(im.pixels.len(), 48);
        assert!(im.pixels[0]);   // row0 col0 = X
        assert!(!im.pixels[6]);  // row1 is "......" -> col0 clear
        assert!(im.pixels[12]);  // row2 col0 = X
    }
#[test]
    fn test_apply_respack_on_af190602() {
        let (bytes0, def) = af190602();
        let slots = parse_image_table(&bytes0, def.image_table_1.clone(), BLOCK1).unwrap();
        let target = slots.iter().find(|s| s.index == 1).unwrap();

        let all_lit: Vec<bool> = vec![true; 6 * 8];
        let pack = ResourcePack {
            definition: "ArcticFox".into(),
            name: "Test".into(),
            version: "1".into(),
            author: "a".into(),
            description: String::new(),
            images: vec![
                ResourcePackImage { index: 1, width: 6, height: 8, pixels: all_lit },
                ResourcePackImage {
                    index: 999,
                    width: 6,
                    height: 8,
                    pixels: vec![false; 48],
                },
            ],
        };

        let mut bytes = bytes0.clone();
        let (applied, skipped) = apply_respack(&mut bytes, &pack, &def).unwrap();
        assert_eq!((applied, skipped), (1, 1));

        // Only the target record bytes may differ.
        for i in 0..bytes.len() {
            if i >= target.data_offset && i < target.data_offset + 8 {
                continue;
            }
            assert_eq!(bytes[i], bytes0[i], "byte {i} outside the target record must be unchanged");
        }
        // All-lit 6x8 => every column byte has all 8 bits set.
        assert_eq!(
            &bytes[target.data_offset..target.data_offset + 8],
            &[0x06, 0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
        );
    }
}