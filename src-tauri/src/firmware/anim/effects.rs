//! Animation effects: pure Rust references + Thumb-1 cave emission.
//!
//! A family of "fade" effects for the timed-out screen: a 16-entry sine LUT
//! maps each framebuffer byte's band index to a brightness; bytes below the
//! half-amplitude threshold are cleared in place. Integer math only.
//!
//! Buffer geometry: 64x128 1bpp, HORIZONTALLY packed, MSB = leftmost pixel
//! (`byte = y*8 + x/8`, `bit = 0x80 >> (x%8)`) — the layout NToolbox copies
//! the 0xC1 screenshot bytes into a GDI+ Format1bppIndexed bitmap verbatim,
//! confirmed on hardware (Pico 25 capture renders readable text only under
//! this packing; see resources/re/af_190602-render.md §"Packing correction").
//! So for byte `i`: row `y = i >> 3`, byte-column `bc = i & 7` (pixels
//! `bc*8 .. bc*8+7`). A 64-byte group `i >> 6` spans 8 full rows — the same
//! 8-row band the old vertical-packing model called a "strip", which is why
//! Gradient Fade and Center Pulse (band-indexed by `i >> 6`) accidentally
//! rendered correctly on hardware while Diagonal Sweep (indexed by byte
//! column) did not.
//!
//! Injection (see `resources/re/af_190602-dispatcher-analy.md` §7): a 4-byte
//! B.W at the timeout decision point `0x9a16` (in `FUN_00009a10`) detours
//! into the APROM-tail code cave. The cave replays the overwritten
//! `tst.w r2, #0x20000`; when the timeout bit is clear (or the config byte
//! selects no effect) it resumes the stock path; when timed out it bumps the
//! phase global, fades the display buffer in place, and resumes.
//!
//! The config byte (a spare dataflash byte) selects the effect:
//! 2 = Gradient Fade, 3 = Center Pulse, 4 = Diagonal Sweep.
//! See `CONFIG_*` constants below for the full assigned value list.

use serde::Deserialize;

use crate::firmware::anim::asm::{bw_pair, AnimError, Asm, AsmError};
use crate::firmware::emu::harness::de_hex;
use crate::firmware::patch::{Patch, PatchModification};

/// Stock bytes at the af_190602 hook site: `tst.w r2, #0x20000`
/// (`resources/re/af_190602-dispatcher-analy.md` §7.4). An image without
/// these 4 bytes at `hook_site` is a different build (or already detoured)
/// and must not receive an animation patch.
pub const HOOK_SITE_STOCK_BYTES: [u8; 4] = [0x12, 0xf4, 0x00, 0x3f];

/// Parse and validate the descriptor's `"animation"` block. Shared by the
/// effect builders and the firmware-editor gate (`image_supports_animation`).
pub fn load_animation_desc(desc_json: &str) -> Result<AnimationDesc, AnimError> {
    let v: serde_json::Value =
        serde_json::from_str(desc_json).map_err(|e| AnimError::Descriptor(e.to_string()))?;
    let anim: AnimationDesc = serde_json::from_value(v["animation"].clone())
        .map_err(|e| AnimError::Descriptor(format!("animation block: {e}")))?;
    if anim.status_base_addr + 4 != anim.status_word_addr {
        return Err(AnimError::Descriptor(format!(
            "status_base_addr {:#x} must be status_word_addr {:#x} - 4",
            anim.status_base_addr, anim.status_word_addr
        )));
    }
    Ok(anim)
}

/// Gate for offering animation patches on an opened image: the hook site must
/// still hold the stock timeout test (right build, not already patched) and
/// the code cave must be erased flash (all 0xFF), so the detour can never
/// clobber live code. A cave region past the image end counts as erased —
/// `apply_patch` grows the image into it.
pub fn image_supports_animation(image: &[u8], anim: &AnimationDesc) -> bool {
    let hook = anim.hook_site as usize;
    if image.len() < hook + HOOK_SITE_STOCK_BYTES.len()
        || image[hook..hook + HOOK_SITE_STOCK_BYTES.len()] != HOOK_SITE_STOCK_BYTES
    {
        return false;
    }
    let cave = anim.code_cave.start as usize;
    let cave_end = cave.saturating_add(anim.code_cave.size).min(image.len());
    image
        .get(cave..cave_end)
        .map(|region| region.iter().all(|b| *b == 0xFF))
        .unwrap_or(true)
}

/// 16-entry quarter-wave sine LUT, values 0..=255.
pub const SINE_LUT: [u8; 16] = [0, 25, 50, 74, 98, 120, 142, 162, 180, 197, 212, 225, 236, 245, 251, 255];

/// Half-amplitude threshold: bytes in bands darker than this are cleared.
const THRESHOLD: u8 = 128;

