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

/// LAYER-4 PRE-FLASH GATE. Requires:
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
        prev = Some(frame);
    }
}
