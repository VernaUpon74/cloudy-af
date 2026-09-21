//! Task 7 integration tests: a synthetic two-function "firmware" (a render
//! function with a zeroing loop plus a BL'd helper that bumps a phase global),
//! a hook patch applied through the real `patch::apply_patch` that detours
//! through a code cave, and the negative case (a corrupted cave literal turns
//! the hook's store into an ACL violation).

use std::collections::HashMap;

use crate::firmware::emu::bus::RAM_BASE;
use crate::firmware::emu::harness::{load_descriptor, Harness};
use crate::firmware::emu::EmuError;
use crate::firmware::patch::{apply_patch, Patch, PatchModification};

const DISPLAY_BUFFER: u32 = RAM_BASE + 0x1000;
const PHASE_GLOBAL: u32 = RAM_BASE;

/// Mirrors the DESC_JSON in harness.rs tests, but render_entry = 0x00.
const SYNTH_DESC: &str = r#"{
    "build": "synthetic",
    "render_entry": "0x00000000",
    "display_buffer": { "start": "0x20001000", "end": "0x20001400", "width": 64, "height": 128 },
    "ram_globals": [ { "start": "0x20000000", "end": "0x20000100" } ],
    "ram_size": 32768,
    "args": [0, 0, 0, 0]
}"#;

/// Build an unconditional-branch halfword: `b to` assembled at `from`.
/// pc = from + 4 + off, off = imm11 * 2 (sign-extended).
fn b_off(from: u32, to: u32) -> u16 {
    let off = (to as i64 - (from as i64 + 4)) / 2;
    0xE000 | ((off as i32 as u16) & 0x7FF)
}

/// Synthetic image.
///
/// Render function at 0x00: push {r4, lr}; zero the first 16 bytes of the
/// display buffer (strb/adds/cmp/bne loop); bl helper at 0x40; hook site at
/// 0x20 (two nops); store the returned pixel byte into display_buffer[0];
/// pop {r4, pc}. Literal pool (display buffer addr) at 0x2C.
///
/// Helper at 0x40: increments the phase global at RAM_BASE, returns
/// pixel = phase & 1 in r0. Literal pool (phase global addr) at 0x50.
///
/// Code cave at 0x60: 32 bytes of zeros, filled by the hook patch.
fn synthetic_image() -> Vec<u8> {
    let mut img = vec![0u8; 0x100];
    let code: [(u32, u16); 30] = [
        (0x00, 0xB510), // push {r4, lr}
        (0x02, 0x490A), // ldr r1, [pc, #40] ; base=align(0x06,4)=0x04, +0x28=0x2C -> &display_buffer
        (0x04, 0x2200), // movs r2, #0      ; loop counter
        (0x06, 0x2300), // movs r3, #0      ; zero value
        (0x08, 0x548B), // strb r3, [r1, r2] ; loop: display_buffer[r2] = 0
        (0x0A, 0x3201), // adds r2, #1
        (0x0C, 0x2A10), // cmp r2, #16
        (0x0E, 0xD1FB), // bne 0x08         ; off = 0x08 - (0x0E+4) = -10 -> imm8 0xFB
        (0x10, 0xF000), // bl 0x40 (hw1)    ; S=0, imm10=0
        (0x12, 0xF816), // bl 0x40 (hw2)    ; J1=J2=1, imm11=22 -> off=44: 0x10+4+44=0x40
        (0x14, 0x4905), // ldr r1, [pc, #20] ; base=align(0x18,4)=0x18, +0x14=0x2C (helper clobbers r1)
        (0x16, 0x46C0), // nop
        (0x18, 0x46C0), // nop
        (0x1A, 0x46C0), // nop
        (0x1C, 0x46C0), // nop
        (0x1E, 0x46C0), // nop
        (0x20, 0x46C0), // nop  <- hook site (patch writes `b 0x60` here)
        (0x22, 0x46C0), // nop  <- hook site (patch writes filler nop here)
        (0x24, 0x7008), // strb r0, [r1, #0] ; display_buffer[0] = pixel
        (0x26, 0xBD10), // pop {r4, pc}
        (0x28, 0x46C0), // nop (pad)
        (0x2A, 0x46C0), // nop (pad)
        // 0x2C: literal pool, filled below
        (0x40, 0x4803), // helper: ldr r0, [pc, #12] ; base=align(0x44,4)=0x44, +0xC=0x50 -> &phase
        (0x42, 0x6801), // ldr r1, [r0, #0]
        (0x44, 0x3101), // adds r1, #1
        (0x46, 0x6001), // str r1, [r0, #0]  ; phase++
        (0x48, 0x2001), // movs r0, #1
        (0x4A, 0x4008), // ands r0, r1       ; pixel = phase & 1
        (0x4C, 0x4770), // bx lr
        (0x4E, 0x46C0), // nop (pad)
        // 0x50: literal pool, filled below
    ];
    for (off, hw) in code {
        img[off as usize..off as usize + 2].copy_from_slice(&hw.to_le_bytes());
    }
    img[0x2C..0x30].copy_from_slice(&DISPLAY_BUFFER.to_le_bytes());
    img[0x50..0x54].copy_from_slice(&PHASE_GLOBAL.to_le_bytes());
    img
}

