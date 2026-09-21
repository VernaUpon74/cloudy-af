//! Static-audit fixture dumper (validation route §4): writes the stock
//! decrypted af_190602 image plus one fully-patched image per animation
//! effect, and the per-effect modification metadata
//! (`/tmp/audit/{stock,gradient,center,diagonal}.bin` + `*.mods.json`) for
//! `scripts/audit-cave.py`. Also applies then rolls back each patch on a
//! scratch copy and verifies byte-exact restoration.
//!
//! Skips cleanly (exit 0) when the gitignored RE artifact is absent, like
//! the layer-5 emulator gate. Run from src-tauri/:
//!   cargo run --offline --release --bin anim_patch_dump

use std::collections::HashMap;
use std::path::Path;

use cloudy_af_lib::firmware::anim::effects::{
    build_center_pulse_patch, build_diagonal_sweep_patch, build_gradient_fade_patch,
    load_animation_desc,
};
use cloudy_af_lib::firmware::patch::{apply_patch, rollback_patch, Patch};

fn main() {
    let desc_path = Path::new("../resources/animations/af_190602.json");
    let img_path = Path::new("../AF_fw/decrypted/af_190602.dec.bin");
    if !desc_path.exists() || !img_path.exists() {
        eprintln!("descriptor or decrypted image missing; skipping audit dump");
        return;
    }
    let desc_json = std::fs::read_to_string(desc_path).expect("read descriptor");
    let stock = std::fs::read(img_path).expect("read decrypted image");
    let anim = load_animation_desc(&desc_json).expect("parse descriptor");

    let out = Path::new("/tmp/audit");
    std::fs::create_dir_all(out).expect("mkdir /tmp/audit");
    std::fs::write(out.join("stock.bin"), &stock).expect("write stock.bin");

    let table: [(&str, fn(&str) -> Result<Patch, _>); 3] = [
        ("gradient", build_gradient_fade_patch),
        ("center", build_center_pulse_patch),
        ("diagonal", build_diagonal_sweep_patch),
    ];

    for (name, build) in table {
        let mut patch = build(&desc_json).expect("build patch");
        let mut img = stock.clone();
        let mut log = HashMap::new();
        apply_patch(&mut img, &mut patch, &mut log).expect("apply patch");
        std::fs::write(out.join(format!("{name}.bin")), &img).expect("write patched image");

        // Rollback on a second scratch copy must restore the stock bytes.
        let mut scratch = stock.clone();
        let mut patch2 = build(&desc_json).expect("build patch");
        let mut log2 = HashMap::new();
        apply_patch(&mut scratch, &mut patch2, &mut log2).expect("apply scratch");
        rollback_patch(&mut scratch, &mut patch2, &mut log2).expect("rollback");
        let restored = &scratch[..stock.len()];
        assert_eq!(restored, &stock[..], "{name}: rollback must be byte-exact");
        eprintln!("{name}: rollback byte-exact OK");

        let offsets: Vec<usize> = patch.modifications.iter().map(|m| m.offset).collect();
        let cave_body_len = offsets
            .iter()
            .filter(|&&o| o >= anim.code_cave.start as usize
                && o < anim.code_cave.start as usize + anim.code_cave.size)
            .max()
            .map(|m| m + 1 - anim.code_cave.start as usize)
            .unwrap_or(0);
        let json = format!(
            "{{\"hook_site\": {}, \"hook_resume\": {}, \"cave_start\": {}, \"cave_size\": {}, \"cave_body_len\": {}, \"config_offset\": {}, \"offsets\": {:?}}}",
            anim.hook_site,
            anim.hook_resume,
            anim.code_cave.start,
            anim.code_cave.size,
            cave_body_len,
            anim.config_byte_addr,
            offsets,
        );
        std::fs::write(out.join(format!("{name}.mods.json")), json).expect("write mods json");
        eprintln!("{name}: dumped ({} mods, cave body {} bytes)", offsets.len(), cave_body_len);
    }
}