/// Config-byte values selecting each effect.
/// 
/// These values are persisted in a spare dataflash byte (`config_byte_addr` in
/// the animation descriptor) and read by the injected cave at runtime. The
/// existing charge-screen effects share the timeout decision point hook site
/// (`0x9a16` in `FUN_00009a10`) documented in `resources/re/af_190602-dispatcher-analy.md` §7.
/// 
/// Value assignments (do not reorder — frontend dropdown options and existing
/// patches depend on these):
/// - 0: Off (stock behavior, no animation)
/// - 1: Swirl (reserved — not yet implemented in this codebase)
/// - 2: Gradient Fade (charge-screen timeout animation)
/// - 3: Center Pulse (charge-screen timeout animation)
/// - 4: Diagonal Sweep (charge-screen timeout animation)
/// - 5: Rippling Wave (Phase 4 addition — charge-screen timeout animation)
/// - 6: Clock Animation (PENDING — requires separate RE confirmation for clock-side
///   injection seam; see `resources/re/af_190602-dispatcher-analy.md` §7.3. The
///   clock hook is NOT the same as the charge-screen timeout hook; the clock is drawn
///   when the timeout bit is CLEAR at `0x9a32`, a distinct pathway from the timeout
///   branch at `0x9a20`. Clock animation injection requires identifying a hook site
///   in the clock renderer (`FUN_00009a10` → clock drawing path via `0x136b4`,
///   `0x90`, `0xb230`, `0x13760`, `0x13798`) — addresses from RE notes, confirm
///   against `af_190602.dec.bin` before flash.)
pub const CONFIG_GRADIENT_FADE: u8 = 2;
pub const CONFIG_CENTER_PULSE: u8 = 3;
pub const CONFIG_DIAGONAL_SWEEP: u8 = 4;
pub const CONFIG_RIPPLING_WAVE: u8 = 5;
pub const CONFIG_CLOCK_ANIMATION: u8 = 6;

/// The descriptor's `"animation"` object.
#[derive(Debug, Deserialize)]
pub struct AnimationDesc {
    #[serde(deserialize_with = "de_hex")]
    pub hook_site: u32,
    #[serde(deserialize_with = "de_hex")]
    pub hook_resume: u32,
    pub code_cave: CodeCave,
    #[serde(deserialize_with = "de_hex")]
    pub config_byte_addr: u32,
    #[serde(deserialize_with = "de_hex")]
    pub phase_global: u32,
    /// Address of the device-status word the hook-site instruction tests
    /// (`tst.w r2, #0x20000` reads `[status_base + 4]`). The cave re-reads it
    /// to restore r2 and the condition flags before resuming the stock path,
    /// whose `beq` at `hook_resume + 4` depends on the tst result.
    #[serde(deserialize_with = "de_hex")]
    pub status_word_addr: u32,
    /// Address of the status BASE register the hook loads the word through
    /// (`ldr r3, [pc, #lit]` at `hook_site - 4`, giving `r3 = status_base`;
    /// `hook_resume + 6` re-reads `[status_base + 4]`). MUST equal
    /// `status_word_addr - 4` (validated in `load_descriptor`). The cave
    /// restores r3 to this value on exit so the stock timeout branch tests
    /// the real status word.
    #[serde(deserialize_with = "de_hex")]
    pub status_base_addr: u32,
}

#[derive(Debug, Deserialize)]
pub struct CodeCave {
    #[serde(deserialize_with = "de_hex")]
    pub start: u32,
    pub size: usize,
}

/// Pure reference for the whole fade family: `index` maps byte offset `i`
/// and `phase` to a LUT entry; bytes whose band brightness is below
/// `THRESHOLD` are cleared in place.
pub fn fade_apply(buf: &mut [u8], width: usize, phase: u32, index: impl Fn(usize, u32) -> usize) {
    for i in 0..buf.len() {
        if SINE_LUT[index(i, phase) & 15] < THRESHOLD {
            buf[i] = 0;
        }
    }
}

/// Gradient Fade (config 2): horizontal brightness bands sweep vertically.
///
/// Each 8-row band (64-byte group `i / width`) shares one brightness value:
/// `SINE_LUT[((band*8 + phase) >> 2) & 15]`; the `>> 2` keeps the
/// phase→band mapping expressible in Thumb shifts (period 64 phases).
pub fn gradient_fade_apply(buf: &mut [u8], width: usize, _height: usize, phase: u32) {
    fade_apply(buf, width, phase, |i, ph| {
        (((i / width) as u32 * 8 + ph) >> 2) as usize
    })
}

/// Center Pulse (config 3): brightness rings pulse outward from the vertical
/// centre of the screen. 8-row-band distance from the centre is `band ^ 7`
/// (bands 7/8 → 0, edges → 15); period 16 phases.
pub fn center_pulse_apply(buf: &mut [u8], width: usize, phase: u32) {
    fade_apply(buf, width, phase, |i, ph| {
        let d = (i / width) as u32 ^ 7;
        d.wrapping_add(ph) as usize
    })
}

/// Diagonal Sweep (config 4): diagonal bands march corner to corner. The LUT
/// index combines the 8-row band `s = i >> 6`, the byte's column group
/// `xg = (i & 7) << 1` (8 byte-columns of 8 px = 16 half-byte groups), and
/// the phase; period 16 phases. (`xg` was `(i & 63) >> 2` under the old
/// vertical-packing model — on the real horizontally-packed buffer that
/// mixed rows and columns and rendered streaky blocks instead of diagonals.)
pub fn diagonal_sweep_apply(buf: &mut [u8], _width: usize, phase: u32) {
    fade_apply(buf, 64, phase, |i, ph| {
        let xg = ((i & 7) << 1) as u32;
        let s = (i >> 6) as u32;
        s.wrapping_add(xg).wrapping_add(ph) as usize
    })
}

/// Emitter signature shared by all effects: append the cave body to `a`
/// (based at `anim.code_cave.start`) for `buf_addr..buf_addr+buf_len`.
pub type EmitFn = fn(&mut Asm, &AnimationDesc, u32, u32) -> Result<(), AsmError>;