/// Hook patch: at 0x20 branch into the code cave at 0x60 (plus a filler nop);
/// in the cave, store 0xFF into display_buffer byte 15 and branch back to 0x24.
/// `display_addr` feeds the cave's literal pool entry at 0x6C — pass a bad
/// address to simulate a buggy patch.
fn hook_patch(display_addr: u32) -> Patch {
    let mut mods = Vec::new();
    let mut push = |addr: u32, hw: u16| {
        let b = hw.to_le_bytes();
        mods.push(PatchModification { offset: addr as usize, original: None, patched: b[0] });
        mods.push(PatchModification { offset: addr as usize + 1, original: None, patched: b[1] });
    };
    push(0x20, b_off(0x20, 0x60)); // hook: branch into cave
    push(0x22, 0x46C0); // nop filler
    push(0x60, 0x20FF); // movs r0, #0xFF
    push(0x62, 0x4902); // ldr r1, [pc, #8] ; base=align(0x66,4)=0x64, +8=0x6C -> literal
    push(0x64, 0x73C8); // strb r0, [r1, #15] ; last byte of the first 16
    push(0x66, b_off(0x66, 0x24)); // branch back after the hook site
    push(0x68, 0x46C0); // nop (keeps pc-relative math simple)
    for (i, b) in display_addr.to_le_bytes().iter().enumerate() {
        mods.push(PatchModification { offset: 0x6C + i, original: None, patched: *b });
    }
    Patch { id: "anim-hook".into(), name: "anim".into(), modifications: mods, ..Default::default() }
}

#[test]
fn test_hooked_render_frame() {
    let mut img = synthetic_image();
    let mut patch = hook_patch(DISPLAY_BUFFER);
    let mut log = HashMap::new();
    apply_patch(&mut img, &mut patch, &mut log).unwrap();
    assert!(patch.applied);

    let d = load_descriptor(SYNTH_DESC).unwrap();
    let mut h = Harness::new(&img, d);
    let frame = h.run_frame(100_000).expect("hooked render must not fault");

    // Cave stored 0xFF into display_buffer byte 15 = row y=1, byte-column 7
    // (stride 8); all 8 bits -> pixels (56..=63, 1). Frame.pixels is
    // row-major: pixels[y * width + x], width 64.
    for x in 56..64 {
        assert_eq!(frame.pixels[64 + x], 1, "pixel ({x}, 1) should be on");
    }
    assert_eq!(frame.pixels[16], 0, "pixel (16, 0) untouched");
}

#[test]
fn test_bad_patch_acl_violation() {
    let mut img = synthetic_image();
    // Corrupt cave literal: store target lands in RAM that the descriptor
    // does not allow (globals end at +0x100, stack starts at +0x1000).
    let mut patch = hook_patch(RAM_BASE + 0x0800);
    let mut log = HashMap::new();
    apply_patch(&mut img, &mut patch, &mut log).unwrap();

    let d = load_descriptor(SYNTH_DESC).unwrap();
    let mut h = Harness::new(&img, d);
    let err = h.run_frame(100_000).unwrap_err();
    assert!(
        matches!(err, EmuError::Descriptor(ref msg) if msg.contains("ACL violations")),
        "expected ACL violation error, got: {err}"
    );
}

#[test]
fn test_two_frames_differ_via_phase() {
    let d = load_descriptor(SYNTH_DESC).unwrap();
    let mut h = Harness::new(&synthetic_image(), d);
    // Frame 1: phase 0 -> 1, pixel = 1 -> display_buffer[0] = 0x01. Under
    // horizontal MSB-first packing bit 0 is x=7, so (7,0) is on.
    let f1 = h.run_frame(100_000).expect("frame 1 must not fault");
    // Frame 2: phase 1 -> 2, pixel = 0 -> (7,0) off again.
    let f2 = h.run_frame(100_000).expect("frame 2 must not fault");
    assert_eq!(f1.pixels[7], 1);
    assert_eq!(f2.pixels[7], 0);
    assert!(Harness::frames_differ(&f1, &f2));
}

