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
    // Same definition setup as loader_test.rs (no load_definitions_from
    // helper exists): ArcticFox detected via the AFOX marker at 0x140.
    // STM32-line builds carry no AFOX marker; after the decrypt cascade the
    // only identifying string is "Joyetech APP" (the same marker
    // encryption.rs uses to validate decrypted STM32 images), so a second
    // definition keyed on it detects them.
    let xml = r#"<FirmwareDefinition Name="ArcticFox">
        <Marker Offset="0x140" Bytes="0x41 0x46 0x4F 0x58" />
        <ImageTable1 PtrFrom="0x144" PtrTo="0x148" />
        <ImageTable2 PtrFrom="0x14C" PtrTo="0x150" />
        <StringTable1 PtrFrom="0x154" PtrTo="0x158" TwoBytesPerChar="false" />
    </FirmwareDefinition>
    <FirmwareDefinition Name="ArcticFox STM32">
        <Marker Bytes="0x4A 0x6F 0x79 0x65 0x74 0x65 0x63 0x68 0x20 0x41 0x50 0x50" />
    </FirmwareDefinition>"#;
    parse_definition(xml).unwrap()
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
