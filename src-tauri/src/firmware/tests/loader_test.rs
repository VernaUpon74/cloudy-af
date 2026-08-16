use crate::firmware::definition::{parse_definition, FirmwareDefinition};
use crate::firmware::loader::load_firmware_from_bytes;
use std::ops::Range;

#[test]
fn test_detect_definition_by_marker() {
    let mut bytes = vec![0u8; 1024];
    let marker = b"ArcticFox";
    bytes[100..100 + marker.len()].copy_from_slice(marker);
    let defs = vec![FirmwareDefinition {
        id: "arcticfox".into(),
        name: "ArcticFox".into(),
        marker: marker.to_vec(),
        image_table_1: Range::default(),
        image_table_2: Range::default(),
        string_table_1: Range::default(),
        string_table_2: Range::default(),
        char_width: 1,
    }];
    let fw = load_firmware_from_bytes(&bytes, &defs).unwrap();
    assert_eq!(fw.definition.name, "ArcticFox");
}

#[test]
fn test_load_af_190624_bin() {
    let xml = r#"<FirmwareDefinition Name="ArcticFox">
        <Marker Offset="0x140" Bytes="0x41 0x46 0x4F 0x58" />
        <ImageTable1 PtrFrom="0x144" PtrTo="0x148" />
        <ImageTable2 PtrFrom="0x14C" PtrTo="0x150" />
        <StringTable1 PtrFrom="0x154" PtrTo="0x158" TwoBytesPerChar="false" />
    </FirmwareDefinition>"#;
    let defs = parse_definition(xml).unwrap();
    let path = "/var/home/j/Downloads/DH-Discord-BKUP/ArcticFox/af_190624.bin";
    let bytes = std::fs::read(path).expect("read firmware file");
    let err = load_firmware_from_bytes(&bytes, &defs).expect_err("should fail for VandalProof");
    assert!(
        err.to_string().contains("Unsupported encryption"),
        "expected unsupported encryption error, got: {err}"
    );
}
