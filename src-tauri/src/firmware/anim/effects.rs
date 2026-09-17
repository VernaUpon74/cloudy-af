//! Animation effects: pure Rust references + Thumb-1 cave emission.
//!
//! Every effect is a **self-contained animation**: the cave clears the whole
//! framebuffer and then *draws its own content*, so what appears on the charge
//! screen is a genuine enumerated animation and never the stock charge text
//! dimmed, banded, or otherwise manipulated.
//!
//! Buffer geometry (see `resources/re/af_190602-render.md`): 64x128 1bpp,
//! vertically packed (`pixel x+(y/8)*width, bit y%8`), so byte `i` lives in
//! 8-pixel strip `i >> 6` at column `i & 63` — 16 strips of 64 bytes.
//!
//! Injection (see `resources/re/af_190602-dispatcher-analy.md` §7): a 4-byte
//! B.W at the timeout decision point `0x9a16` (in `FUN_00009a10`) detours
//! into the APROM-tail code cave. The cave replays the overwritten
//! `tst.w r2, #0x20000`; when the timeout bit is clear (or the config byte
//! selects no effect) it resumes the stock path; when timed out it bumps the
//! phase global, redraws the display buffer, and resumes.
//!
//! The config byte (a spare dataflash byte) selects the effect:
//! 2 = Gradient Bar, 3 = Center Pulse, 4 = Diagonal Sweep, 5 = Wave.

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

/// Framebuffer geometry the effect maths is expressed against: 64 columns of
/// 1bpp pixels, 128 rows tall, vertically packed (`byte = ((y>>3)<<6) + x`,
/// `bit = 1 << (y & 7)`) — 1024 bytes total.
pub const FB_WIDTH: usize = 64;
pub const FB_HEIGHT: usize = 128;

/// Set one pixel in a Block1-packed framebuffer. Out-of-range pixels are
/// dropped, so the drawing references are total functions on any buffer.
#[inline]
pub fn set_px(buf: &mut [u8], x: usize, y: usize) {
    if x >= FB_WIDTH || y >= FB_HEIGHT {
        return;
    }
    let idx = ((y >> 3) << 6) + x;
    if let Some(b) = buf.get_mut(idx) {
        *b |= 1 << (y & 7);
    }
}

/// Draw a vertical run of pixels in one column (`y0..=y1`, inclusive).
#[inline]
pub fn set_col(buf: &mut [u8], x: usize, y0: usize, y1: usize) {
    for y in y0..=y1.min(FB_HEIGHT.saturating_sub(1)) {
        set_px(buf, x, y);
    }
}

/// Draw a horizontal run of pixels in one row (`x0..=x1`, inclusive).
#[inline]
pub fn set_row(buf: &mut [u8], x0: usize, x1: usize, y: usize) {
    for x in x0..=x1.min(FB_WIDTH.saturating_sub(1)) {
        set_px(buf, x, y);
    }
}

/// Config-byte values selecting each effect.
pub const CONFIG_GRADIENT_FADE: u8 = 2;
pub const CONFIG_CENTER_PULSE: u8 = 3;
pub const CONFIG_DIAGONAL_SWEEP: u8 = 4;
pub const CONFIG_WAVE: u8 = 5;

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

/// Clear the whole framebuffer so no stock charge-screen content survives.
#[inline]
pub fn clear_frame(buf: &mut [u8]) {
    buf.fill(0);
}

/// Gradient Bar (config 2): a solid horizontal bar of `BAR_THICKNESS` rows
/// sweeps vertically down the panel, wrapping at the bottom. The bar's top
/// edge follows the same phase→row mapping the Thumb cave computes with
/// shifts and adds: `y = ((phase >> 2) << 3) & 127` — 128 phases per full
/// sweep. Drawn from a cleared frame, so only the bar is on-screen.
pub const BAR_THICKNESS: usize = 12;

pub fn gradient_fade_apply(buf: &mut [u8], width: usize, _height: usize, phase: u32) {
    let _ = width; // geometry is fixed (see FB_WIDTH/FB_HEIGHT)
    clear_frame(buf);
    let top = (((phase >> 2) << 3) & 127) as usize;
    for d in 0..BAR_THICKNESS {
        let y = (top + d) & (FB_HEIGHT - 1);
        set_row(buf, 0, FB_WIDTH - 1, y);
    }
}

/// Center Pulse (config 3): expanding rings drawn as panel-scaled ellipses.
///
/// The ring is the ellipse `(dx/a)² + (dy/b)² = 1` with `b = 2a` (the panel is
/// twice as tall as it is wide), scaled by `s = 1 + (phase % 8)` of 8 steps, so
/// the half-width sweeps `4, 8, … 32` and the ring is always fully on-panel
/// (it reaches the screen edges at the last step, then restarts). Drawn from a
/// cleared frame, so only the ring is on-screen.
pub const RING_STEPS: u32 = 8;

/// Half-width of the ring at step `s` (1..=RING_STEPS): 4 * s, max 32.
#[inline]
pub fn ring_half_width(step: u32) -> usize {
    4 * (step as usize)
}

pub fn center_pulse_apply(buf: &mut [u8], width: usize, phase: u32) {
    let _ = width;
    clear_frame(buf);
    let step = 1 + (phase % RING_STEPS); // 1..=8
    let a = ring_half_width(step) as i64; // horizontal half-width, 4..=32
    let cx = FB_WIDTH as i64 / 2; // 32
    let cy = FB_HEIGHT as i64 / 2; // 64
    for x in 0..FB_WIDTH {
        let dx = (x as i64) - cx;
        if dx.abs() > a {
            continue;
        }
        // dy = 2 * isqrt(a² - dx²)  — the ellipse's vertical half-axis
        let dy = 2 * isqrt(a * a - dx * dx);
        for y in [cy + dy, cy - dy] {
            if (0..FB_HEIGHT as i64).contains(&y) {
                set_px(buf, x, y as usize);
            }
        }
    }
}