/// LAYER-5 PRE-FLASH GATE. Requires:
///   - resources/animations/af_190602.json (Phase 3 RE output)
///   - AF_fw/decrypted/af_190602.dec.bin (gitignored RE artifact)
/// Both absent in CI/other machines -> skip. Run explicitly before flashing
/// any animation image:
///   cargo test --offline --lib firmware::tests::emu_test -- --ignored
#[test]
#[ignore]
fn test_af_190602_render_gate() {
    use std::path::Path;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let desc_path = root.join("resources/animations/af_190602.json");
    let img_path = root.join("AF_fw/decrypted/af_190602.dec.bin");
    if !desc_path.exists() || !img_path.exists() {
        eprintln!("descriptor or decrypted image missing; skipping gate");
        return;
    }
    let desc = load_descriptor(&std::fs::read_to_string(&desc_path).unwrap()).unwrap();
    let img = std::fs::read(&img_path).unwrap();
    let mut h = Harness::new(&img, desc);
    let mut prev = None;
    for frame_no in 0..4 {
        let frame = h.run_frame(5_000_000)
            .unwrap_or_else(|e| panic!("frame {frame_no}: {e}\n{}", h.cpu.debug_dump()));
        if let Some(p) = &prev {
            // stock charge screen may be static; just record. Animation patches
            // assert difference in the effect tests (later phase).
            let _ = Harness::frames_differ(p, &frame);
        }
        // Golden assertion: the stock charge screen renders 409 on-pixels per
        // frame (eyeball-validated against a hardware capture). Catches any
        // emulator regression that executes without faulting but renders
        // garbage.
        let on = frame.pixels.iter().filter(|&&px| px != 0).count();
        assert_eq!(on, 409, "frame {frame_no}: on-pixel count drifted");
        prev = Some(frame);
    }
}

/// Uniform reference-apply shims for the effect table (width 64, height 128
/// per the af_190602 descriptor's display_buffer).
fn apply_gradient(buf: &mut [u8], phase: u32) {
    crate::firmware::anim::effects::gradient_fade_apply(buf, 64, 128, phase);
}
fn apply_center(buf: &mut [u8], phase: u32) {
    crate::firmware::anim::effects::center_pulse_apply(buf, 64, phase);
}
fn apply_diagonal(buf: &mut [u8], phase: u32) {
    crate::firmware::anim::effects::diagonal_sweep_apply(buf, 64, phase);
}

/// Run the emulator from `entry` until the return sentinel, like
/// `Harness::run_frame` but with an explicit entry point (the stock render
/// entry is not the only way into the patched code path).
fn run_at(h: &mut Harness, entry: u32, budget: u64) {
    use crate::firmware::emu::harness::RETURN_SENTINEL;
    h.cpu.pc = entry & !1;
    h.cpu.lr = RETURN_SENTINEL | 1;
    h.cpu
        .run_until(&mut h.bus, RETURN_SENTINEL, budget)
        .unwrap_or_else(|e| panic!("run_at {entry:#x}: {e}\n{}", h.cpu.debug_dump()));
    assert!(
        h.bus.acl_violations.is_empty(),
        "ACL violations: {:?}",
        h.bus.acl_violations
    );
}

