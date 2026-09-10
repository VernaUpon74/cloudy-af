use quick_xml::events::{attributes::Attributes, Event};
use quick_xml::Reader;
use std::ops::Range;

use super::{FirmwareError, Result};

#[derive(Debug, Clone)]
pub struct FirmwareDefinition {
    pub id: String,
    pub name: String,
    pub marker: Vec<u8>,
    pub image_table_1: Range<usize>,
    pub image_table_2: Range<usize>,
    pub string_table_1: Range<usize>,
    pub string_table_2: Range<usize>,
    pub char_width: u8,
}

impl Default for FirmwareDefinition {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            marker: Vec::new(),
            image_table_1: Range::default(),
            image_table_2: Range::default(),
            string_table_1: Range::default(),
            string_table_2: Range::default(),
            char_width: 1,
        }
    }
}

fn hex_to_bytes(s: &str) -> Result<Vec<u8>> {
    // Split on whitespace and "0x"/"0X" prefixes so inputs like
    // "0x41 0x46 0x4F 0x58" or "41464F58" both work.
    let s = s.replace("0x", " ").replace("0X", " ");
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if s.len() % 2 != 0 {
        return Err(FirmwareError::Xml("odd hex byte count".into()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| FirmwareError::Xml(format!("bad hex byte at offset {}", i)))
        })
        .collect()
}

fn parse_hex_usize(s: &str) -> Result<usize> {
    let s = s.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        usize::from_str_radix(&s[2..], 16)
            .map_err(|e| FirmwareError::Xml(format!("bad hex address {}: {}", s, e)))
    } else {
        s.parse::<usize>()
            .map_err(|e| FirmwareError::Xml(format!("bad address {}: {}", s, e)))
    }
}

fn parse_range(attrs: Attributes<'_>) -> Result<Range<usize>> {
    let mut start = 0usize;
    let mut end = 0usize;
    for attr in attrs {
        let attr = attr.map_err(|e| FirmwareError::Xml(e.to_string()))?;
        let value = String::from_utf8_lossy(&attr.value);
        match attr.key.as_ref() {
            b"PtrFrom" | b"From" => start = parse_hex_usize(&value)?,
            b"PtrTo" | b"To" => end = parse_hex_usize(&value)?,
            _ => {}
        }
    }
    Ok(start..end)
}

fn parse_bool(s: &str) -> bool {
    let s = s.trim().to_lowercase();
    s == "true" || s == "1" || s == "yes"
}

pub fn parse_definition(xml: &str) -> Result<Vec<FirmwareDefinition>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut defs = Vec::new();
    let mut current: Option<FirmwareDefinition> = None;
    let mut pending_text: Option<String> = None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                match e.name().as_ref() {
                    b"FirmwareDefinition" | b"Definition" => {
                        let mut def = FirmwareDefinition::default();
                        for attr in e.attributes() {
                            let attr = attr.map_err(|e| FirmwareError::Xml(e.to_string()))?;
                            if attr.key.as_ref() == b"Name" {
                                let name = String::from_utf8_lossy(&attr.value).to_string();
                                def.id = name.to_lowercase().replace(' ', "-");
                                def.name = name;
                            }
                        }
                        current = Some(def);
                    }
                    b"Marker" => {
                        if let Some(ref mut def) = current {
                            let mut bytes_text = String::new();
                            let mut offset_text = String::new();
                            for attr in e.attributes() {
                                let attr = attr.map_err(|e| FirmwareError::Xml(e.to_string()))?;
                                let value = String::from_utf8_lossy(&attr.value).to_string();
                                match attr.key.as_ref() {
                                    b"Bytes" => bytes_text = value,
                                    b"Offset" => offset_text = value,
                                    _ => {}
                                }
                            }
                            if !bytes_text.is_empty() {
                                def.marker = hex_to_bytes(&bytes_text)?;
                            }
                            // Offset is currently informational; the marker bytes
                            // themselves are enough for detection.
                            let _ = offset_text;
                        }
                        pending_text = None;
                    }
                    b"ImageTable1" => {
                        if let Some(ref mut def) = current {
                            def.image_table_1 = parse_range(e.attributes())?;
                        }
                    }
                    b"ImageTable2" => {
                        if let Some(ref mut def) = current {
                            def.image_table_2 = parse_range(e.attributes())?;
                        }
                    }
                    b"StringTable1" => {
                        if let Some(ref mut def) = current {
                            def.string_table_1 = parse_range(e.attributes())?;
                            for attr in e.attributes() {
                                let attr = attr.map_err(|e| FirmwareError::Xml(e.to_string()))?;
                                if attr.key.as_ref() == b"TwoBytesPerChar" {
                                    let value = String::from_utf8_lossy(&attr.value);
                                    if parse_bool(&value) {
                                        def.char_width = 2;
                                    }
                                }
                            }
                        }
                    }
                    b"StringTable2" => {
                        if let Some(ref mut def) = current {
                            def.string_table_2 = parse_range(e.attributes())?;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if current.is_some() {
                    pending_text = Some(e.html_content().unwrap_or_default().to_string());
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"FirmwareDefinition" | b"Definition" => {
                    if let Some(def) = current.take() {
                        defs.push(def);
                    }
                }
                _ => {
                    pending_text = None;
                }
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(FirmwareError::Xml(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(defs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_arcticfox_definition() {
        let xml = r#"<FirmwareDefinition Name="ArcticFox">
    <Marker Offset="0x140" Bytes="0x41 0x46 0x4F 0x58" />
    <ImageTable1 PtrFrom="0x144" PtrTo="0x148" />
    <ImageTable2 PtrFrom="0x14C" PtrTo="0x150" />
    <StringTable1 PtrFrom="0x154" PtrTo="0x158" TwoBytesPerChar="false" />
</FirmwareDefinition>"#;
        let defs = parse_definition(xml).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "ArcticFox");
        assert_eq!(defs[0].marker, vec![0x41, 0x46, 0x4F, 0x58]);
        assert_eq!(defs[0].image_table_1, 0x144..0x148);
        assert_eq!(defs[0].string_table_1, 0x154..0x158);
        assert_eq!(defs[0].char_width, 1);
    }
}