/// Integer square root (floor), used by the ring reference so the Rust model
/// stays all-integer like the Thumb cave.
#[inline]
fn isqrt(v: i64) -> i64 {
    if v <= 0 {
        return 0;
    }
    let mut r = (v as f64).sqrt() as i64;
    while r * r > v {
        r -= 1;
    }
    while (r + 1) * (r + 1) <= v {
        r += 1;
    }
    r
}

/// Diagonal Sweep (config 4): a thick diagonal bar marches from the top-left
/// to the bottom-right corner and wraps. `x + y == c` selects the bar's
/// diagonal; `c` advances with phase. Drawn from a cleared frame.
pub const DIAG_THICKNESS: usize = 10;

pub fn diagonal_sweep_apply(buf: &mut [u8], _width: usize, phase: u32) {
    clear_frame(buf);
    let span = FB_WIDTH + FB_HEIGHT; // 192 diagonals for this panel
    let c = ((phase % (span as u32 / 6)) * 6) as usize;
    for x in 0..FB_WIDTH {
        // y = c - x, plus the bar's thickness along y.
        for d in 0..DIAG_THICKNESS {
            let y = (c + d) as isize - x as isize;
            if y >= 0 && (y as usize) < FB_HEIGHT {
                set_px(buf, x, y as usize);
            }
        }
    }
}

/// Wave (config 5) — the first effect that RENDERS NEW CONTENT instead of
/// fading the existing screen: clears the whole 64x128 buffer, then draws a
/// flowing sine wave — a pixel at `y = 64 + sin((x + 4*phase)/64 * 2pi) * 24`
/// and its vertical mirror `127 - y` — so two waves cross in the middle.
/// Non-cumulative by construction (each pass wipes first), which the shared
/// reference-replay `for p in 1..=phase { apply }` folds to apply(phase).
/// Period 16 phases (angle advances 4/64 per phase).
pub fn wave_apply(buf: &mut [u8], phase: u32) {
    buf.fill(0);
    for x in 0..64u32 {
        let a = (x + phase * 4) & 63;
        let q = a & 15;
        let mut v = SINE_LUT[q as usize] as u32;
        if a & 16 != 0 {
            v = 255 - v; // falling quarter: mirror of the LUT
        }
        let off = (v * 24) >> 8; // 0..=23
        let y = if a & 32 != 0 { 64 + off } else { 64 - off };
        // set_px: byte ((y >> 3) << 6) + x, bit 1 << (y & 7)
        let idx = ((y >> 3) << 6) + x;
        buf[idx as usize] |= 1 << (y & 7);
        let y2 = 127 - y;
        let idx2 = ((y2 >> 3) << 6) + x;
        buf[idx2 as usize] |= 1 << (y2 & 7);
    }
}

/// Emitter signature shared by all effects: append the cave body to `a`
/// (based at `anim.code_cave.start`) for `buf_addr..buf_addr+buf_len`.
pub type EmitFn = fn(&mut Asm, &AnimationDesc, u32, u32) -> Result<(), AsmError>;

/// Emits the cave prologue shared by every effect: replay the overwritten
/// hook-site `tst.w`, bail out to `resume` unless the timeout bit is set and
/// the config byte matches, then bump the phase global into r4.
///
/// On return r4 = the new phase, r0/r2 are free; the caller emits its drawing
/// code followed by [`emit_cave_epilogue`].
fn emit_cave_prologue(a: &mut Asm, _anim: &AnimationDesc, config_value: u8) {
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
    a.adds(4, 1); // phase += 1
    a.str_imm(4, 0, 0);
}

/// Emits the cave epilogue shared by every effect: restore the hook-site
/// register/flag state the stock path relies on, then branch back.
///
/// The drawing loops clobber r2/r3 and leave Z=1, which would wrongly divert
/// the stock `beq` after `hook_resume`. Re-read the status word into r2,
/// restore r3 to the status BASE the hook literal holds (stock re-reads
/// `[base + 4]` in its timeout branch), and replay the overwritten `tst.w`.
fn emit_cave_epilogue(a: &mut Asm, anim: &AnimationDesc) {
    a.ldr_lit(3, "stw");
    a.ldr_imm(2, 3, 0); // r2 = [status_word]
    a.ldr_lit(3, "stb"); // r3 = status_base (stock register at hook_resume)
    a.raw32(0xF412, 0x3F00); // tst.w r2, #0x20000
    a.label("resume");
    a.b_abs(anim.hook_resume);
}

/// Pools the words every cave needs: config byte, phase global, framebuffer
/// base and length, status word/base, and the sine LUT block.
fn emit_cave_pool(a: &mut Asm, anim: &AnimationDesc, buf_addr: u32, buf_len: u32) {
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
}

/// Emits the shared "wipe the framebuffer" loop: `buf[0..len] = 0`, where the
/// pooled `len` word carries the framebuffer length.
///
/// Every effect starts from a cleared frame, so no stock charge-screen content
/// can bleed into the animation. Clobbers r0 (index), r2 (zero), r7 (count);
/// leaves r1 = buf.
fn emit_clear_frame(a: &mut Asm) {
    a.movs(0, 0);
    a.movs(2, 0);
    a.ldr_lit(1, "buf");
    a.ldr_lit(7, "len");
    a.label("clr");
    a.ls_reg(2, 2, 1, 0); // strb r2, [r1, r0]
    a.adds(0, 1);
    a.subs(7, 1);
    a.bcond(0x1, "clr"); // NE
}