/// LAYER-5 PRE-FLASH GATE: patched af_190602 animates each effect in the emu.
/// Requires the same artifacts as the layer-4 gate.
///
/// Drive pattern: prime the display buffer with the real charge-screen render
/// (timeout bit clear), then raise the timeout bit 0x20000 in the
/// display-status word [0x20002c30+4] (the bit the hook at 0x9a16 tests) and
/// call the clock renderer FUN_00009a10 directly — the host of the patched
/// gate — four times. RE (af_190602-dispatcher-analy.md §7.1) shows the
/// timeout branch at 0x9a20 reaches the pop-only screen-off path at 0x9b48
/// only when status bit 19 (0x80000) is also set (bmi.w at 0x9a24); with
/// both bits set the stock code draws nothing, so the framebuffer evolves
/// purely by the cave's clearing fade: after call k the raw buffer must
/// equal the reference fade of the primed buffer at phase k, and the phase
/// global must read k.
///
/// Hook site: `0x9a16` (timeout decision point in `FUN_00009a10`).
/// This test exercises the charge-screen timeout effects (CONFIG_GRADIENT_FADE
/// through CONFIG_RIPPLING_WAVE). The clock animation (CONFIG_CLOCK_ANIMATION = 6)
/// uses a separate hook site (clock drawing path at `0x9a32`) and is NOT covered
/// by this test — see DEFERRED clock hook comment in effects.rs.
#[test]
#[ignore]
fn test_af_190602_animation_frames() {
    use std::path::Path;

    use crate::firmware::anim::asm::AnimError;
    use crate::firmware::anim::effects::{
        build_center_pulse_patch, build_diagonal_sweep_patch, build_gradient_fade_patch,
        CONFIG_CENTER_PULSE, CONFIG_DIAGONAL_SWEEP, CONFIG_GRADIENT_FADE,
    };
    use crate::firmware::emu::harness::dump_pgm;

    const CLOCK_RENDERER: u32 = 0x9a10; // FUN_00009a10, host of the hook
    const PHASE_GLOBAL: u32 = 0x2000_2cf8; // descriptor animation.phase_global
    // Display-status word [0x20002c30 + 4] (composer literal 0xb698 /
    // clock-renderer literal 0x9b4c, RE doc §7.1); bit 0x20000 = timed out.
    const STATUS_WORD: u32 = 0x2000_2c34;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let desc_path = root.join("resources/animations/af_190602.json");
    let img_path = root.join("AF_fw/decrypted/af_190602.dec.bin");
    if !desc_path.exists() || !img_path.exists() {
        eprintln!("descriptor or decrypted image missing; skipping gate");
        return;
    }
    let desc_json = std::fs::read_to_string(&desc_path).unwrap();
    let stock_img = std::fs::read(&img_path).unwrap();

    // Self-goldening: first run writes AF_fw/goldens/<effect>_phase<k>.bin
    // (gitignored); later runs compare against it, catching silent drift of
    // emitter/emulator/descriptor across commits.
    let goldens = root.join("AF_fw/goldens");
    std::fs::create_dir_all(&goldens).unwrap();

    let table: [(
        &str,
        fn(&str) -> Result<Patch, AnimError>,
        u8,
        fn(&mut [u8], u32),
    ); 3] = [
        ("gradient", build_gradient_fade_patch, CONFIG_GRADIENT_FADE, apply_gradient),
        ("center", build_center_pulse_patch, CONFIG_CENTER_PULSE, apply_center),
        ("diagonal", build_diagonal_sweep_patch, CONFIG_DIAGONAL_SWEEP, apply_diagonal),
    ];

    for (name, build, config, apply) in table {
        let mut img = stock_img.clone();
        let mut patch = build(&desc_json).unwrap();
        let mut log = HashMap::new();
        apply_patch(&mut img, &mut patch, &mut log).unwrap();

        let mut h = Harness::new(&img, load_descriptor(&desc_json).unwrap());
        let buf_start = h.desc.display_buffer.range.start;
        let buf_len =
            (h.desc.display_buffer.range.end - h.desc.display_buffer.range.start) as usize;
        let read_buf = |h: &Harness| -> Vec<u8> {
            (0..buf_len as u32)
                .map(|i| h.bus.read_u8(buf_start + i).unwrap())
                .collect()
        };

        // Prime: stock charge screen with the timeout bit clear.
        let prime = h.run_frame(5_000_000).unwrap_or_else(|e| {
            panic!("{name} prime: {e}\n{}", h.cpu.debug_dump())
        });
        let primed = read_buf(&h);

        // Raise the timeout bits the stock dim/idle path needs: 0x20000 makes
        // the hook's `beq` fall through at 0x9a20; 0x80000 (status bit 19)
        // makes the `bmi.w 0x9b48` take the pop-only screen-off branch —
        // without it the code continues to FUN_0000993C, which draws a
        // different idle screen (RE doc §7.1 glossed over this condition).
        // Then select this effect (the descriptor stub defaults to Gradient
        // Fade) and drive the hook host directly.
        let status = h.bus.read_u32(STATUS_WORD).unwrap_or(0);
        h.bus.write_u32(STATUS_WORD, status | 0x20000 | 0x80000).unwrap();
        let cfg_addr = h.desc.stubs[0].addr;
        h.bus.set_stub(cfg_addr, config as u32);

        let (w, hh) = (h.desc.display_buffer.width, h.desc.display_buffer.height);
        let unpack = |raw: &[u8]| crate::firmware::emu::harness::unpack_block1(raw, w, hh);
        let mut prev = prime.clone();
        for k in 1..=4u32 {
            run_at(&mut h, CLOCK_RENDERER, 5_000_000);
            let phase = h.bus.read_u32(PHASE_GLOBAL).unwrap();
            assert_eq!(phase, k, "{name}: phase global must count cave runs");
            let got = read_buf(&h);
            // The emulator is cumulative: each cave run fades the buffer left
            // by the previous run, so the reference must replay phases
            // 1..=k, not just phase k.
            let mut expect = primed.clone();
            for p in 1..=phase {
                apply(&mut expect, p);
            }
            let first_diffs: Vec<String> = got
                .iter()
                .zip(expect.iter())
                .enumerate()
                .filter(|(_, (g, e))| g != e)
                .take(5)
                .map(|(i, (g, e))| format!("{i:#x}: got {g:#x} want {e:#x}"))
                .collect();
            assert_eq!(
                got, expect,
                "{name}: frame {k} must equal the reference fade of the primed buffer (diffs: {})",
                first_diffs.join(", ")
            );
            let golden_path = goldens.join(format!("{name}_phase{k}.bin"));
            if golden_path.exists() {
                let golden = std::fs::read(&golden_path).unwrap();
                assert_eq!(
                    got, golden,
                    "{name}: phase {k} frame drifted from the recorded golden"
                );
            } else {
                std::fs::write(&golden_path, &got).unwrap();
            }
            // Consecutive phases may map to the same bands (e.g. Gradient
            // Fade only shifts bands every 4 phases); the frames must differ
            // exactly when the cumulative reference fade says they should.
            let mut expect_prev = primed.clone();
            for p in 1..phase {
                apply(&mut expect_prev, p);
            }
            if Harness::frames_differ(&unpack(&expect_prev), &unpack(&expect)) {
                assert!(
                    Harness::frames_differ(&prev, &unpack(&got)),
                    "{name}: animation must change frames (phase {phase})"
                );
            }
            prev = unpack(&got);
        }

        // NOTE: no "final frame must differ from prime" assert — where the
        // stock charge screen actually has pixels (strips 0-1) and which
        // bands an effect clears in phases 1-4 are effect properties, not
        // emulator properties (Center Pulse leaves the top strips untouched
        // for these phases; the exact per-phase buffer equality above is
        // the actual gate).

        let final_on = prev.pixels.iter().filter(|&&px| px != 0).count();
        dump_pgm(&prime, Path::new(&format!("/tmp/af_anim_{name}_prime.pgm"))).unwrap();
        dump_pgm(&prev, Path::new(&format!("/tmp/af_anim_{name}_f4.pgm"))).unwrap();
        eprintln!("{name}: ok, final on-pixels = {final_on}");
    }
}

