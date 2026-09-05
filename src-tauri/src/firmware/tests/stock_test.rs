//! Bundled stock library integrity: the four builds ship in
//! resources/firmware and pass the decrypt cascade.
use crate::firmware::definition::{parse_definition, FirmwareDefinition};
use crate::firmware::loader::load_firmware;
use std::path::Path;

fn resources() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("resources")
}

fn defs() -> Vec<FirmwareDefinition> {
    // Parse the SHIPPED definitions file, not an inline copy: an inline copy
    // once masked the fact that ArcticFox.xml lacked the STM32-line
    // ("Joyetech APP" marker) definition, making af_211009.bin unloadable in
    // the app while this test passed.
    let xml = std::fs::read_to_string(resources().join("definitions/ArcticFox.xml")).unwrap();
    parse_definition(&xml).unwrap()
}

#[test]
fn test_bundled_builds_present_and_decryptable() {
    let dir = resources().join("firmware");
    let defs = defs();
    for build in ["af_170222.bin", "af_180913.bin", "af_190602.bin", "af_211009.bin"] {
        let path = dir.join(build);
        let img = load_firmware(&path, &defs).unwrap_or_else(|e| panic!("{build}: {e}"));
        assert!(img.bytes.len() > 30_000, "{build} too small after decrypt");
    }
}

#[test]
fn test_bundled_decrypted_builds_present_and_loadable() {
    let dir = resources().join("firmware/decrypted");
    let defs = defs();
    for build in ["af_170222.bin", "af_180913.bin", "af_190602.bin", "af_211009.bin"] {
        let path = dir.join(build);
        let img = load_firmware(&path, &defs).unwrap_or_else(|e| panic!("{build}: {e}"));
        assert!(img.bytes.len() > 30_000, "{build} too small");
        assert_eq!(img.encryption, crate::firmware::encryption::EncryptionType::None,
            "{build} should be plaintext");
    }
}

#[test]
fn test_devices_json_parses() {
    let raw = std::fs::read_to_string(resources().join("firmware/devices.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["builds"].as_array().unwrap().len(), 4);
    assert!(v["lines"]["nuvoton"]["product_ids"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "M041"));
}

use crate::firmware::stock::{line_for_product, load_library, match_build, MatchKind};

fn lib() -> crate::firmware::stock::StockLibrary {
    let raw = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
            .join("resources/firmware/devices.json")).unwrap();
    load_library(&raw).unwrap()
}

#[test]
fn test_line_lookup() {
    let lib = lib();
    assert_eq!(line_for_product(&lib, "M041").unwrap().name, "nuvoton");
    assert!(line_for_product(&lib, "X999").is_none());
}

#[test]
fn test_match_build_rules() {
    let lib = lib();
    // No build records fw_version 999 -> LineOnly, newest nuvoton build.
    let (kind, build) = match_build(&lib, "M041", 999);
    assert!(matches!(kind, MatchKind::LineOnly));
    assert_eq!(build.unwrap().id, "af_190602");
    // Unknown product -> NoLine.
    let (kind, build) = match_build(&lib, "X999", 110);
    assert!(matches!(kind, MatchKind::NoLine));
    assert!(build.is_none());
}

#[test]
fn test_exact_version_match() {
    let lib = lib();
    let (kind, build) = match_build(&lib, "M041", 110);
    assert!(matches!(kind, MatchKind::ExactVersion));
    assert_eq!(build.unwrap().id, "af_190602");
}