/// Emits a pixel set: OR the bit for row `y_reg` into the framebuffer at
/// column `x_reg`.
///
/// Contract: needs r1 = framebuffer base. Scratches r5 (shift count/byte),
/// r6 (byte index), r7 (bit mask), and flags; preserves r0-r4.
/// Coordinates must be valid panel positions in r0, r2, r3, or r4.
fn emit_set_px(a: &mut Asm, y_reg: u8, x_reg: u8) {
    debug_assert!(
        [0, 2, 3, 4].contains(&y_reg) && [0, 2, 3, 4].contains(&x_reg),
        "emit_set_px coordinates must use preserved registers r0/r2/r3/r4"
    );
    // r6 = ((y >> 3) << 6) + x  — index of the byte holding the pixel
    a.lsrs(6, y_reg, 3);
    a.lsls(6, 6, 6);
    a.adds_reg(6, 6, x_reg);
    // r7 = 1 << (y & 7)  — the bit within that byte
    a.movs(5, 7);
    a.ands(5, y_reg);
    a.movs(7, 1);
    a.raw16(0x4080 | (5u16 << 3) | 7); // lsls r7, r5: 1 << (y & 7)
    // buf[r6] |= r7  — ls_reg(op, rt, rn, rm) with op 6 = LDRB, 2 = STRB
    a.ls_reg(6, 5, 1, 6); // ldrb r5, [r1, r6]
    a.raw16(0x4300 | ((7 as u16) << 3) | 5); // orrs r5, r7
    a.ls_reg(2, 5, 1, 6); // strb r5, [r1, r6]
    let _ = x_reg;
}

/// Gradient Bar (config 2): a solid horizontal bar sweeps vertically.
///
/// The bar's top row is `((phase >> 2) << 3) & 127` (64 phases per full
/// sweep, matching `gradient_fade_apply`); `BAR_THICKNESS` full-width rows are
/// filled, wrapping at the bottom edge. Drawn from a cleared frame.
///
/// Registers: r4 = bar top (phase is consumed into it), r2 = bar row offset,
/// r0 = x, r3 = y passed to `emit_set_px` (which preserves r0/r2/r3/r4 and
/// scratches r5/r6/r7).
pub fn emit_gradient_fade(
    a: &mut Asm,
    anim: &AnimationDesc,
    buf_addr: u32,
    buf_len: u32,
) -> Result<(), AsmError> {
    emit_cave_prologue(a, anim, CONFIG_GRADIENT_FADE);
    emit_clear_frame(a);
    // r4 = top row = ((phase >> 2) << 3) & 127
    a.lsrs(4, 4, 2);
    a.lsls(4, 4, 3);
    a.movs(3, 127);
    a.ands(4, 3);
    a.movs(2, 0); // d = row offset within the bar
    a.label("barrow");
    a.movs(0, 0); // x = 0
    a.label("barcol");
    // r3 = y = (top + d) & 127
    a.movs_reg(3, 4);
    a.adds_reg(3, 3, 2);
    a.movs(5, 127);
    a.ands(3, 5);
    emit_set_px(a, 3, 0);
    a.adds(0, 1);
    a.cmp(0, FB_WIDTH as u8);
    a.bcond(0x3, "barcol"); // LO: next column
    a.adds(2, 1);
    a.cmp(2, BAR_THICKNESS as u8);
    a.bcond(0x3, "barrow"); // LO: next bar row
    emit_cave_epilogue(a, anim);
    emit_cave_pool(a, anim, buf_addr, buf_len);
    Ok(())
}

/// Center Pulse (config 3): expanding rings drawn as panel-scaled ellipses.
///
/// Half-width `a = 4 + (phase & 7) * 4` (4..=32), matching
/// `center_pulse_apply`. For each column the cave computes
/// `y = 64 ± isqrt(a² - dx²) * 2`; the integer square root is a restoring
/// bit-by-bit loop, all-integer like the rest of the cave (the emulator's
/// `muls` is exact for these magnitudes). Drawn from a cleared frame.
///
/// Registers: r4 = a, r2 = x, r3 = y (the `emit_set_px` contract), r0/r5/r6/r7
/// = scratch, r1 = framebuffer base (never written).
pub fn emit_center_pulse(
    a: &mut Asm,
    anim: &AnimationDesc,
    buf_addr: u32,
    buf_len: u32,
) -> Result<(), AsmError> {
    emit_cave_prologue(a, anim, CONFIG_CENTER_PULSE);
    emit_clear_frame(a);
    // r4 = a = 4 + (phase & 7) * 4
    a.movs(5, 7);
    a.ands(5, 4); // r5 = phase & 7
    a.lsls(4, 5, 2); // r4 = (phase & 7) * 4
    a.adds(4, 4); // r4 = 4 + (phase & 7) * 4
    a.movs(2, 0); // x = 0
    a.label("rcol");
    // r6 = |dx| = |x - 32|
    a.movs_reg(6, 2);
    a.movs(5, 32);
    a.subs_reg(6, 6, 5); // r6 = x - 32
    a.cmp(6, 64); // unsigned: a borrowed (negative) dx exceeds 64
    a.bcond(0x9, "absdone"); // LS: 0..=63 -> already non-negative
    a.movs(5, 0);
    a.subs_reg(6, 5, 6); // r6 = -dx
    a.label("absdone");
    a.muls(6, 6); // r6 = dx²
    // r5 = a²; if dx² > a² this column is outside the ring
    a.movs_reg(5, 4);
    a.muls(5, 5);
    a.raw16(0x4280 | (5u16 << 3) | 6); // cmp r6, r5 (dx² vs a²)
    a.bcond(0x8, "rnext"); // HI
    a.subs_reg(6, 5, 6); // r6 = radicand = a² - dx²
    // Build floor(sqrt(radicand)) one bit at a time, high bit first.
    // Keep the radicand unchanged when accepting each candidate root.
    a.movs(3, 0); // result
    a.movs(5, 32); // a <= 32 -> root <= 32
    a.label("isq");
    a.movs_reg(7, 3);
    a.adds_reg(7, 7, 5); // r7 = t = result + bit
    a.movs_reg(0, 7);
    a.muls(0, 0); // r0 = t²
    a.raw16(0x4280 | (6u16 << 3)); // cmp r0, r6 (t² vs radicand)
    a.bcond(0x8, "isqskip"); // HI: candidate too large
    a.movs_reg(3, 7); // result = t
    a.label("isqskip");
    a.lsrs(5, 5, 1); // bit >>= 1
    a.cmp(5, 0);
    a.bcond(0x1, "isq"); // NE: more bits
    // dy = result * 2 ; ring rows at 64 ± dy (128 = off-panel, clipped)
    a.lsls(3, 3, 1);
    a.movs(5, 64);
    a.adds_reg(3, 5, 3); // r3 = 64 + dy
    a.cmp(3, 128);
    a.bcond(0x2, "rhi"); // HS: row 128+ never on-panel
    emit_set_px(a, 3, 2);
    a.label("rhi");
    a.movs(5, 128);
    a.subs_reg(3, 5, 3); // r3 = 128 - (64 + dy) = 64 - dy
    emit_set_px(a, 3, 2);
    a.label("rnext");
    a.adds(2, 1);
    a.cmp(2, FB_WIDTH as u8);
    a.bcond(0x3, "rcol"); // LO
    emit_cave_epilogue(a, anim);
    emit_cave_pool(a, anim, buf_addr, buf_len);
    Ok(())
}