/// LAYER-5 PRE-FLASH GATE: with the config stub deselected (0 = no effect)
/// the patched detour must be transparent. Drive a gradient-patched harness
/// and an unpatched stock harness through the identical sequence (prime,
/// timeout bits, 4× clock renderer) and assert after every call that the
/// framebuffer is byte-exact equal and the phase global stayed 0 — the cave
/// ran but the config gate resumed it before any fade or phase bump.
#[test]
#[ignore]
fn test_af_190602_patched_converges_when_gated_off() {
    use std::path::Path;

    use crate::firmware::anim::effects::build_gradient_fade_patch;

    const CLOCK_RENDERER: u32 = 0x9a10;
    const PHASE_GLOBAL: u32 = 0x2000_2cf8;
    const STATUS_WORD: u32 = 0x2000_2c34;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let desc_path = root.join("resources/animations/af_190602.json");
    let img_path = root.join("AF_fw/decrypted/af_190602.dec.bin");
    if !desc_path.exists() || !img_path.exists() {
        eprintln!("descriptor or decrypted image missing; skipping gate");
        return;
    }
    let desc_json = std::fs::read_to_string(&desc_path).unwrap();
    let stock_img = std::fs::read(&img_path).unwrap();

    let mut patched_img = stock_img.clone();
    let mut patch = build_gradient_fade_patch(&desc_json).unwrap();
    let mut log = HashMap::new();
    apply_patch(&mut patched_img, &mut patch, &mut log).unwrap();

    let mut patched = Harness::new(&patched_img, load_descriptor(&desc_json).unwrap());
    let mut stock = Harness::new(&stock_img, load_descriptor(&desc_json).unwrap());

    let buf_start = patched.desc.display_buffer.range.start;
    let buf_len =
        (patched.desc.display_buffer.range.end - patched.desc.display_buffer.range.start) as usize;
    let read_buf = |h: &Harness| -> Vec<u8> {
        (0..buf_len as u32)
            .map(|i| h.bus.read_u8(buf_start + i).unwrap())
            .collect()
    };

    // Identical drive on both: prime, raise the timeout bits, deselect the
    // effect on the patched harness (stub 0), then call the hook host.
    patched.run_frame(5_000_000).expect("patched prime");
    stock.run_frame(5_000_000).expect("stock prime");
    for h in [&mut patched, &mut stock] {
        let status = h.bus.read_u32(STATUS_WORD).unwrap_or(0);
        h.bus.write_u32(STATUS_WORD, status | 0x20000 | 0x80000).unwrap();
    }
    let cfg_addr = patched.desc.stubs[0].addr;
    patched.bus.set_stub(cfg_addr, 0);

    for call in 1..=4u32 {
        run_at(&mut patched, CLOCK_RENDERER, 5_000_000);
        run_at(&mut stock, CLOCK_RENDERER, 5_000_000);
        assert_eq!(
            read_buf(&patched),
            read_buf(&stock),
            "call {call}: gated-off patched framebuffer diverged from stock"
        );
        assert_eq!(
            patched.bus.read_u32(PHASE_GLOBAL).unwrap(),
            0,
            "call {call}: gated-off cave must not bump the phase global"
        );
    }
}

// == Full-boot per-PID dispatch emulation (validation §2, plan 2026-09-14) ==