/// Emits the shared cave body into `a`: replay the overwritten timeout test,
/// gate on the config byte, bump the phase global, run the fade loop whose
/// LUT index computation is supplied by `emit_index`, then branch back to
/// `hook_resume`.
///
/// Register contract inside the loop: r0 = i, r1 = buf, r4 = phase,
/// r5 = LUT pointer, r6 = 15 (index mask), r7 = bytes remaining; `emit_index`
/// may clobber r2/r3 and must leave the LUT index (0..=15) in r3.
fn emit_effect_cave(
    a: &mut Asm,
    anim: &AnimationDesc,
    buf_addr: u32,
    buf_len: u32,
    config_value: u8,
    emit_index: impl Fn(&mut Asm),
) -> Result<(), AsmError> {
    // tst.w r2, #0x20000 — the overwritten hook-site instruction, replayed
    // (Capstone-verified bytes from af_190602 0x9a16).
    a.raw32(0xF412, 0x3F00);
    a.bcond(0x0, "resume"); // EQ: timeout bit clear -> stock clock path
    a.ldr_lit(0, "cfg");
    a.ldrb_imm(0, 0, 0);
    a.cmp(0, config_value);
    a.bcond(0x1, "resume"); // NE: not our effect -> stock path
    a.ldr_lit(0, "phg");
    a.ldr_imm(4, 0, 0);
    a.adds(4, 1);
    a.str_imm(4, 0, 0);
    a.movs(0, 0); // i = 0
    a.ldr_lit(1, "buf");
    a.ldr_lit(5, "lutp");
    a.ldr_lit(7, "len");
    a.movs(6, 15);
    a.label("loop");
    emit_index(a);
    a.ls_reg(6, 2, 5, 3); // ldrb r2, [r5, r3]  (LUT)
    a.cmp(2, THRESHOLD);
    a.bcond(0xA, "keep"); // GE: bright enough -> keep byte
    a.movs(2, 0);
    a.ls_reg(2, 2, 1, 0); // strb r2, [r1, r0]
    a.label("keep");
    a.adds(0, 1);
    a.subs(7, 1);
    a.bcond(0x1, "loop"); // NE
    // Restore the hook-site register/flag state the stock path relies on:
    // the fade loop clobbered r2/r3 and left Z=1 (r7 hit 0), which would
    // wrongly divert the stock `beq` after hook_resume. Re-read the status
    // word, restore r3 to the status BASE the hook literal holds (stock
    // re-reads [base + 4] in its timeout branch), and replay the overwritten
    // tst.w so the resume behaves exactly like unpatched code.
    a.ldr_lit(3, "stw");
    a.ldr_imm(2, 3, 0); // r2 = [status_word]
    a.ldr_lit(3, "stb"); // r3 = status_base (stock register at hook_resume)
    a.raw32(0xF412, 0x3F00); // tst.w r2, #0x20000
    a.label("resume");
    a.b_abs(anim.hook_resume);

    a.pool_word("cfg", anim.config_byte_addr);
    a.pool_word("phg", anim.phase_global);
    a.pool_word("buf", buf_addr);
    a.pool_word("len", buf_len);
    a.pool_word("stw", anim.status_word_addr);
    a.pool_word("stb", anim.status_base_addr);
    // "lutp" holds the ADDRESS of the 16-byte LUT block pooled as four
    // consecutive words; the "lut" label keeps the FIRST word's address.
    a.pool_addr("lutp", "lut");
    for w in SINE_LUT.chunks(4) {
        a.pool_word("lut", u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
    }
    Ok(())
}

pub fn emit_gradient_fade(
    a: &mut Asm,
    anim: &AnimationDesc,
    buf_addr: u32,
    buf_len: u32,
) -> Result<(), AsmError> {
    emit_effect_cave(a, anim, buf_addr, buf_len, CONFIG_GRADIENT_FADE, |a| {
        a.lsrs(3, 0, 6); // i >> 6        (8-row band = 64 bytes)
        a.lsls(3, 3, 3); // * 8           (y of the band's first row)
        a.adds_reg(3, 3, 4); // + phase
        a.lsrs(3, 3, 2); // >> 2          (period 64 phases)
        a.ands(3, 6); // & 15
    })
}

pub fn emit_center_pulse(
    a: &mut Asm,
    anim: &AnimationDesc,
    buf_addr: u32,
    buf_len: u32,
) -> Result<(), AsmError> {
    emit_effect_cave(a, anim, buf_addr, buf_len, CONFIG_CENTER_PULSE, |a| {
        a.lsrs(3, 0, 6); // 8-row band s = i >> 6
        a.movs(2, 7); // centre constant (r2 is the byte temp, free here)
        a.eors(3, 2); // d = s ^ 7   (distance from vertical centre)
        a.adds_reg(3, 3, 4); // + phase   (period 16)
        a.ands(3, 6); // & 15
    })
}

pub fn emit_diagonal_sweep(
    a: &mut Asm,
    anim: &AnimationDesc,
    buf_addr: u32,
    buf_len: u32,
) -> Result<(), AsmError> {
    emit_effect_cave(a, anim, buf_addr, buf_len, CONFIG_DIAGONAL_SWEEP, |a| {
        a.lsls(3, 0, 29); // i << 29
        a.lsrs(3, 3, 28); // >> 28       (column group = (i & 7) << 1)
        a.lsrs(2, 0, 6); // 8-row band s = i >> 6
        a.adds_reg(3, 3, 2); // band + column group
        a.adds_reg(3, 3, 4); // + phase   (period 16)
        a.ands(3, 6); // & 15
    })
}

// ============================================================================
// Clock animation hook — DEFERRED (RE-pending)
// ============================================================================
// The clock animation requires a separate hook site from the charge-screen
// timeout effects. Per `resources/re/af_190602-dispatcher-analy.md` §7:
//
// - The timeout decision point at `0x9a16` (tst.w r2, #0x20000) branches to
//   `0x9a20` (timeout/dim path) when the bit is SET, or `0x9a32` (clock path)
//   when the bit is CLEAR.
// - The clock path at `0x9a32` draws the clock via `0x136b4`, `0x90`, `0xb230`,
//   `0x13760`, `0x13798`.
//
// The existing charge-screen effects (CONFIG_GRADIENT_FADE through
// CONFIG_RIPPLING_WAVE) use the `0x9a16` hook site and run when the timeout
// bit is SET. A clock animation would need to intercept the clock drawing path
// (when timeout bit is CLEAR), which is a distinct injection seam.
//
// TODO: Identify a clock-side hook site in the clock renderer path.
// Candidate approaches (need RE confirmation against af_190602.dec.bin):
// 1. Hook into the clock drawing function(s) at `0x136b4`/`0x13760`/`0x13798`
// 2. Hook the dispatcher's clock-path branch at `0x9a32`
// 3. Use the post-render commit selector at `0x13bf4` (§7.3 site C) with a
//    clock-specific mode byte
//
// Until RE confirms a clock hook site, CONFIG_CLOCK_ANIMATION (6) is reserved
// but no emitter/patch builder is provided. The existing charge-screen cave at
// `0x9a16` already handles the "show-clock-on-timeout" pathway: when the
// timeout bit is clear, the cave branches to the stock clock path at `0x9a32`.
// ============================================================================

/// Builds the full patch: 4-byte B.W detour at `hook_site` + cave body at
/// `code_cave.start`. `desc_json` is the full descriptor document.
fn build_effect_patch(
    desc_json: &str,
    id: &str,
    name: &str,
    description: &str,
    effect_key: &str,
    emit: EmitFn,
) -> Result<Patch, AnimError> {
    let anim = load_animation_desc(desc_json)?;
    let v: serde_json::Value =
        serde_json::from_str(desc_json).map_err(|e| AnimError::Descriptor(e.to_string()))?;
    let effect = v["animation"]["effects"][effect_key]
        .as_u64()
        .ok_or_else(|| AnimError::Descriptor(format!("animation.effects.{effect_key} missing")))?
        as u8;
    let hex = |key: &str| -> Result<u32, AnimError> {
        let s = v["display_buffer"][key]
            .as_str()
            .ok_or_else(|| AnimError::Descriptor(format!("display_buffer.{key} missing")))?;
        let d = s.strip_prefix("0x").unwrap_or(s);
        u32::from_str_radix(d, if s.starts_with("0x") { 16 } else { 10 })
            .map_err(|e| AnimError::Descriptor(format!("display_buffer.{key}: {e}")))
    };
    let buf_addr = hex("start")?;
    let buf_len = hex("end")? - buf_addr;

    let mut asm = Asm::new(anim.code_cave.start);
    emit(&mut asm, &anim, buf_addr, buf_len)?;
    let body = asm.finish()?;
    if body.len() > anim.code_cave.size {
        return Err(AnimError::CaveOverflow {
            body: body.len(),
            cave: anim.code_cave.size,
        });
    }

    let mut modifications = Vec::with_capacity(4 + body.len());
    let [h1, h2] = bw_pair(anim.hook_site, anim.code_cave.start);
    for (i, hw) in [h1, h2].iter().enumerate() {
        modifications.push(PatchModification {
            offset: anim.hook_site as usize + i * 2,
            original: None,
            patched: (hw & 0xFF) as u8,
        });
        modifications.push(PatchModification {
            offset: anim.hook_site as usize + i * 2 + 1,
            original: None,
            patched: (hw >> 8) as u8,
        });
    }
    for (i, b) in body.iter().enumerate() {
        modifications.push(PatchModification {
            offset: anim.code_cave.start as usize + i,
            original: None,
            patched: *b,
        });
    }
    // The config byte selects the effect at runtime (the cave loads it from
    // `config_byte_addr`). It lives beyond the stock image in erased APROM,
    // so this modification grows the image to cover it — without it a
    // flashed animation would stay gated off (erased 0xFF = no effect).
    modifications.push(PatchModification {
        offset: anim.config_byte_addr as usize,
        original: None,
        patched: effect,
    });

    Ok(Patch {
        id: id.into(),
        name: name.into(),
        version: "1.0".into(),
        author: "cloudy-af".into(),
        description: description.into(),
        modifications,
        applied: false,
    })
}

pub fn build_gradient_fade_patch(desc_json: &str) -> Result<Patch, AnimError> {
    build_effect_patch(
        desc_json,
        "anim-gradient-fade",
        "Gradient Fade (charge-screen animation)",
        "CRT-style gradient fade on screen timeout (config byte 2)",
        "gradient_fade",
        emit_gradient_fade,
    )
}

pub fn build_center_pulse_patch(desc_json: &str) -> Result<Patch, AnimError> {
    build_effect_patch(
        desc_json,
        "anim-center-pulse",
        "Center Pulse (charge-screen animation)",
        "Brightness rings pulse outward from screen centre on timeout (config byte 3)",
        "center_pulse",
        emit_center_pulse,
    )
}

pub fn build_diagonal_sweep_patch(desc_json: &str) -> Result<Patch, AnimError> {
    build_effect_patch(
        desc_json,
        "anim-diagonal-sweep",
        "Diagonal Sweep (charge-screen animation)",
        "Diagonal bands sweep the screen on timeout (config byte 4)",
        "diagonal_sweep",
        emit_diagonal_sweep,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundled descriptor must accept the bundled real af_190602 image —
    /// this is exactly the "Animations not available under Patches" gate.
    /// Cwd for `cargo test` is the crate dir (src-tauri/).
    #[test]
    fn bundled_descriptor_accepts_bundled_af190602_image() {
        let desc_json = std::fs::read_to_string("../resources/animations/af_190602.json")
            .expect("read bundled descriptor");
        let anim = load_animation_desc(&desc_json).expect("parse bundled descriptor");
        let image = std::fs::read("../resources/firmware/decrypted/af_190602.bin")
            .expect("read bundled decrypted af_190602 image");
        assert!(
            image_supports_animation(&image, &anim),
            "bundled af_190602 image must pass the animation gate (hook site {:#x})",
            anim.hook_site
        );
    }

    /// Measure exactly what applying the bundled gradient-fade patch does to
    /// the bundled stock af_190602 image, mirroring the wiring in
    /// `commands/firmware.rs::apply_patch_cmd` (rollback_other_animations on a
    /// single-patch list is a no-op, then `apply_patch`). The patch writes
    /// the hook-site branch, the cave body, and the config byte that enables
    /// the effect at `config_byte_addr` — so the image grows past the stock
    /// end to cover it. With `WRITE_PATCHED=1` the test also writes the
    /// fully-applied image to /tmp/anim_patched.bin for hardware flash
    /// experiments; the default run stays side-effect free.
    #[test]
    fn gradient_fade_patch_on_bundled_af190602_measures_image_effect() {
        use crate::firmware::patch::{apply_patch, rollback_other_animations};

        let desc_json = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../resources/animations/af_190602.json"
        ))
        .expect("read bundled descriptor");
        let anim = load_animation_desc(&desc_json).expect("parse bundled descriptor");
        let mut image = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../resources/firmware/decrypted/af_190602.bin"
        ))
        .expect("read bundled decrypted af_190602 image");
        let stock_len = image.len();

        let patch = build_gradient_fade_patch(&desc_json).expect("build gradient fade patch");
        let mods = patch.modifications.len();
        let min_off = patch.modifications.iter().map(|m| m.offset).min().unwrap();
        let max_off = patch.modifications.iter().map(|m| m.offset).max().unwrap();

        // Exactly what apply_patch_cmd does: single animation patch, so
        // rollback_other_animations is a no-op; then apply_patch.
        let mut rollback_log = std::collections::HashMap::new();
        let mut patches = vec![patch.clone()];
        rollback_other_animations(&mut image, &mut patches, &mut rollback_log, &patch.id);
        apply_patch(&mut image, &mut patches[0], &mut rollback_log).expect("apply patch");

        let patched_len = image.len();
        let growth = patched_len - stock_len;
        let max_aprom = 128 * 1024; // 0x20000
        let config_addr = anim.config_byte_addr as usize;

        println!("stock_len          = {stock_len} (0x{stock_len:x})");
        println!("patched_len        = {patched_len} (0x{patched_len:x})");
        println!("growth_bytes       = {growth}");
        println!("modification_count = {mods}");
        println!("min_modified_off   = 0x{min_off:x}");
        println!("max_modified_off   = 0x{max_off:x}");
        println!("within_stock_image = {}", max_off < stock_len);
        println!("plausible_aprom    = {} (<= {max_aprom})", patched_len <= max_aprom);
        println!(
            "reaches_config_byte_0x{config_addr:x} = {}",
            patched_len > config_addr
        );
        println!(
            "config_byte_in_mods = {}",
            patch.modifications.iter().any(|m| m.offset == config_addr)
        );
        // The growth is exactly the cave body — apply_patch only extends to
        // max_offset+1, leaving no trailing 0xFF padding.
        let padding = image[max_off + 1..].iter().filter(|b| **b == 0xFF).count();
        println!("trailing_padding_bytes = {padding}");
        assert_eq!(max_off + 1, patched_len, "no trailing padding after the last modification");
        // The patch must stay inside plausible APROM and must write the
        // config byte so the flashed animation is actually enabled.
        assert!(patched_len <= max_aprom, "patched image exceeds 128 KiB APROM");
        assert!(patched_len > config_addr, "image must reach the config byte addr 0x1f7f0");
        assert_eq!(
            patch
                .modifications
                .iter()
                .find(|m| m.offset == config_addr)
                .map(|m| m.patched),
            Some(2),
            "config byte must be written with the gradient-fade effect id (2)"
        );

        if std::env::var("WRITE_PATCHED").ok().as_deref() == Some("1") {
            std::fs::write("/tmp/anim_patched.bin", &image).expect("write /tmp/anim_patched.bin");
            println!("wrote /tmp/anim_patched.bin ({patched_len} bytes)");
        }
    }

    const DESC: &str = r#"{
        "build": "t",
        "render_entry": "0x0",
        "display_buffer": {"start": "0x20001000", "end": "0x20001400",
                           "width": 64, "height": 128},
        "animation": { "hook_site": "0x100", "hook_resume": "0x104",
            "code_cave": {"start": "0x8000", "size": 512},
            "config_byte_addr": "0x1F100", "phase_global": "0x20000000",
            "status_word_addr": "0x20000008",
            "status_base_addr": "0x20000004",
            "effects": {"gradient_fade": 2, "center_pulse": 3,
                        "diagonal_sweep": 4} }
    }"#;

    /// Reference apply per effect, uniform signature for the shared tests.
    fn apply(effect: &str, buf: &mut [u8], phase: u32) {
        match effect {
            "gradient" => gradient_fade_apply(buf, 64, 128, phase),
            "center" => center_pulse_apply(buf, 64, phase),
            "diagonal" => diagonal_sweep_apply(buf, 64, phase),
            _ => panic!("unknown effect {effect}"),
        }
    }

    /// LUT-index period per effect (full cycle of the phase→frame mapping).
    fn period(effect: &str) -> u32 {
        match effect {
            "gradient" => 64, // 16 LUT entries << 2
            _ => 16,
        }
    }

    // ------------------------------------------------------------------
    // Parametric panel-geometry property tests (drafted by a local ollama
    // agent, corrected + reviewed by hand). The geometries are the known
    // ArcticFox panels (see the table in flasher.rs above read_product_id).
    // The effects' index math is 64x128-shaped, so these tests document
    // safe behaviour (no OOB, byte-clearing only) on every panel size.
    // ------------------------------------------------------------------

    const PANEL_GEOMETRIES: [(usize, usize, usize); 5] = [
        (64, 128, 1024), // default/fallback, stride 8
        (96, 16, 192),   // Pico / Pico Dual / ASTER / TC family, stride 12
        (128, 32, 512),  // Pico 25 / Sinuous CB-80, stride 16
        (64, 48, 384),   // Wismec RX family
        (64, 32, 256),   // Pico Squeeze 2 / ASTER RT
    ];

    #[test]
    fn test_geometry_sweep_no_panic_and_size_invariant() {
        for &(w, h, bytes) in &PANEL_GEOMETRIES {
            for phase in 0..64 {
                let mut g = vec![0xFFu8; bytes];
                gradient_fade_apply(&mut g, w, h, phase);
                assert_eq!(g.len(), bytes, "gradient {w}x{h} phase {phase}: length changed");

                let mut c = vec![0xFFu8; bytes];
                center_pulse_apply(&mut c, w, phase);
                assert_eq!(c.len(), bytes, "center {w}x{h} phase {phase}: length changed");

                let mut d = vec![0xFFu8; bytes];
                diagonal_sweep_apply(&mut d, w, phase);
                assert_eq!(d.len(), bytes, "diagonal {w}x{h} phase {phase}: length changed");
            }
        }
    }

    #[test]
    fn test_effects_only_clear_bytes() {
        for &(w, h, bytes) in &PANEL_GEOMETRIES {
            for phase in 0..64 {
                for effect in ["gradient", "center", "diagonal"] {
                    let mut buf = vec![0xFFu8; bytes];
                    apply(effect, &mut buf, phase);
                    assert!(
                        buf.iter().all(|b| *b == 0x00 || *b == 0xFF),
                        "{effect} {w}x{h} phase {phase}: fade family only clears whole bytes in place"
                    );
                }
            }
        }
    }

    #[test]
    fn test_gradient_fade_band_monotonic_in_phase() {
        let mut a = vec![0xFFu8; 64 * 128 / 8];
        let mut b = vec![0xFFu8; 64 * 128 / 8];
        // The band steps one LUT entry per 4 phases ((strip*8 + phase) >> 2);
        // adjacent LUT entries are all on the same side of THRESHOLD, so a
        // 4-phase step changes nothing — 8 phases crosses it (strip 2:
        // index 4 -> 6, 98 -> 142).
        gradient_fade_apply(&mut a, 64, 128, 0);
        gradient_fade_apply(&mut b, 64, 128, 8);
        assert_ne!(a, b, "frames at phases 0 and 8 must differ");
        // bands are horizontal: each 64-byte group (8 rows) is one brightness
        for s in 0..16 {
            let base = s * 64;
            for i in 0..64 {
                assert_eq!(a[base + i], a[base], "band {s} constant across its 8 rows");
            }
        }
    }

    #[test]
    fn test_diagonal_sweep_full_cycle_covers_bands() {
        // Bands march over any fixed byte: byte 0 (strip 0, column group 0)
        // must survive at every phase whose LUT index >= 6 (index == phase
        // for byte 0), i.e. phases 6..=15.
        let mut survives = vec![false; 16];
        for phase in 0..16 {
            let mut buf = vec![0xFFu8; 64 * 128 / 8];
            diagonal_sweep_apply(&mut buf, 64, phase);
            survives[phase as usize] = buf[0] == 0xFF;
        }
        assert!(
            survives[6..].iter().all(|s| *s),
            "byte 0 must survive the marching band at phases 6..=15"
        );
        assert!(
            survives[..6].iter().all(|s| !*s),
            "byte 0 must be covered by the band at phases 0..=5"
        );
    }

    #[test]
    fn test_center_pulse_symmetry() {
        let mut buf = vec![0xFFu8; 64 * 128 / 8];
        center_pulse_apply(&mut buf, 64, 0);
        // Dark LUT indices are 0..=5 (values < 128). d = s ^ 7 lands in 0..=5
        // for strips 2..=7; everything else is kept at phase 0.
        for s in 2..=7 {
            assert_eq!(buf[s * 64], 0, "strip {s} cleared at phase 0 (d = {} ^ 7)", s);
        }
        for s in [0usize, 1, 8, 9, 10, 11, 12, 13, 14, 15] {
            assert_eq!(buf[s * 64], 0xFF, "strip {s} kept at phase 0");
        }
    }

    #[test]
    fn test_gradient_fade_band_moves_with_phase() {
        let mut a = vec![0xFFu8; 64 * 128 / 8]; // all pixels on
        let mut b = a.clone();
        gradient_fade_apply(&mut a, 64, 128, 0);
        gradient_fade_apply(&mut b, 64, 128, 32);
        assert_ne!(a, b, "different phases must dim different rows");
        // band is horizontal: same strip pattern for every column
        assert_eq!(a[0], a[1], "columns equal at same strip for phase 0");
    }

    #[test]
    fn test_gradient_fade_period() {
        let mut a = vec![0xFFu8; 64 * 128 / 8];
        let mut b = a.clone();
        gradient_fade_apply(&mut a, 64, 128, 0);
        gradient_fade_apply(&mut b, 64, 128, 64); // full LUT period (16 << 2)
        assert_eq!(a, b);
    }

    #[test]
    fn test_center_pulse_pulses_from_centre() {
        let mut a = vec![0xFFu8; 64 * 128 / 8];
        let mut b = a.clone();
        center_pulse_apply(&mut a, 64, 0);
        center_pulse_apply(&mut b, 64, 8); // half period: centre <-> edges
        assert_ne!(a, b);
        // At phase 0 LUT[0] = 0: strip 7 (d = 7^7 = 0) is the dark centre;
        // LUT[15] = 255: strip 8 (d = 8^7 = 15) is the bright centre flank.
        // Dark LUT entries are indices 0..=5 (<= 120).
        assert_eq!(a[7 * 64], 0, "strip 7 cleared at phase 0");
        assert_eq!(a[2 * 64], 0, "strip 2 cleared at phase 0 (d = 5)");
        assert_eq!(a[64], 0xFF, "strip 1 kept at phase 0 (d = 6, LUT[6] = 142)");
        assert_eq!(a[8 * 64], 0xFF, "strip 8 kept at phase 0 (LUT[15] = 255)");
        assert_eq!(a[0], 0xFF, "strip 0 kept at phase 0 (d = 7, LUT[7] = 162)");
    }

    #[test]
    fn test_diagonal_sweep_marches() {
        let mut frames: Vec<Vec<u8>> = Vec::new();
        for ph in 0..16 {
            let mut f = vec![0xFFu8; 64 * 128 / 8];
            apply("diagonal", &mut f, ph);
            frames.push(f);
        }
        // all 16 phases distinct (strip+column index spreads the band)
        for i in 0..16 {
            for j in (i + 1)..16 {
                assert_ne!(frames[i], frames[j], "phases {i} and {j} identical");
            }
        }
    }

    #[test]
    fn test_periods() {
        for effect in ["gradient", "center", "diagonal"] {
            let mut a = vec![0xFFu8; 64 * 128 / 8];
            let mut b = a.clone();
            apply(effect, &mut a, 5);
            apply(effect, &mut b, 5 + period(effect));
            assert_eq!(a, b, "{effect}: phase wraps at its period");
        }
    }

    #[test]
    fn test_build_patch_layout() {
        let p = build_gradient_fade_patch(DESC).unwrap();
        // hook: 4 bytes at 0x100 = bw_pair(0x100, 0x8000)
        let [h1, h2] = bw_pair(0x100, 0x8000);
        let at = |off: usize| {
            p.modifications
                .iter()
                .find(|m| m.offset == off)
                .unwrap_or_else(|| panic!("no modification at {off:#x}"))
                .patched
        };
        assert_eq!(at(0x100), (h1 & 0xFF) as u8);
        assert_eq!(at(0x101), (h1 >> 8) as u8);
        assert_eq!(at(0x102), (h2 & 0xFF) as u8);
        assert_eq!(at(0x103), (h2 >> 8) as u8);
        // cave body starts at 0x8000, fits in 512 bytes; the config byte at
        // 0x1F100 is a separate modification beyond the cave.
        let max_cave = p
            .modifications
            .iter()
            .filter(|m| m.offset != 0x1F100)
            .map(|m| m.offset)
            .max()
            .unwrap();
        assert!(max_cave < 0x8000 + 512);
        assert_eq!(at(0x1F100), 2, "config byte = gradient-fade effect id");
    }

    #[test]
    fn test_all_builders_fit_cave() {
        for build in [
            build_gradient_fade_patch as fn(&str) -> Result<Patch, AnimError>,
            build_center_pulse_patch,
            build_diagonal_sweep_patch,
        ] {
            let p = build(DESC).unwrap();
            let max_cave = p
                .modifications
                .iter()
                .filter(|m| m.offset != 0x1F100)
                .map(|m| m.offset)
                .max()
                .unwrap();
            assert!(max_cave < 0x8000 + 512, "{} overflows cave", p.id);
        }
    }

    /// Steps the emitted cave for `effect` in the emu against the Rust
    /// reference: same inputs must produce the same framebuffer.
    fn assert_cave_matches_reference(
        effect: &str,
        emit: EmitFn,
        config: u8,
    ) {
        use crate::firmware::emu::bus::{Bus, RAM_BASE};
        use crate::firmware::emu::cpu::Cpu;

        let anim: AnimationDesc = serde_json::from_value(
            serde_json::from_str::<serde_json::Value>(DESC).unwrap()["animation"].clone(),
        )
        .unwrap();
        let mut asm = Asm::new(anim.code_cave.start);
        emit(&mut asm, &anim, 0x20001000, 0x400).unwrap();
        let body = asm.finish().unwrap();

        let mut flash = vec![0u8; 0x9000];
        flash[0x8000..0x8000 + body.len()].copy_from_slice(&body);
        let mut bus = Bus::new(flash, 0x1400); // RAM covers phase global + buffer
        bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
        bus.allow_region(0x20001000..0x20001400);
        bus.set_stub(anim.config_byte_addr, config as u32);
        bus.write_u32(anim.status_word_addr, 0x20000).unwrap(); // timeout bit set
        for i in 0..0x400u32 {
            bus.write_u8(0x20001000 + i, 0xFF).unwrap();
        }

        let mut cpu = Cpu::new();
        cpu.r[2] = 0x20000; // timeout bit set
        cpu.pc = 0x8000;
        for _ in 0..(0x400 * 20 + 100) {
            cpu.step(&mut bus).unwrap();
            if cpu.pc == 0x104 {
                break;
            }
        }
        assert_eq!(cpu.pc, 0x104, "{effect}: cave must branch back to hook_resume");
        assert_eq!(cpu.r[4], 1, "{effect}: phase global bumped once");
        assert_eq!(
            cpu.r[2], 0x20000,
            "{effect}: cave must restore r2 (status word) for the stock beq"
        );
        assert_eq!(
            cpu.r[3], 0x20000004,
            "{effect}: cave must restore r3 to the status base (stock re-reads [base+4])"
        );
        assert!(!cpu.z, "{effect}: restored flags must reflect the timeout bit (Z=0)");
        let mut expect = vec![0xFFu8; 0x400];
        apply(effect, &mut expect, 1);
        let got: Vec<u8> = (0..0x400)
            .map(|i| bus.read_u8(0x20001000 + i as u32).unwrap())
            .collect();
        assert_eq!(got, expect, "{effect}: emitted cave must match the Rust reference");
    }

    #[test]
    fn test_emitted_caves_match_references_in_emu() {
        assert_cave_matches_reference("gradient", emit_gradient_fade, CONFIG_GRADIENT_FADE);
        assert_cave_matches_reference("center", emit_center_pulse, CONFIG_CENTER_PULSE);
        assert_cave_matches_reference("diagonal", emit_diagonal_sweep, CONFIG_DIAGONAL_SWEEP);
    }

    #[test]
    fn test_image_supports_animation_gate() {
        let anim = load_animation_desc(DESC).unwrap();
        // DESC hook_site 0x100, cave 0x8000 (past any test image).

        let mut img = vec![0u8; 0x1000];
        assert!(!image_supports_animation(&img, &anim), "all-zero hook site");

        img[0x100..0x104].copy_from_slice(&HOOK_SITE_STOCK_BYTES);
        assert!(image_supports_animation(&img, &anim));

        // Different build / already detoured at the hook site.
        img[0x101] = 0x00;
        assert!(!image_supports_animation(&img, &anim), "hook site tampered");
        img[0x101] = HOOK_SITE_STOCK_BYTES[1];

        // Image too small to contain the hook site.
        assert!(!image_supports_animation(&img[..0x100], &anim));

        // Cave inside the image must be erased flash.
        let cave_json = r#"{"animation": { "hook_site": "0x100", "hook_resume": "0x104",
            "code_cave": {"start": "0x400", "size": 256},
            "config_byte_addr": "0x1F100", "phase_global": "0x20000000",
            "status_word_addr": "0x20000008",
            "status_base_addr": "0x20000004" }}"#;
        let cave_anim = load_animation_desc(cave_json).unwrap();
        let mut erased = vec![0xFFu8; 0x1000];
        erased[0x100..0x104].copy_from_slice(&HOOK_SITE_STOCK_BYTES);
        assert!(image_supports_animation(&erased, &cave_anim));
        erased[0x4FF] = 0x42;
        assert!(!image_supports_animation(&erased, &cave_anim), "cave not erased");
    }

    #[test]
    fn test_load_animation_desc_rejects_bad_status_base() {
        let bad = r#"{"animation": { "hook_site": "0x100", "hook_resume": "0x104",
            "code_cave": {"start": "0x8000", "size": 512},
            "config_byte_addr": "0x1F100", "phase_global": "0x20000000",
            "status_word_addr": "0x20000008",
            "status_base_addr": "0x20000000" }}"#;
        assert!(load_animation_desc(bad).is_err());
    }

    #[test]
    fn test_cave_resumes_stock_path_when_gated_off() {
        use crate::firmware::emu::bus::{Bus, RAM_BASE};
        use crate::firmware::emu::cpu::Cpu;

        let anim: AnimationDesc = serde_json::from_value(
            serde_json::from_str::<serde_json::Value>(DESC).unwrap()["animation"].clone(),
        )
        .unwrap();
        let mut asm = Asm::new(anim.code_cave.start);
        emit_gradient_fade(&mut asm, &anim, 0x20001000, 0x400).unwrap();
        let body = asm.finish().unwrap();

        for (r2, config, label) in [
            (0u32, CONFIG_GRADIENT_FADE as u32, "timeout bit clear"),
            (0x20000, 0, "config byte 0 = stock"),
        ] {
            let mut flash = vec![0u8; 0x9000];
            flash[0x8000..0x8000 + body.len()].copy_from_slice(&body);
            let mut bus = Bus::new(flash, 0x1400);
            bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
            bus.allow_region(0x20001000..0x20001400);
            bus.set_stub(anim.config_byte_addr, config);
            for i in 0..0x400u32 {
                bus.write_u8(0x20001000 + i, 0xFF).unwrap();
            }
            let mut cpu = Cpu::new();
            cpu.r[2] = r2;
            cpu.pc = 0x8000;
            for _ in 0..100 {
                cpu.step(&mut bus).unwrap();
                if cpu.pc == 0x104 {
                    break;
                }
            }
            assert_eq!(cpu.pc, 0x104, "{label}: must resume");
            let intact = (0..0x400u32)
                .all(|i| bus.read_u8(0x20001000 + i).unwrap() == 0xFF);
            assert!(intact, "{label}: buffer must be untouched");
        }
    }
}