/// Diagonal Sweep (config 4): a thick diagonal bar marches corner to corner.
///
/// Diagonal constant `c = (phase * 6) mod 192` (192 = width + height for this
/// panel, matching `diagonal_sweep_apply`). For each column the cave sets
/// `DIAG_THICKNESS` pixels at `y = c + d - x`, skipping rows outside 0..=127.
/// Drawn from a cleared frame.
///
/// Registers: r4 = c (phase consumed), r0 = x, r2 = d, r3 = y; `emit_set_px`
/// preserves r0/r2/r3/r4 and scratches r5/r6/r7.
pub fn emit_diagonal_sweep(
    a: &mut Asm,
    anim: &AnimationDesc,
    buf_addr: u32,
    buf_len: u32,
) -> Result<(), AsmError> {
    emit_cave_prologue(a, anim, CONFIG_DIAGONAL_SWEEP);
    emit_clear_frame(a);
    // Reduce the 32-phase cycle BEFORE multiplication. Since 32 * 6 = 192,
    // this is equivalent to (phase * 6) % 192 without a phase-sized loop.
    a.movs(3, 31);
    a.ands(4, 3);
    a.movs(3, 6);
    a.muls(4, 3); // c = (phase & 31) * 6, always 0..=186
    a.movs(0, 0); // x = 0
    a.label("dcol");
    a.movs(2, 0); // d = 0
    a.label("dthick");
    // r3 = y = c + d - x
    a.movs_reg(3, 4);
    a.adds_reg(3, 3, 2);
    a.subs_reg(3, 3, 0);
    // unsigned-compare trick: y > 127 catches negative y too (128 is never on-panel)
    a.cmp(3, 127);
    a.bcond(0x8, "dskip"); // HI: row off-panel
    emit_set_px(a, 3, 0);
    a.label("dskip");
    a.adds(2, 1);
    a.cmp(2, DIAG_THICKNESS as u8);
    a.bcond(0x3, "dthick"); // LO
    a.adds(0, 1);
    a.cmp(0, FB_WIDTH as u8);
    a.bcond(0x3, "dcol"); // LO
    emit_cave_epilogue(a, anim);
    emit_cave_pool(a, anim, buf_addr, buf_len);
    Ok(())
}