/// Build a combined firmware+dataflash image suitable for booting from reset.
///
/// The stock firmware (0x1C7EC bytes) is laid out at 0x00000..0x1C7EB,
/// the APROM tail / gap (0x1C7EC..0x1F000) is padded with 0xFF, and the
/// dataflash (2048 B) is placed at 0x1F000..0x1F7FF. The existing Bus::read
/// flash path serves dataflash reads from this combined image, so no new
/// callback plumbing is needed — dataflash byte 0 is at img[0x1F000] and
/// the PID lives at dataflash offset 316 = img[0x1F13C..0x1F140].
const COMBINED_IMAGE_SIZE: usize = 0x1F800;
/// Boot-gate helper: run the emulator from the reset vector until either the
/// dispatcher is reached (success) or the unknown-PID hang at 0x2FF8 is hit
/// (failure), or the budget is exhausted.
///
/// `stop_pcs` is the set of PCs that count as "dispatcher reached" — the
/// dispatcher is at 0xD684; the render entry (charge-screen composer inner
/// body) is at 0x8CD1. The first run should reveal which of these (if either)
/// the boot path actually reaches, and the set can be refined then.
///
/// Returns `Ok(true)` if a stop_pc was reached, `Ok(false)` if the hang target
/// (0x2FF8) was reached, `Err(...)` on budget/exhaustion/unmapped.
fn boot_until_settle(
    h: &mut Harness,
    budget: u64,
    stop_pcs: &[u32],
    hang_pc: u32,
    ldrom: &[u8],
) -> Result<bool, EmuError> {
    // Set up the CPU for boot: PC = reset handler (the vector-table value
    // carries the Thumb bit — mask it off; Cpu.pc must be the even address
    // the CPU fetches from), SP = initial_sp, LR = RETURN_SENTINEL | 1
    // (safe default; the boot path returns to main which loops forever, but
    // we stop on the signal instead).
    use crate::firmware::emu::harness::RETURN_SENTINEL;
    let reset_vector = 0x00001659u32;
    let initial_sp = 0x200031C0u32;
    h.cpu.pc = reset_vector & !1;
    h.cpu.sp = initial_sp;
    h.cpu.lr = RETURN_SENTINEL | 1;
    h.cpu.min_sp = h.cpu.sp;

    let mut n: u64 = 0;
    loop {
        if stop_pcs.contains(&h.cpu.pc) {
            return Ok(true);
        }
        if h.cpu.pc == hang_pc {
            return Ok(false);
        }
        // Test-only model of the read helper at 0x178c: r0 is a flash
        // address, and the helper returns one LE word without changing flags.
        // Synthetic LDROM is separate from APROM/dataflash; never return a
        // constant PID for unrelated ISP addresses. This bypasses controller
        // timing, not the firmware's device-ID comparison/dispatch branches.
        if h.cpu.pc == 0x178c {
            let addr = h.cpu.r[0];
            h.cpu.r[0] = if (0x100000..0x101000).contains(&addr) {
                let offset = (addr - 0x100000) as usize;
                let bytes = ldrom.get(offset..offset + 4)
                    .ok_or(EmuError::Unmapped { addr })?;
                u32::from_le_bytes(bytes.try_into().unwrap())
            } else {
                h.bus.read_u32(addr)?
            };
            h.cpu.pc = h.cpu.lr & !1;
        } else {
            h.cpu.step(&mut h.bus)?;
        }
        if h.cpu.sp < h.cpu.min_sp {
            h.cpu.min_sp = h.cpu.sp;
        }
        n += 1;
        if n >= budget {
            return Err(EmuError::BudgetExceeded { executed: n });
        }
    }
}

const DATACFLASH_OFFSET: usize = 0x1F000;
const DATACFLASH_SIZE: usize = 2048;
const FIRMWARE_SIZE: usize = 0x1C7EC; // af_190602.dec.bin length

fn make_combined_image(firmware: &[u8], dataflash: &[u8; DATACFLASH_SIZE]) -> Vec<u8> {
    assert!(
        firmware.len() <= FIRMWARE_SIZE,
        "firmware too large for combined image: {} > {}",
        firmware.len(),
        FIRMWARE_SIZE
    );
    let mut img = vec![0xFFu8; COMBINED_IMAGE_SIZE];
    img[..firmware.len()].copy_from_slice(firmware);
    img[DATACFLASH_OFFSET..DATACFLASH_OFFSET + DATACFLASH_SIZE].copy_from_slice(dataflash);
    img
}

/// Build a 2048-byte dataflash with the given 4-byte product ID at offset 316
/// (DF offset 316 = MCU address 0x1F13C) and a sane default for the rest:
/// boot flag 0 (boot into APROM / normal runtime), fw version 110 at offset 256
/// (matching the AF_190602 observation in goals.md: fw_versions=[110]).
/// All other offsets are zero. Offsets are dataflash-local (0..2048); the caller
/// maps them into the combined image at 0x1F000 + offset.
fn make_dataflash(pid: &[u8; 4]) -> [u8; DATACFLASH_SIZE] {
    let mut df = [0u8; DATACFLASH_SIZE];
    // fw version 110 at dataflash offset 256 (LE32)
    df[256..260].copy_from_slice(&110u32.to_le_bytes());
    // boot flag 0
    df[9] = 0;
    // PID at offset 316
    df[316..320].copy_from_slice(pid);
    df
}

