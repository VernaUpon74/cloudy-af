use crate::firmware::definition::parse_definition;

#[test]
fn test_parse_arcticfox_definition() {
    let xml = r#"<?xml version="1.0"?>
<FirmwareDefinition Name="ArcticFox">
  <Marker Offset="0x140" Bytes="0x41 0x46 0x4F 0x58" />
  <ImageTable1 PtrFrom="0x144" PtrTo="0x148" />
  <ImageTable2 PtrFrom="0x14C" PtrTo="0x150" />
  <StringTable1 PtrFrom="0x154" PtrTo="0x158" TwoBytesPerChar="false" />
</FirmwareDefinition>"#;
    let defs = parse_definition(xml).unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "ArcticFox");
    assert_eq!(defs[0].marker, vec![0x41, 0x46, 0x4F, 0x58]);
    assert_eq!(defs[0].id, "arcticfox");
    assert_eq!(defs[0].image_table_1, 0x144..0x148);
    assert_eq!(defs[0].image_table_2, 0x14C..0x150);
    assert_eq!(defs[0].string_table_1, 0x154..0x158);
    assert_eq!(defs[0].char_width, 1);
}