/// Emits the Wave cave (config 5) — the first effect that renders NEW
/// content instead of fading the existing pixels: replay the overwritten
/// timeout test, gate on the config byte, bump the phase global, CLEAR the
/// whole buffer, then draw the flowing sine wave (one pixel + its vertical
/// mirror per column; see `wave_apply` for the math).
///
/// Register contract: r0 = i (clear loop) then x (draw loop), r1 = buf,
/// r4 = phase, r5 = LUT pointer, r2/r3/r6/r7 scratch. Store discipline:
/// strb to the framebuffer only, one phase-global str — same as the fades.
/// The pixel-block maths needs three register-form T1 ops the builder lacks
/// as named methods, emitted via `raw16`: LSLS-reg 0x4080 (op 2 in the
/// 0x40xx data-processing group), ORRS 0x4300 (op 12). Subtraction uses the
/// ADD/SUB group (`Asm::subs_reg`, 0x1A00) — NOT the 0x40xx family, where
/// op nibble 6 is SBC.
pub fn emit_wave(
    a: &mut Asm,
    anim: &AnimationDesc,
    buf_addr: u32,
    buf_len: u32,
) -> Result<(), AsmError> {
    // tst.w r2, #0x20000 — the overwritten hook-site instruction, replayed.
    a.raw32(0xF412, 0x3F00);
    a.bcond(0x0, "resume"); // EQ: timeout bit clear -> stock clock path
    a.ldr_lit(0, "cfg");
    a.ldrb_imm(0, 0, 0);
    a.cmp(0, CONFIG_WAVE);
    a.bcond(0x1, "resume"); // NE: not our effect -> stock path
    a.ldr_lit(0, "phg");
    a.ldr_imm(4, 0, 0);
    a.adds(4, 1); // phase += 1
    a.str_imm(4, 0, 0);
    // --- clear the whole framebuffer
    a.movs(0, 0);
    a.movs(2, 0);
    a.ldr_lit(1, "buf");
    a.ldr_lit(7, "len");
    a.label("clr");
    a.ls_reg(2, 2, 1, 0); // strb r2, [r1, r0]
    a.adds(0, 1);
    a.subs(7, 1);
    a.bcond(0x1, "clr"); // NE
    // --- draw: r0 = x, r5 = LUT
    a.movs(0, 0);
    a.ldr_lit(5, "lutp");
    a.label("dloop");
    // r6 = a = (x + 4*phase) & 63
    a.movs_reg(6, 4);
    a.lsls(6, 6, 2);
    a.adds_reg(6, 6, 0);
    a.movs(3, 63);
    a.ands(6, 3);
    // r2 = v = LUT[a & 15]
    a.movs(3, 15);
    a.movs_reg(2, 6);
    a.ands(2, 3);
    a.ls_reg(6, 2, 5, 2); // ldrb r2, [r5, r2]
    // falling quarter: if a & 16 { v = 255 - v }  (XOR with 255)
    a.movs(3, 16);
    a.ands(3, 6);
    a.bcond(0x0, "w0"); // EQ: skip
    a.movs(3, 255);
    a.eors(2, 3);
    a.label("w0");
    // r2 = off = (v * 24) >> 8   (v*16 + v*8)
    a.lsls(3, 2, 4);
    a.lsls(2, 2, 3);
    a.adds_reg(2, 2, 3);
    a.lsrs(2, 2, 8);
    // r3 = y = a & 32 ? 64 + off : 64 - off
    a.movs(3, 32);
    a.ands(3, 6);
    a.bcond(0x0, "wlo"); // EQ: lower half
    a.movs(3, 64);
    a.adds_reg(3, 3, 2);
    a.b("wdone");
    a.label("wlo");
    a.movs(3, 64);
    a.subs_reg(3, 3, 2); // subs r3, r3, r2  (64 - off)
    a.label("wdone");
    // pixel(r3): r7 = 1 << (y & 7); r2 = ((y>>3)<<6) + x; buf[r2] |= r7
    a.movs(2, 7);
    a.ands(2, 3);
    a.movs(7, 1);
    a.raw16(0x4080 | (2 << 3) | 7); // lsls r7, r2 (register form)
    a.lsrs(2, 3, 3);
    a.lsls(2, 2, 6);
    a.adds_reg(2, 2, 0);
    a.ls_reg(6, 6, 1, 2); // ldrb r6, [r1, r2]
    a.raw16(0x4300 | (7 << 3) | 6); // orrs r6, r7
    a.ls_reg(2, 6, 1, 2); // strb r6, [r1, r2]
    // mirrored pixel: r3 = 127 - y, same block
    a.movs(6, 127);
    a.subs_reg(6, 6, 3); // subs r6, r6, r3  (127 - y)
    a.movs_reg(3, 6);
    a.movs(2, 7);
    a.ands(2, 3);
    a.movs(7, 1);
    a.raw16(0x4080 | (2 << 3) | 7); // lsls r7, r2
    a.lsrs(2, 3, 3);
    a.lsls(2, 2, 6);
    a.adds_reg(2, 2, 0);
    a.ls_reg(6, 6, 1, 2); // ldrb r6, [r1, r2]
    a.raw16(0x4300 | (7 << 3) | 6); // orrs r6, r7
    a.ls_reg(2, 6, 1, 2); // strb r6, [r1, r2]
    // next column
    a.adds(0, 1);
    a.cmp(0, 64);
    a.bcond(0x3, "dloop"); // LO: x < 64
    // Restore the hook-site register/flag state (same tail as the fades):
    // re-read the status word into r2, point r3 at the status base, replay
    // the overwritten tst.w so the resume behaves like unpatched code.
    a.ldr_lit(3, "stw");
    a.ldr_imm(2, 3, 0);
    a.ldr_lit(3, "stb");
    a.raw32(0xF412, 0x3F00); // tst.w r2, #0x20000
    a.label("resume");
    a.b_abs(anim.hook_resume);

    a.pool_word("cfg", anim.config_byte_addr);
    a.pool_word("phg", anim.phase_global);
    a.pool_word("buf", buf_addr);
    a.pool_word("len", buf_len);
    a.pool_word("stw", anim.status_word_addr);
    a.pool_word("stb", anim.status_base_addr);
    a.pool_addr("lutp", "lut");
    for w in SINE_LUT.chunks(4) {
        a.pool_word("lut", u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
    }
    Ok(())
}

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

pub fn build_wave_patch(desc_json: &str) -> Result<Patch, AnimError> {
    build_effect_patch(
        desc_json,
        "anim-wave",
        "Wave (charge-screen screensaver)",
        "Renders a new flowing double sine wave on the cleared screen on \
         timeout — content the stock screen never shows (config byte 5)",
        "wave",
        emit_wave,
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
                        "diagonal_sweep": 4, "wave": 5} }
    }"#;

    /// Reference apply per effect, uniform signature for the shared tests.
    fn apply(effect: &str, buf: &mut [u8], phase: u32) {
        match effect {
            "gradient" => gradient_fade_apply(buf, 64, 128, phase),
            "center" => center_pulse_apply(buf, 64, phase),
            "diagonal" => diagonal_sweep_apply(buf, 64, phase),
            "wave" => wave_apply(buf, phase),
            _ => panic!("unknown effect {effect}"),
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
    fn test_wave_renders_new_content_not_fades_text() {
        // Start from a "text screen" (all-on buffer): wave must WIPE it and
        // draw its own pattern — a sparse double sine wave, NOT the original
        // content faded.
        let mut buf = vec![0xFFu8; 1024];
        wave_apply(&mut buf, 0);
        let bits: u32 = buf.iter().map(|b| b.count_ones() as u32).sum();
        assert_eq!(bits, 128, "64 columns x 2 pixels (wave + mirror)");
        // exactly two pixels per column — the wave and its mirror cross in
        // the middle band (y and 127-y are both in 40..87)
        for x in 0..64usize {
            let col_bits: u32 = (0..16usize)
                .map(|s| buf[s * 64 + x].count_ones() as u32)
                .sum();
            assert_eq!(col_bits, 2, "column {x}: wave + mirror = 2 pixels");
        }
        // pattern is sparse (single bits), unlike the fade bands (0x00/0xFF)
        assert!(
            buf.iter().filter(|b| **b != 0).any(|b| *b & (b - 1) != 0 || *b == 1),
            "wave bytes are bit patterns, not whole-byte bands"
        );
    }

    #[test]
    fn test_wave_period_and_flow() {
        let mut a = vec![0u8; 1024];
        let mut b = vec![0u8; 1024];
        wave_apply(&mut a, 0);
        wave_apply(&mut b, 16); // angle advances 4/64 per phase -> period 16
        assert_eq!(a, b, "period 16");
        let mut c = vec![0u8; 1024];
        wave_apply(&mut c, 1);
        assert_ne!(a, c, "wave flows between consecutive phases");
    }

    #[test]
    fn test_wave_pixels_stay_in_band_range() {
        // y = 64 ± 23 -> rows 41..87, so set bytes only in strips 5..=10 and
        // never outside the buffer.
        for phase in 0..64u32 {
            let mut buf = vec![0u8; 1024];
            wave_apply(&mut buf, phase);
            for (i, &byte) in buf.iter().enumerate() {
                if byte != 0 {
                    let strip = i >> 6;
                    assert!(
                        (5..=10).contains(&strip),
                        "phase {phase}: pixel outside the wave band (byte {i:#x}, strip {strip})"
                    );
                }
            }
        }
    }

    /// Every effect is now a self-contained drawing: it must CLEAR the stock
    /// content (so no charge-screen text survives) and then paint its own
    /// pixels. Asserted on the real charge-screen geometry (64x128).
    #[test]
    fn test_every_effect_draws_its_own_content() {
        for phase in 0..64 {
            for effect in ["gradient", "center", "diagonal", "wave"] {
                let mut buf = vec![0xFFu8; 1024];
                apply(effect, &mut buf, phase);
                assert!(
                    buf.iter().any(|b| *b != 0xFF),
                    "{effect} phase {phase}: stock content must not survive"
                );
                assert!(
                    buf.iter().any(|b| *b != 0),
                    "{effect} phase {phase}: effect must draw something"
                );
            }
        }
    }

    /// The anti-"manipulated text" property: an effect clears the stock screen
    /// before drawing, so the output cannot depend on what was on-screen.
    #[test]
    fn test_draw_is_independent_of_stock_content() {
        for phase in 0..64 {
            for effect in ["gradient", "center", "diagonal", "wave"] {
                // Same phase, two very different starting screens (all-on and
                // all-off). A clearing effect must land on the same frame both
                // times: nothing of the input can influence the result, so the
                // stock charge text can never be recognised in the output.
                let mut on = vec![0xFFu8; 1024];
                let mut off = vec![0u8; 1024];
                apply(effect, &mut on, phase);
                apply(effect, &mut off, phase);
                assert_eq!(
                    on, off,
                    "{effect} phase {phase}: output depends on the stock screen (not a clean draw)"
                );
                assert!(
                    on.iter().any(|b| *b != 0),
                    "{effect} phase {phase}: effect drew nothing"
                );
            }
        }
    }

    #[test]
    fn test_gradient_bar_sweeps_vertically() {
        // The bar is a solid horizontal band: every set row spans all 64
        // columns, and the band's top row advances with phase.
        let bar_rows = |phase: u32| -> Vec<usize> {
            let mut buf = vec![0u8; 1024];
            gradient_fade_apply(&mut buf, 64, 128, phase);
            (0..FB_HEIGHT)
                .filter(|&y| {
                    (0..FB_WIDTH).all(|x| {
                        let idx = ((y >> 3) << 6) + x;
                        buf[idx] & (1 << (y & 7)) != 0
                    })
                })
                .collect()
        };
        let rows0 = bar_rows(0);
        assert_eq!(rows0.len(), BAR_THICKNESS, "bar thickness is constant");
        // contiguous rows (wrapping is possible, so check the cyclic run)
        let set: Vec<bool> = {
            let mut v = vec![false; FB_HEIGHT];
            for y in &rows0 {
                v[*y] = true;
            }
            v
        };
        let runs = set.iter().filter(|b| **b).count();
        assert_eq!(runs, BAR_THICKNESS);
        // phase 4 (>> 2 = 1, << 3 = 8) moves the top row down by 8
        assert_eq!(bar_rows(4)[0], 8, "bar top follows ((phase>>2)<<3) & 127");
        assert_ne!(rows0, bar_rows(4), "bar must move between phases");
    }

    #[test]
    fn test_gradient_bar_period() {
        // ((phase >> 2) << 3) & 127 has period 64 phases (top wraps at 128).
        let mut a = vec![0u8; 1024];
        let mut b = vec![0u8; 1024];
        gradient_fade_apply(&mut a, 64, 128, 0);
        gradient_fade_apply(&mut b, 64, 128, 64);
        assert_eq!(a, b, "bar sweep period is 64 phases");
    }

    #[test]
    fn test_center_rings_are_symmetric_and_pulse() {
        let mut buf = vec![0u8; 1024];
        center_pulse_apply(&mut buf, 64, 0);
        // Ring pixels are vertically mirrored about y = 64: for every set
        // pixel at row y there is one at 2*64 - y = 128 - y (row 64 is its
        // own mirror, which is why the check uses 128 - y, not 127 - y).
        for x in 0..FB_WIDTH {
            let col: Vec<usize> = (0..FB_HEIGHT)
                .filter(|&y| buf[((y >> 3) << 6) + x] & (1 << (y & 7)) != 0)
                .collect();
            for &y in &col {
                assert!(
                    col.contains(&(2 * 64 - y)),
                    "ring not mirrored at column {x}: row {y} has no mirror at {}",
                    2 * 64 - y
                );
            }
        }
        // The ring grows with phase, so consecutive steps differ.
        let mut a = vec![0u8; 1024];
        let mut b = vec![0u8; 1024];
        center_pulse_apply(&mut a, 64, 0);
        center_pulse_apply(&mut b, 64, 1);
        assert_ne!(a, b, "ring must expand between phases");
        // and the radius cycle is 16 phases
        let mut c = vec![0u8; 1024];
        center_pulse_apply(&mut c, 64, RING_STEPS);
        assert_eq!(a, c, "ring period is RING_STEPS phases");
    }

    #[test]
    fn test_diagonal_bar_marches_corner_to_corner() {
        // Diagonals advance by 6 rows per phase and wrap after 192/6 = 32.
        let mut prev: Option<Vec<u8>> = None;
        for phase in 0..32 {
            let mut buf = vec![0u8; 1024];
            diagonal_sweep_apply(&mut buf, 64, phase);
            if let Some(p) = &prev {
                assert_ne!(*p, buf, "diagonal must move between phases");
            }
            prev = Some(buf);
        }
        // period: c wraps mod 192, and 192 | (6 * 32)
        let mut a = vec![0u8; 1024];
        let mut b = vec![0u8; 1024];
        diagonal_sweep_apply(&mut a, 64, 0);
        diagonal_sweep_apply(&mut b, 64, 32);
        assert_eq!(a, b, "diagonal period is 32 phases");
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
            build_wave_patch,
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

    #[test]
    fn test_emit_set_px() {
        use crate::firmware::emu::bus::{Bus, RAM_BASE};
        use crate::firmware::emu::cpu::Cpu;

        const CODE: u32 = 0x8000;
        const DONE: u32 = 0x104;
        const GUARD: usize = 16;
        const FRAME_LEN: usize = 64 * 128 / 8;
        const RAM_LEN: usize = GUARD + FRAME_LEN + GUARD;
        const BUFFER: u32 = RAM_BASE + GUARD as u32;

        // Exercise both calling conventions without an effect's prologue or
        // epilogue hiding register clobbers or framebuffer mistakes.
        for x_reg in [0u8, 2] {
            let mut asm = Asm::new(CODE);
            emit_set_px(&mut asm, 3, x_reg);
            asm.b_abs(DONE);
            let body = asm.finish().unwrap();
            let mut flash = vec![0u8; CODE as usize + body.len()];
            flash[CODE as usize..].copy_from_slice(&body);

            // All rows cover every bit position and every strip transition,
            // including 7/8, 63/64 and 119/120, through the final row 127.
            for y in 0..128usize {
                for x in [0usize, 1, 31, 32, 62, 63] {
                    let index = x + 64 * (y / 8);
                    let mask = 1u8 << (y % 8);
                    // Alternating bits catch destructive stores; zero catches
                    // missing/extra bits; full bytes catch toggling set bits.
                    for initial_byte in [0x00u8, 0x55, 0xAA, 0xFF] {
                        let context = format!("x=r{x_reg}, ({x},{y}), initial={initial_byte:#04x}");
                        let mut expected: Vec<u8> = (0..RAM_LEN)
                            .map(|i| (i as u8).wrapping_mul(37) ^ 0xA5)
                            .collect();
                        expected[GUARD + index] = initial_byte;
                        let mut bus = Bus::new(flash.clone(), RAM_LEN);
                        bus.allow_region(RAM_BASE..RAM_BASE + RAM_LEN as u32);
                        for (i, &byte) in expected.iter().enumerate() {
                            bus.write_u8(RAM_BASE + i as u32, byte).unwrap();
                        }
                        expected[GUARD + index] |= mask;

                        let mut cpu = Cpu::new();
                        cpu.r[..8].copy_from_slice(&[
                            0x1234_5678, BUFFER, 0x2345_6789, y as u32,
                            0x4567_89AB, 0x5678_9ABC, 0x6789_ABCD, 0x789A_BCDE,
                        ]);
                        cpu.r[x_reg as usize] = x as u32;
                        let preserved = cpu.r[..5].to_vec();
                        cpu.pc = CODE;
                        for _ in 0..64 {
                            cpu.step(&mut bus)
                                .unwrap_or_else(|err| panic!("{context}: emulator error: {err:?}"));
                            if cpu.pc == DONE {
                                break;
                            }
                        }
                        assert_eq!(cpu.pc, DONE, "{context}: helper must finish");
                        for (i, &byte) in expected.iter().enumerate() {
                            assert_eq!(
                                bus.read_u8(RAM_BASE + i as u32).unwrap(), byte,
                                "{context}: RAM offset {i:#x} (includes framebuffer guards)"
                            );
                        }
                        assert_eq!(&cpu.r[..5], preserved.as_slice(), "{context}: preserve r0-r4");
                        assert!(bus.acl_violations.is_empty(), "{context}: out-of-region store");
                        assert!(bus.dropped_writes.is_empty(), "{context}: peripheral store");
                    }
                }
            }
        }
    }

    /// Each invocation must draw the reference frame, advance phase once, and
    /// resume stock code without stores outside the phase word/framebuffer.
    fn assert_cave_matches_reference(effect: &str, emit: EmitFn, config: u8) {
        use crate::firmware::emu::bus::{Bus, RAM_BASE};
        use crate::firmware::emu::cpu::Cpu;

        const BUFFER: u32 = 0x20001000;
        const FRAME_LEN: u32 = 0x400;
        const GUARD: u32 = 64;
        const BUDGET: usize = 100_000;
        const STATUS: u32 = 0x8002_0045; // timeout plus unrelated status bits

        let anim = load_animation_desc(DESC).unwrap();
        let mut asm = Asm::new(anim.code_cave.start);
        emit(&mut asm, &anim, BUFFER, FRAME_LEN).unwrap();
        let body = asm.finish().unwrap();
        let mut flash = vec![0u8; 0x9000];
        let code = anim.code_cave.start as usize;
        flash[code..code + body.len()].copy_from_slice(&body);

        // Two full 64-phase gradient cycles also cover the shorter center,
        // diagonal and wave cycles, including both sides of every wrap.
        for phase in (1..=128u32).chain([0x0010_0001, 0x2000_001f]) {
            for initial in [0x00u8, 0xFF] {
                let context = format!("{effect}: phase={phase}, initial={initial:#04x}");
                let mut bus = Bus::new(
                    flash.clone(), (BUFFER + FRAME_LEN + GUARD - RAM_BASE) as usize,
                );
                bus.allow_region(anim.phase_global..anim.phase_global + 4);
                bus.allow_region(BUFFER..BUFFER + FRAME_LEN);
                bus.set_stub(anim.config_byte_addr, config as u32);
                bus.write_u32(anim.phase_global, phase - 1).unwrap();
                // Setup writes land even outside the ACL. Do not grant the
                // cave permission to write status or either framebuffer guard.
                bus.write_u32(anim.status_word_addr, STATUS).unwrap();
                for i in 0..GUARD {
                    bus.write_u8(BUFFER - GUARD + i, 0xA5).unwrap();
                    bus.write_u8(BUFFER + FRAME_LEN + i, 0x5A).unwrap();
                }
                for i in 0..FRAME_LEN {
                    bus.write_u8(BUFFER + i, initial).unwrap();
                }
                bus.acl_violations.clear();
                bus.write_log.clear();
                assert!(bus.dropped_writes.is_empty(), "{context}: invalid setup");

                let mut cpu = Cpu::new();
                cpu.r[2] = 0x20000; // active hook; exit must re-read the full status
                cpu.r[3] = anim.status_base_addr;
                cpu.pc = anim.code_cave.start;
                for _ in 0..BUDGET {
                    cpu.step(&mut bus)
                        .unwrap_or_else(|err| panic!("{context}: emulator error: {err:?}"));
                    if cpu.pc == anim.hook_resume {
                        break;
                    }
                }
                assert_eq!(
                    cpu.pc, anim.hook_resume,
                    "{context}: must resume within {BUDGET} instructions"
                );
                assert!(
                    bus.acl_violations.is_empty(),
                    "{context}: out-of-region stores: {:?}", bus.acl_violations
                );
                assert!(
                    bus.dropped_writes.is_empty(),
                    "{context}: dropped stores: {:?}", bus.dropped_writes
                );
                // Check full store extents, not just their starting addresses.
                let phase_writes: Vec<_> = bus.write_log.iter().filter(|w| {
                    !(w.addr >= BUFFER
                        && u64::from(w.addr) + u64::from(w.size)
                            <= u64::from(BUFFER + FRAME_LEN))
                }).collect();
                assert_eq!(phase_writes.len(), 1, "{context}: exactly one non-frame store");
                let write = phase_writes[0];
                assert_eq!(
                    (write.addr, write.size, write.value), (anim.phase_global, 4, phase),
                    "{context}: only non-frame store must advance phase once"
                );
                assert_eq!(bus.read_u32(anim.phase_global).unwrap(), phase, "{context}: phase");
                assert_eq!(bus.read_u32(anim.status_word_addr).unwrap(), STATUS, "{context}: status unchanged");
                assert_eq!(cpu.r[2], STATUS, "{context}: restore full status word in r2");
                assert_eq!(cpu.r[3], anim.status_base_addr, "{context}: restore status base in r3");
                assert!(!cpu.z, "{context}: timeout TST must restore Z=0");
                assert!(!cpu.n, "{context}: timeout TST must restore N=0");
                for i in 0..GUARD {
                    assert_eq!(bus.read_u8(BUFFER - GUARD + i).unwrap(), 0xA5, "{context}: leading guard {i}");
                    assert_eq!(bus.read_u8(BUFFER + FRAME_LEN + i).unwrap(), 0x5A, "{context}: trailing guard {i}");
                }
                let mut expect = vec![initial; FRAME_LEN as usize];
                apply(effect, &mut expect, phase);
                let got: Vec<u8> = (0..FRAME_LEN)
                    .map(|i| bus.read_u8(BUFFER + i).unwrap())
                    .collect();
                assert_eq!(got, expect, "{context}: emitted cave must match the Rust reference");
            }
        }
    }

    #[test]
    fn test_emitted_caves_gradient_matches_reference_in_emu() {
        assert_cave_matches_reference("gradient", emit_gradient_fade, CONFIG_GRADIENT_FADE);
    }

    #[test]
    fn test_emitted_caves_center_matches_reference_in_emu() {
        assert_cave_matches_reference("center", emit_center_pulse, CONFIG_CENTER_PULSE);
    }

    #[test]
    fn test_emitted_caves_diagonal_matches_reference_in_emu() {
        assert_cave_matches_reference("diagonal", emit_diagonal_sweep, CONFIG_DIAGONAL_SWEEP);
    }

    #[test]
    fn test_emitted_caves_wave_matches_reference_in_emu() {
        assert_cave_matches_reference("wave", emit_wave, CONFIG_WAVE);
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