#[test]
fn test_combined_image_pins_pid_at_expected_offset() {
    let fw = vec![0u8; FIRMWARE_SIZE];
    let pid = b"M041";
    let df = make_dataflash(pid);
    let img = make_combined_image(&fw, &df);
    assert_eq!(&img[0x1F13C..0x1F140], pid, "PID not at dataflash offset 316 in combined image");
    // dataflash byte 0 lives at img[0x1F000] (df[0] is 0 in this layout —
    // the PID is at offset 316, checked above); verify the mapping with the
    // fw-version word at dataflash offset 256 = img[0x1F100].
    assert_eq!(
        &img[0x1F000 + 256..0x1F000 + 260],
        &110u32.to_le_bytes(),
        "fw version not at dataflash offset 256 in combined image"
    );
    // APROM tail padding is 0xFF
    assert_eq!(
        img[FIRMWARE_SIZE..DATACFLASH_OFFSET],
        vec![0xFFu8; DATACFLASH_OFFSET - FIRMWARE_SIZE],
        "APROM tail / gap not padded with 0xFF"
    );
}

/// LAYER-6 FULL-BOOT GATE (validation §2): boot the *whole* stock image from
/// the reset vector with a synthetic dataflash carrying each product ID, and
/// assert the real boot dispatch reaches the dispatcher/render path without
/// ever hitting the unknown-PID infinite loop at 0x2FF8 — the exact failure
/// mode that hung the Pico Dual, which no render-only gate covers.
///
/// Stop PCs are discovery-driven: 0xD684 is the documented dispatcher, and
/// 0x8CD1 the charge-screen render entry the descriptor names. Whichever the
/// boot path reaches first ends the run; the eprintln hit-report lets the
/// stop set be refined to the exact reachable PC later.
#[test]
#[ignore]
fn test_af_190602_boot_dispatch_by_pid() {
    use std::path::Path;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    // NOTE: the Descriptor schema is the render descriptor's (render_entry /
    // display_buffer / ram_size); the boot json has a different schema and
    // cannot be loaded as one. We take the render descriptor for the harness
    // and hardcode the reset/SP/hang constants from the boot json instead.
    let desc_path = root.join("resources/animations/af_190602.json");
    let img_path = root.join("AF_fw/decrypted/af_190602.dec.bin");
    if !desc_path.exists() || !img_path.exists() {
        eprintln!("descriptor or decrypted image missing; skipping boot gate");
        return;
    }
    let desc_json = std::fs::read_to_string(&desc_path).unwrap();
    let fw = std::fs::read(&img_path).unwrap();

    // Boot constants from resources/boot/af_190602.json (reset vector
    // 0x1659 and initial SP 0x200031C0 are applied by boot_until_settle).
    const HANG_PC: u32 = 0x0000_2FF8; // unknown-PID infinite loop
    const DISPATCHER: u32 = 0x0000_D684;
    const RENDER_ENTRY: u32 = 0x0000_8CD1;
    const BUDGET: u64 = 10_000_000;

    // MMIO the boot path touches, with the values the boot code requires.
    // Discovery (first run): clock_pll_init spins at 0x1890 writing the
    // REGWRPROT unlock bytes (0x59, 0x16, 0x88 — the boot descriptor's
    // unlock_bytes) to 0x40000100 and looping until the register reads back
    // NONZERO (real REGWRPROT bit0 sets when unlocked) — so that stub must
    // return 1, not 0. Reads return the stub value, writes are dropped.
    // Anything else the path turns out to touch surfaces as Unmapped or a
    // budget-exceeded trace — add it here and record it in the handoff doc.
    const BOOT_MMIO_STUBS: &[(u32, u32)] = &[
        (0x4000_0100, 1), // REGWRPROT: unlock sequence leaves it read-1
        (0x4000_0104, 0), // protected reg 1 (boot descriptor)
        (0x5000_0000, 0), // protected reg 2 (boot descriptor)
        (0xE000_ED88, 0), // CPACR (FPU enable)
        (0x4000_0250, 0xFFFF_FFFF), // CLK status: wait_for_clock (0x338, literal
            // 0x40000200 @0x354) polls [base+0x50] until the requested bits read
            // SET (bics.w r2,r0,r2 == 0 → return 1); all-ones = every clock/PLL
            // wait succeeds immediately instead of burning its 0x20F581 countdown
            // (~10M insns per call, clock_pll_init calls it 3×).
    ];

    // Known PIDs → expected to reach the dispatcher. M177 is not asserted at
    // all: it is the STM32 line in a Nuvoton image (per the plan).
    let known_pids: &[&[u8; 4]] = &[
        b"M041", b"M065", b"M077", b"M045", b"M038", b"M037", b"M095", b"M105", b"M064",
    ];
    let unknown_pids: &[&[u8; 4]] = &[b"XXXX"];

    for pid in known_pids.iter().chain(unknown_pids.iter()) {
        let df = make_dataflash(pid);
        let img = make_combined_image(&fw, &df);
        // Fresh descriptor-derived ACL/stub state per harness; the JSON is
        // re-parsed each iteration because Descriptor is not Clone.
        let mut h = Harness::new(&img, load_descriptor(&desc_json).unwrap());
        for &(mmio, val) in BOOT_MMIO_STUBS {
            h.bus.set_stub(mmio, val);
        }
        // boot_until_settle owns the CPU setup (PC/SP/LR/min-sp) from the
        // boot descriptor constants; nothing else to prime here.

        let stop_pcs = [DISPATCHER, RENDER_ENTRY];
        let pid_str = String::from_utf8_lossy(*pid).into_owned();
        match boot_until_settle(&mut h, BUDGET, &stop_pcs, HANG_PC) {
            Ok(true) => {
                assert!(
                    known_pids.contains(&pid),
                    "PID {pid_str} is an unknown-PID negative case but reached the dispatcher \
                     (PC {:#010x}) — it must hit the 0x2FF8 hang",
                    h.cpu.pc
                );
                eprintln!(
                    "PID {pid_str}: settled at {:#010x} ({}), min_sp {:#010x}, \
                     {} acl violations (boot bss/stack writes outside the render ACL), \
                     {} dropped MMIO writes",
                    h.cpu.pc,
                    if h.cpu.pc == DISPATCHER { "dispatcher" } else { "render entry" },
                    h.cpu.min_sp,
                    h.bus.acl_violations.len(),
                    h.bus.dropped_writes.len()
                );
            }
            Ok(false) => {
                assert!(
                    !known_pids.contains(&pid),
                    "PID {pid_str} is a known PID but hit the 0x2FF8 unknown-PID hang\n{}",
                    h.cpu.debug_dump()
                );
                eprintln!("PID {pid_str}: hit the unknown-PID hang at 0x2FF8 as expected");
            }
            Err(EmuError::Unmapped { addr }) => panic!(
                "PID {pid_str}: boot hit unmapped address {addr:#010x} — add it to \
                 BOOT_MMIO_STUBS (or extend the bus) and record it in the handoff doc\n{}",
                h.cpu.debug_dump()
            ),
            Err(e) => panic!("PID {pid_str}: boot failed: {e}\n{}", h.cpu.debug_dump()),
        }
    }
}

