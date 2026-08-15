use quick_xml::events::{attributes::Attributes, Event};
use quick_xml::Reader;
use std::ops::Range;

use super::{FirmwareError, Result};

#[derive(Debug, Clone, Default)]
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

fn hex_to_bytes(s: &str) -> Result<Vec<u8>> {
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
                    b"Definition" => {
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
                    b"Marker" => pending_text = None,
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
                    pending_text = Some(e.unescape().unwrap_or_default().to_string());
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"Definition" => {
                    if let Some(def) = current.take() {
                        defs.push(def);
                    }
                }
                b"Marker" => {
                    if let Some(ref mut def) = current {
                        if let Some(text) = pending_text.take() {
                            def.marker = hex_to_bytes(&text)?;
                        }
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
