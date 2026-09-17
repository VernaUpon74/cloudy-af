//! Animation-patch screenshot dump (validation §3 visual check): applies each
//! effect patch to the decrypted af_190602 image, boots the emulator, primes
//! the stock charge screen, then drives the hook host (FUN_00009a10) once per
//! phase — exactly like `test_af_190602_animation_frames` — and writes every
//! framebuffer as a PGM into an output folder. Post-process with
//! scripts/anim_shots_to_gif.py (PNG + animated GIF, needs PIL).

use cloudy_af_lib::firmware::anim::asm::AnimError;
use cloudy_af_lib::firmware::anim::effects::{
    build_center_pulse_patch, build_diagonal_sweep_patch, build_gradient_fade_patch,
    CONFIG_CENTER_PULSE, CONFIG_DIAGONAL_SWEEP, CONFIG_GRADIENT_FADE,
};
use cloudy_af_lib::firmware::emu::harness::{
    dump_pgm, load_descriptor, unpack_block1, Harness, RETURN_SENTINEL,
};
use cloudy_af_lib::firmware::patch::apply_patch;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const CLOCK_RENDERER: u32 = 0x9a10; // FUN_00009a10, host of the hook
const PHASE_GLOBAL: u32 = 0x2000_2cf8; // descriptor animation.phase_global
const STATUS_WORD: u32 = 0x2000_2c34; // bit 0x20000 = timed out
/// Phases per effect period: the sine LUT is 16 entries; Gradient Fade moves
/// bands every 4 phases, so 16 covers a full cycle of every effect.
const PHASES: u32 = 16;

fn run_at(h: &mut Harness, entry: u32, budget: u64) {
    h.cpu.pc = entry & !1;
    h.cpu.lr = RETURN_SENTINEL | 1;
    h.cpu
        .run_until(&mut h.bus, RETURN_SENTINEL, budget)
        .unwrap_or_else(|e| panic!("run_at {entry:#x}: {e}\n{}", h.cpu.debug_dump()));
    assert!(
        h.bus.acl_violations.is_empty(),
        "cave must stay inside the framebuffer ACL"
    );
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let out_root = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| root.join("tmp/anim-shots").to_string_lossy().into_owned()),
    );
    let desc_path = root.join("resources/animations/af_190602.json");
    let img_path = root.join("AF_fw/decrypted/af_190602.dec.bin");
    let desc_json = std::fs::read_to_string(&desc_path).unwrap_or_else(|e| {
        eprintln!("descriptor {} missing ({e})", desc_path.display());
        std::process::exit(1)
    });
    let stock_img = std::fs::read(&img_path).unwrap_or_else(|e| {
        eprintln!("decrypted image {} missing ({e})", img_path.display());
        std::process::exit(1)
    });

    let table: [( &str, fn(&str) -> Result<cloudy_af_lib::firmware::patch::Patch, AnimError>, u8); 3] = [
        ("gradient", build_gradient_fade_patch, CONFIG_GRADIENT_FADE),
        ("center", build_center_pulse_patch, CONFIG_CENTER_PULSE),
        ("diagonal", build_diagonal_sweep_patch, CONFIG_DIAGONAL_SWEEP),
    ];

    for (name, build, config) in table {
        let mut img = stock_img.clone();
        let mut patch = build(&desc_json).unwrap();
        let mut log = HashMap::new();
        apply_patch(&mut img, &mut patch, &mut log).unwrap();

        let mut h = Harness::new(&img, load_descriptor(&desc_json).unwrap());
        let buf_start = h.desc.display_buffer.range.start;
        let buf_len = (h.desc.display_buffer.range.end - h.desc.display_buffer.range.start) as usize;
        let read_buf = |h: &Harness| -> Vec<u8> {
            (0..buf_len as u32)
                .map(|i| h.bus.read_u8(buf_start + i).unwrap())
                .collect()
        };
        let (w, hh) = (h.desc.display_buffer.width, h.desc.display_buffer.height);

        // Prime: stock charge screen with the timeout bit clear.
        let prime = h.run_frame(5_000_000).unwrap_or_else(|e| {
            panic!("{name} prime: {e}\n{}", h.cpu.debug_dump())
        });

        // Timeout bits + effect selection, then drive the hook host once per
        // phase — each run bumps the phase global and fades the buffer.
        let status = h.bus.read_u32(STATUS_WORD).unwrap_or(0);
        h.bus.write_u32(STATUS_WORD, status | 0x20000 | 0x80000).unwrap();
        let cfg_addr = h.desc.stubs[0].addr;
        h.bus.set_stub(cfg_addr, config as u32);

        let dir = out_root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dump_pgm(&prime, &dir.join("phase_00.pgm")).unwrap();
        for k in 1..=PHASES {
            run_at(&mut h, CLOCK_RENDERER, 5_000_000);
            let phase = h.bus.read_u32(PHASE_GLOBAL).unwrap();
            assert_eq!(phase, k, "{name}: phase global must count cave runs");
            let raw = read_buf(&h);
            let frame = unpack_block1(&raw, w, hh);
            dump_pgm(&frame, &dir.join(&format!("phase_{k:02}.pgm"))).unwrap();
            let on = frame.pixels.iter().filter(|&&px| px != 0).count();
            eprintln!("{name}: phase {k:2} ok, on-pixels = {on}");
        }
        eprintln!("{name}: {PHASES} frames -> {}", dir.display());
    }
    eprintln!("all effects done -> {}", out_root.display());
}