// ============================================================================
// CLOCK ANIMATION TEST — DEFERRED (RE-pending)
// ============================================================================
// This test is a placeholder for future clock animation validation.
//
// Status: DEFERRED — requires RE confirmation of a clock-side hook site.
// See:
// - `resources/re/af_190602-dispatcher-analy.md` §7 (clock pathway at 0x9a32)
// - `src-tauri/src/firmware/anim/effects.rs` DEFERRED clock hook comment
// - `resources/animations/af_190602.json` `clock_animations` section
//
// The clock animation (CONFIG_CLOCK_ANIMATION = 6) requires a separate hook
// site from the charge-screen timeout effects. The clock is drawn when the
// timeout bit is CLEAR (beq 0x9a32 in FUN_00009a10), not when it's SET
// (the timeout/dim path at 0x9a20 that the existing effects use).
//
// When RE confirms a clock hook site, this test should:
// 1. Use the confirmed clock hook addresses from the descriptor's
//    clock_animations section
// 2. Exercise the clock drawing path (timeout bit clear)
// 3. Verify the clock animation renders correctly per frame
//
// Placeholder addresses (from RE notes — confirm against af_190602.dec.bin):
// - Clock draw entry: 0x136b4 (big time widget, from §7)
// - Clock branch target: 0x9a32 (beq target when timeout bit clear, from §7.4)
// - Post-render commit: 0x13bf4 (site C, §7.3 — higher regression risk)
//
// Until then, this test is #[ignore]d and will fail if run.
#[test]
#[ignore]
fn test_clock_animation_hook_pending_re_confirmation() {
    // TODO: Implement when RE confirms a clock-side hook site.
    // This test currently documents the intended validation approach.
    //
    // Expected validation sequence (per docs/superpowers/specs/2026-08-19-firmware-animation-pipeline-design.md):
    // 1. Load the patched image with a clock animation config byte (value 6)
    // 2. Boot to the clock display (timeout bit clear)
    // 3. Capture frames over time and assert:
    //    a. Successive frames differ (animation is running)
    //    b. The changing region matches the clock animation's expected pattern
    //    c. After Undo, frames are static and match the stock clock
    //
    // For now, this test is a no-op placeholder that documents the deferred status.
    assert!(false, "Clock animation test is deferred — RE confirmation pending for clock-side hook site");
}

// ============================================================================
// End of animation tests
// ============================================================================

