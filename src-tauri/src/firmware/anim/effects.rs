//! Animation effects: pure Rust references + Thumb-1 cave emission.
//!
//! A family of "fade" effects for the timed-out screen: a 16-entry sine LUT
//! maps each framebuffer byte's band index to a brightness; bytes below the
//! half-amplitude threshold are cleared in place. Integer math only.
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
//! phase global, fades the display buffer in place, and resumes.
//!
//! The config byte (a spare dataflash byte) selects the effect:
//! 2 = Gradient Fade, 3 = Center Pulse, 4 = Diagonal Sweep.

use serde::Deserialize;

use crate::firmware::anim::asm::{bw_pair, AnimError, Asm, AsmError};
use crate::firmware::emu::harness::de_hex;
use crate::firmware::patch::{Patch, PatchModification};

/// 16-entry quarter-wave sine LUT, values 0..=255.
pub const SINE_LUT: [u8; 16] = [0, 25, 50, 74, 98, 120, 142, 162, 180, 197, 212, 225, 236, 245, 251, 255];

/// Half-amplitude threshold: bytes in bands darker than this are cleared.
const THRESHOLD: u8 = 128;

/// Config-byte values selecting each effect.
pub const CONFIG_GRADIENT_FADE: u8 = 2;
pub const CONFIG_CENTER_PULSE: u8 = 3;
pub const CONFIG_DIAGONAL_SWEEP: u8 = 4;

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
/// Each 64-byte strip shares one brightness value:
/// `SINE_LUT[((strip*8 + phase) >> 2) & 15]`; the `>> 2` keeps the
/// phase→strip mapping expressible in Thumb shifts (period 64 phases).
pub fn gradient_fade_apply(buf: &mut [u8], width: usize, _height: usize, phase: u32) {
    fade_apply(buf, width, phase, |i, ph| {
        (((i / width) as u32 * 8 + ph) >> 2) as usize
    })
}

/// Center Pulse (config 3): brightness rings pulse outward from the vertical
/// centre of the screen. Strip distance from the centre is `strip ^ 7`
/// (strips 7/8 → 0, edges → 15); period 16 phases.
pub fn center_pulse_apply(buf: &mut [u8], width: usize, phase: u32) {
    fade_apply(buf, width, phase, |i, ph| {
        let d = (i / width) as u32 ^ 7;
        d.wrapping_add(ph) as usize
    })
}

/// Diagonal Sweep (config 4): diagonal bands march corner to corner. The LUT
/// index combines the strip, the byte's column group `((i & 63) >> 2)`, and
/// the phase; period 16 phases.
pub fn diagonal_sweep_apply(buf: &mut [u8], _width: usize, phase: u32) {
    fade_apply(buf, 64, phase, |i, ph| {
        let xg = ((i & 63) >> 2) as u32;
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
        a.lsrs(3, 0, 6); // i >> 6        (byte strip = 64 bytes)
        a.lsls(3, 3, 3); // * 8           (y of the strip's pixel group)
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
        a.lsrs(3, 0, 6); // strip s
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
        a.lsls(3, 0, 26); // i << 26
        a.lsrs(3, 3, 28); // >> 28       (column group = (i & 63) >> 2)
        a.lsrs(2, 0, 6); // strip s
        a.adds_reg(3, 3, 2); // strip + column group
        a.adds_reg(3, 3, 4); // + phase   (period 16)
        a.ands(3, 6); // & 15
    })
}

/// Builds the full patch: 4-byte B.W detour at `hook_site` + cave body at
/// `code_cave.start`. `desc_json` is the full descriptor document.
fn build_effect_patch(
    desc_json: &str,
    id: &str,
    name: &str,
    description: &str,
    emit: EmitFn,
) -> Result<Patch, AnimError> {
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
        emit_gradient_fade,
    )
}

pub fn build_center_pulse_patch(desc_json: &str) -> Result<Patch, AnimError> {
    build_effect_patch(
        desc_json,
        "anim-center-pulse",
        "Center Pulse (charge-screen animation)",
        "Brightness rings pulse outward from screen centre on timeout (config byte 3)",
        emit_center_pulse,
    )
}

pub fn build_diagonal_sweep_patch(desc_json: &str) -> Result<Patch, AnimError> {
    build_effect_patch(
        desc_json,
        "anim-diagonal-sweep",
        "Diagonal Sweep (charge-screen animation)",
        "Diagonal bands sweep the screen on timeout (config byte 4)",
        emit_diagonal_sweep,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESC: &str = r#"{
        "build": "t",
        "render_entry": "0x0",
        "display_buffer": {"start": "0x20001000", "end": "0x20001400",
                           "width": 64, "height": 128},
        "animation": { "hook_site": "0x100", "hook_resume": "0x104",
            "code_cave": {"start": "0x8000", "size": 512},
            "config_byte_addr": "0x1F100", "phase_global": "0x20000000",
            "status_word_addr": "0x20000008",
            "status_base_addr": "0x20000004" }
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
        // cave body starts at 0x8000, fits in 512 bytes
        let max = p.modifications.iter().map(|m| m.offset).max().unwrap();
        assert!(max < 0x8000 + 512);
    }

    #[test]
    fn test_all_builders_fit_cave() {
        for build in [
            build_gradient_fade_patch as fn(&str) -> Result<Patch, AnimError>,
            build_center_pulse_patch,
            build_diagonal_sweep_patch,
        ] {
            let p = build(DESC).unwrap();
            let max = p.modifications.iter().map(|m| m.offset).max().unwrap();
            assert!(max < 0x8000 + 512, "{} overflows cave", p.id);
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
