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

    // Cave stored 0xFF into display_buffer byte 15 = column x=15, y-group 0;
    // bits 0..7 -> pixels (15, 0..7). Frame.pixels is row-major:
    // pixels[y * width + x], width 64.
    for y in 0..8 {
        assert_eq!(frame.pixels[y * 64 + 15], 1, "pixel (15, {y}) should be on");
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
    // Frame 1: phase 0 -> 1, pixel = 1 -> display_buffer[0] bit0 set -> (0,0) on.
    let f1 = h.run_frame(100_000).expect("frame 1 must not fault");
    // Frame 2: phase 1 -> 2, pixel = 0 -> (0,0) off again.
    let f2 = h.run_frame(100_000).expect("frame 2 must not fault");
    assert_eq!(f1.pixels[0], 1);
    assert_eq!(f2.pixels[0], 0);
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
    gate_render("af_190602", 409);
}

#[test]
#[ignore]
fn test_af_190624_render_gate() {
    // Same gate against the newer Nuvoton-line build (19.06.24). The charge
    // screen should render the same 409 on-pixels; a drift here is a build
    // difference to record, not an emulator fault.
    gate_render("af_190624", 409);
}

/// Layer-4 render gate, parametric over the build: boots the stock image at
/// the descriptor's render entry and asserts a clean 4-frame run plus the
/// golden on-pixel count. Skipped (not failed) when the gitignored decrypted
/// image is absent.
fn gate_render(build: &str, expect_on: usize) {
    use std::path::Path;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let desc_path = root.join(format!("resources/animations/{build}.json"));
    let img_path = root.join(format!("AF_fw/decrypted/{build}.dec.bin"));
    if !desc_path.exists() || !img_path.exists() {
        eprintln!("{build}: descriptor or decrypted image missing; skipping gate");
        return;
    }
    let desc = load_descriptor(&std::fs::read_to_string(&desc_path).unwrap()).unwrap();
    let img = std::fs::read(&img_path).unwrap();
    let mut h = Harness::new(&img, desc);
    let mut prev = None;
    for frame_no in 0..4 {
        let frame = h.run_frame(5_000_000)
            .unwrap_or_else(|e| panic!("{build} frame {frame_no}: {e}\n{}", h.cpu.debug_dump()));
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
        assert_eq!(on, expect_on, "{build} frame {frame_no}: on-pixel count drifted");
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
fn apply_wave(buf: &mut [u8], phase: u32) {
    crate::firmware::anim::effects::wave_apply(buf, phase);
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
#[test]
#[ignore]
fn test_af_190602_animation_frames() {
    gate_animation_frames("af_190602");
}

#[test]
#[ignore]
fn test_af_190624_animation_frames() {
    // The animation gate against the newer Nuvoton-line build (19.06.24) —
    // descriptor resources/animations/af_190624.json was derived from the
    // af_190602 RE by pattern-anchored porting (code +0xBC early, RAM
    // literals -0x18; validation-handoff Task 8). This gate is the
    // empirical proof of the port.
    gate_animation_frames("af_190624");
}

/// Layer-5 animation gate, parametric over the build. All addresses come
/// from the descriptor: the hook host is the even address at hook_site-6
/// (the prologue ahead of the stock `tst.w r2,#0x20000` at hook_site holds
/// in both 19.06.x builds).
fn gate_animation_frames(build: &str) {
    use std::path::Path;

    use crate::firmware::anim::asm::AnimError;
    use crate::firmware::anim::effects::{
        build_center_pulse_patch, build_diagonal_sweep_patch, build_gradient_fade_patch,
        build_wave_patch, CONFIG_CENTER_PULSE, CONFIG_DIAGONAL_SWEEP, CONFIG_GRADIENT_FADE,
        CONFIG_WAVE,
    };
    use crate::firmware::anim::effects::load_animation_desc;
    use crate::firmware::emu::harness::dump_pgm;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let desc_path = root.join(format!("resources/animations/{build}.json"));
    let img_path = root.join(format!("AF_fw/decrypted/{build}.dec.bin"));
    if !desc_path.exists() || !img_path.exists() {
        eprintln!("{build}: descriptor or decrypted image missing; skipping gate");
        return;
    }
    let desc_json = std::fs::read_to_string(&desc_path).unwrap();
    let stock_img = std::fs::read(&img_path).unwrap();
    let anim = load_animation_desc(&desc_json).unwrap();
    let clock_renderer = (anim.hook_site - 6) & !1; // host of the hook
    let phase_global = anim.phase_global;
    let status_word = anim.status_word_addr;

    let table: [(
        &str,
        fn(&str) -> Result<Patch, AnimError>,
        u8,
        fn(&mut [u8], u32),
    ); 4] = [
        ("gradient", build_gradient_fade_patch, CONFIG_GRADIENT_FADE, apply_gradient),
        ("center", build_center_pulse_patch, CONFIG_CENTER_PULSE, apply_center),
        ("diagonal", build_diagonal_sweep_patch, CONFIG_DIAGONAL_SWEEP, apply_diagonal),
        ("wave", build_wave_patch, CONFIG_WAVE, apply_wave),
    ];

    for (name, build_patch, config, apply) in table {
        let mut img = stock_img.clone();
        let mut patch = build_patch(&desc_json).unwrap();
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
            panic!("{build} {name} prime: {e}\n{}", h.cpu.debug_dump())
        });
        let primed = read_buf(&h);

        // Raise the timeout bits the stock dim/idle path needs: 0x20000 makes
        // the hook's `beq` fall through at hook_site+4; 0x80000 (status bit
        // 19) makes the `bmi.w` take the pop-only screen-off branch —
        // without it the code continues to the other idle screen (RE doc
        // §7.1). Then select this effect (the descriptor stub defaults to
        // Gradient Fade) and drive the hook host directly.
        let status = h.bus.read_u32(status_word).unwrap_or(0);
        h.bus.write_u32(status_word, status | 0x20000 | 0x80000).unwrap();
        let cfg_addr = h.desc.stubs[0].addr;
        h.bus.set_stub(cfg_addr, config as u32);

        let (w, hh) = (h.desc.display_buffer.width, h.desc.display_buffer.height);
        let unpack = |raw: &[u8]| crate::firmware::emu::harness::unpack_block1(raw, w, hh);
        let mut prev = prime.clone();
        // Cover complete cycles, including the center/diagonal clipping edges.
        for k in 1..=64u32 {
            run_at(&mut h, clock_renderer, 5_000_000);
            let phase = h.bus.read_u32(phase_global).unwrap();
            assert_eq!(phase, k, "{build} {name}: phase global must count cave runs");
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
                "{build} {name}: frame {k} must equal the reference fade of the primed buffer (diffs: {})",
                first_diffs.join(", ")
            );
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
                    "{build} {name}: animation must change frames (phase {phase})"
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
        dump_pgm(&prime, Path::new(&format!("/tmp/af_anim_{build}_{name}_prime.pgm"))).unwrap();
        dump_pgm(&prev, Path::new(&format!("/tmp/af_anim_{build}_{name}_f4.pgm"))).unwrap();
        eprintln!("{build} {name}: ok, final on-pixels = {final_on}");
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
    // TEMP probe: one-shot log of the transition into non-image space, with
    // the preceding PCs — the boot runaway diagnosis (Task 7).
    let mut stray_logged = false;
    loop {
        if stop_pcs.contains(&h.cpu.pc) {
            return Ok(true);
        }
        if h.cpu.pc == hang_pc {
            return Ok(false);
        }
        let stray = h.cpu.pc >= 0x1F800 && h.cpu.pc < 0x2000_0000
            || (0x4000_0000..0xE000_0000).contains(&h.cpu.pc);
        if stray && !stray_logged {
            stray_logged = true;
            if std::env::var("CLOUDY_STRAY_TRACE").is_ok() {
                let sp = h.cpu.sp;
                let stack = (-4..6i32)
                    .map(|i| h.bus.read_u32((sp as i64 + i as i64 * 4) as u32).unwrap_or(0xFFFF_FFFF))
                    .collect::<Vec<_>>();
                eprintln!(
                    "[stray-pc] pc={:#010x} sp={:#010x} lr={:#010x} after {} insns; stack: {:x?}; recent: {:?}",
                    h.cpu.pc, sp, h.cpu.lr, n, stack,
                    h.cpu.trace.iter().rev().take(24).collect::<Vec<_>>()
                );
            }
        }
        h.cpu.step(&mut h.bus)?;
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
    // BUDGET raised 10M→200M (2026-09-16): the 0x4000_C000 block-copy engine
    // handshake costs ~13 emulated insns per copied word (program [0xC]=0,
    // [0x10]=1, poll [0x10]==0, copy [8], loop at 0x5EC) where the real
    // engine does it in a few cycles — boot copies enough words that 10M
    // ran out mid-copy (discovery run: ~30M insn/s, 200M ≈ 7 s wall clock).
    const BUDGET: u64 = 200_000_000;

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
        // 0x4000C000 is the Nuvoton FMC (ISPCON/ISPADR/ISPDAT/ISPCMD/ISPTRG),
        // modeled in bus.rs (Fmc) — no stubs: ISPDAT reads serve the last ISP
        // command's result, ISPTRG reads 0 (not busy). The boot's early copy
        // loop (0x5EC), CID/DID/UID reads (0x38xx) and the 4 KiB LDROM SKU
        // scan (0x302C: JWEI-gated, FMC READ 0x00100000..0x00100FFF, ~60
        // 4-char ID literals incl. M041 at 0x3324) all go through it.
        (0x4000_0250, 0xFFFF_FFFF), // CLK status: wait_for_clock (0x338, literal
            // 0x40000200 @0x354) polls [base+0x50] until the requested bits read
            // SET (bics.w r2,r0,r2 == 0 → return 1); all-ones = every clock/PLL
            // wait succeeds immediately instead of burning its 0x20F581 countdown
            // (~10M insns per call, clock_pll_init calls it 3×).
    ];

    // Real dataflash ground truth: the LDROM boot path SKU-scans the whole
    // 2 KiB dataflash image (0x30AE loop: r1 walks 4 KiB, r5 counts to
    // 0x1000, Eleaf-SKU ASCII prefix literals in the pool at 0x34B8) and
    // hangs on `bl 0x2FF8` when nothing matches — a sparse synthetic
    // image (PID+version+bootflag only) therefore exhausts the scan even
    // with a valid PID (Task 6 discovery: real dumps carry e.g. "U25TE" at
    // df[0x124] plus a full config block). Boot the REAL rescue dump
    // (PID-patched to the matrix value) when present; fall back to the
    // synthetic image, which then honestly fails the scan for ALL pids —
    // acceptable only for the negative cases, where the hang IS the
    // expectation.
    let real_df_path = root.join("test-fixtures/rescue/dataflash_stock_v1.00.bin");
    let real_df: Option<Vec<u8>> = std::fs::read(&real_df_path)
        .ok()
        .filter(|d| d.len() >= DATACFLASH_SIZE && &d[316..320] == b"M041");
    if real_df.is_none() {
        eprintln!(
            "no real M041 dataflash dump at {} — known-PID boot matrix will \
             exhaust the LDROM SKU scan on the synthetic image (scan-exhaust \
             hang is a known limitation, see handoff Task 6)",
            real_df_path.display()
        );
    }

    // LDROM ground truth: the boot SKU-scan (0x302C) reads the first 4 KiB of
    // the LDROM at 0x00100000 via FMC ISP READ and compares every word
    // against ~60 4-char device-ID literals (E052/E115/E043 in registers +
    // pools at 0x32F4-0x3388 and 0x34B8-0x3528; M041 at 0x3324). The real
    // LDROM carries "M041" at offset 0x878 (HIDC at 0x5C7); without it the
    // scan reads zeros and exhausts → `bl 0x2FF8` hang even with a valid
    // dataflash PID. Artifact-gated like the dataflash dump.
    let ldrom_path = root.join("test-fixtures/ldrom/ldrom_m041_16k.bin");
    let ldrom: Option<Vec<u8>> = std::fs::read(&ldrom_path).ok().filter(|d| d.len() >= 0x1000);
    if ldrom.is_none() {
        eprintln!(
            "no LDROM dump at {} — the SKU scan reads zeros and hangs at 0x2FF8 \
             even for known PIDs (the scan source is the LDROM via FMC, not the \
             dataflash — Task 7 discovery)",
            ldrom_path.display()
        );
    }

    // Known PIDs → expected to reach the dispatcher. M177 is not asserted at
    // all: it is the STM32 line in a Nuvoton image (per the plan).
    let known_pids: &[&[u8; 4]] = &[
        b"M041", b"M065", b"M077", b"M045", b"M038", b"M037", b"M095", b"M105", b"M064",
    ];
    let unknown_pids: &[&[u8; 4]] = &[b"XXXX"];

    for pid in known_pids.iter().chain(unknown_pids.iter()) {
        // Known PIDs boot on the real dump (PID-patched); negatives use the
        // sparse synthetic image, where the SKU scan exhausts → 0x2FF8 hang.
        let mut df: [u8; DATACFLASH_SIZE] = match &real_df {
            Some(d) => {
                let mut a = [0u8; DATACFLASH_SIZE];
                a.copy_from_slice(&d[..DATACFLASH_SIZE]);
                a
            }
            None => make_dataflash(pid),
        };
        df[316..320].copy_from_slice(*pid);
        let img = make_combined_image(&fw, &df);
        // Fresh descriptor-derived ACL/stub state per harness; the JSON is
        // re-parsed each iteration because Descriptor is not Clone.
        let mut h = Harness::new(&img, load_descriptor(&desc_json).unwrap());
        for &(mmio, val) in BOOT_MMIO_STUBS {
            h.bus.set_stub(mmio, val);
        }
        // Attach the real LDROM dump for the FMC SKU scan (present on every
        // physical device, so negatives get it too).
        if let Some(ld) = &ldrom {
            h.bus.fmc.ldrom = ld.clone();
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

/// Execute the af_190602 IRQ 42 handler in isolation. Binary evidence:
/// vector 0xE8 points to 0x108B5; the handler writes the completion flag
/// polled at 0x10B14 and acknowledges the peripheral at 0x400430F8.
/// This does not test interrupt delivery, exception entry/return, or timing.
#[test]
#[ignore] // requires AF_fw/decrypted/af_190602.dec.bin
fn test_af_190602_irq42_handler_sets_completion_flag() {
    use crate::firmware::emu::bus::Bus;
    use crate::firmware::emu::cpu::Cpu;
    use crate::firmware::emu::harness::RETURN_SENTINEL;
    use std::path::Path;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let fw = std::fs::read(root.join("AF_fw/decrypted/af_190602.dec.bin"))
        .expect("explicit artifact gate requires the decrypted af_190602 image");
    const FLAG: u32 = 0x2000_0E6C;
    const ACK: u32 = 0x4004_30F8;
    for initial_flag in [0u32, 1] {
        let mut bus = Bus::new(fw.clone(), 0x8000);
        bus.allow_region(FLAG..FLAG + 4);
        bus.write_u32(FLAG, initial_flag).unwrap();
        bus.write_log.clear();
        let vector = bus.read_u32(0xE8).unwrap();
        assert_eq!(vector, 0x0001_08B5, "unexpected IRQ42 vector");
        let mut cpu = Cpu::new();
        cpu.pc = vector & !1;
        cpu.sp = 0x2000_3100;
        cpu.lr = RETURN_SENTINEL | 1;
        cpu.run_until(&mut bus, RETURN_SENTINEL, 32)
            .expect("IRQ42 handler must return within 32 instructions");

        assert_eq!(bus.read_u32(FLAG).unwrap(), 1);
        assert_eq!(cpu.sp, 0x2000_3100, "handler must not alter SP");
        assert!(bus.acl_violations.is_empty(), "{:?}", bus.acl_violations);
        assert_eq!(bus.write_log.len(), 1, "only the completion flag is written");
        let flag_write = &bus.write_log[0];
        assert_eq!((flag_write.addr, flag_write.value, flag_write.size), (FLAG, 1, 4));
        // MMIO writes are recorded, not latched; assert the write, not readback.
        assert_eq!(bus.dropped_writes.len(), 1);
        let ack = &bus.dropped_writes[0];
        assert_eq!((ack.addr, ack.value, ack.size), (ACK, 1, 4));
    }
}
