use crate::firmware::definition::parse_definition;

#[test]
fn test_parse_arcticfox_definition() {
    let xml = r#"<?xml version="1.0"?>
<Definitions>
  <Definition Name="ArcticFox">
    <Marker>41 72 63 74 69 63 46 6F 78</Marker>
    <ImageTable1 PtrFrom="0x144" PtrTo="0x148" />
    <ImageTable2 PtrFrom="0x14C" PtrTo="0x150" />
    <StringTable1 From="0x10000" To="0x11000" />
    <StringTable2 From="0x12000" To="0x13000" />
  </Definition>
</Definitions>"#;
    let defs = parse_definition(xml).unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "ArcticFox");
    assert_eq!(
        defs[0].marker,
        vec![0x41, 0x72, 0x63, 0x74, 0x69, 0x63, 0x46, 0x6F, 0x78]
    );
    assert_eq!(defs[0].id, "arcticfox");
    assert_eq!(defs[0].image_table_1, 0x144..0x148);
    assert_eq!(defs[0].image_table_2, 0x14C..0x150);
    assert_eq!(defs[0].string_table_1, 0x10000..0x11000);
    assert_eq!(defs[0].string_table_2, 0x12000..0x13000);
}
