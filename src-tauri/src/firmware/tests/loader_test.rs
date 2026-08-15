use crate::firmware::definition::FirmwareDefinition;
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
