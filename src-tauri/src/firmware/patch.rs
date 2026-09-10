use std::collections::HashMap;

use super::{FirmwareError, Result};

/// A single byte modification inside a patch.
#[derive(Debug, Clone)]
pub struct PatchModification {
    pub offset: usize,
    pub original: Option<u8>,
    pub patched: u8,
}

/// A firmware patch with metadata and a list of byte modifications.
#[derive(Debug, Clone, Default)]
pub struct Patch {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub modifications: Vec<PatchModification>,
    pub applied: bool,
}

/// Parse a `.patch` XML document.
///
/// The expected format is:
///
/// ```xml
/// <Patch Name="..." Version="..." Author="...">
///   <Description>...</Description>
///   <Data>
///     0x10: 0x00 - 0xFF
///     0x11: * - 0xAA
///     # comment
///   </Data>
/// </Patch>
/// ```
pub fn parse_patch(xml: &str, id: &str) -> Result<Patch> {
    let mut patch = Patch {
        id: id.to_string(),
        ..Default::default()
    };

    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut pending_text = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) => {
                if e.name().as_ref() == b"Patch" {
                    for attr in e.attributes() {
                        let attr = attr.map_err(|e| FirmwareError::Xml(e.to_string()))?;
                        let value = String::from_utf8_lossy(&attr.value).to_string();
                        match attr.key.as_ref() {
                            b"Name" => patch.name = value,
                            b"Version" => patch.version = value,
                            b"Author" => patch.author = value,
                            _ => {}
                        }
                    }
                }
                pending_text.clear();
            }
            Ok(quick_xml::events::Event::Text(e)) => {
                pending_text.push_str(&e.html_content().unwrap_or_default());
            }
            Ok(quick_xml::events::Event::End(e)) => match e.name().as_ref() {
                b"Description" => {
                    patch.description = pending_text.trim().to_string();
                    pending_text.clear();
                }
                b"Data" => {
                    patch.modifications = parse_data(&pending_text)?;
                    pending_text.clear();
                }
                _ => {
                    pending_text.clear();
                }
            },
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => return Err(FirmwareError::Xml(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(patch)
}

fn parse_data(text: &str) -> Result<Vec<PatchModification>> {
    let mut mods = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("");
        let line = line.split(';').next().unwrap_or("");
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Expected format: "0xOFFSET: 0xOLD - 0xNEW" or "0xOFFSET: * - 0xNEW"
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() != 2 {
            continue;
        }
        let offset = parse_hex(parts[0].trim())?;

        let rhs: Vec<&str> = parts[1].split('-').collect();
        if rhs.len() != 2 {
            return Err(FirmwareError::Xml(format!("bad patch line: {line}")));
        }

        let original_str = rhs[0].trim();
        let original = if original_str == "*" {
            None
        } else {
            Some(parse_hex(original_str)? as u8)
        };

        let patched = parse_hex(rhs[1].trim())? as u8;

        mods.push(PatchModification {
            offset,
            original,
            patched,
        });
    }
    Ok(mods)
}

fn parse_hex(s: &str) -> Result<usize> {
    let s = s.trim();
    let s = s.strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    usize::from_str_radix(s, 16).map_err(|_| FirmwareError::Xml(format!("bad hex: {s}")))
}

/// Apply a patch to a firmware image.
///
/// `rollback_log` records the original byte at each modified offset so the
/// patch can be rolled back later. If another patch has already modified an
/// offset, the existing rollback entry is preserved.
pub fn apply_patch(
    firmware: &mut Vec<u8>,
    patch: &mut Patch,
    rollback_log: &mut HashMap<usize, u8>,
) -> Result<()> {
    for m in &patch.modifications {
        // Modifications past the stock image end land in an erased-APROM code
        // cave: grow the image (0xFF = erased flash) to cover them.
        if m.offset >= firmware.len() {
            firmware.resize(m.offset + 1, 0xFF);
        }
        let current = firmware[m.offset];
        if let Some(expected) = m.original {
            if current != expected {
                return Err(FirmwareError::IncompatiblePatch {
                    offset: m.offset,
                    expected,
                    found: current,
                });
            }
        }
        rollback_log.entry(m.offset).or_insert(current);
        firmware[m.offset] = m.patched;
    }
    patch.applied = true;
    Ok(())
}

/// Roll back a previously applied patch.
pub fn rollback_patch(
    firmware: &mut [u8],
    patch: &mut Patch,
    rollback_log: &mut HashMap<usize, u8>,
) -> Result<()> {
    for m in &patch.modifications {
        if let Some(original) = rollback_log.get(&m.offset) {
            firmware[m.offset] = *original;
        }
    }
    // Remove rollback entries that are no longer needed by any applied patch.
    for m in &patch.modifications {
        rollback_log.remove(&m.offset);
    }
    patch.applied = false;
    Ok(())
}

/// Find conflicts between patches that want to write different bytes to the
/// same offset.
pub fn find_conflicts(patches: &[Patch]) -> Vec<(String, String, usize)> {
    let mut claims: HashMap<usize, (u8, String)> = HashMap::new();
    let mut conflicts = Vec::new();
    for patch in patches {
        for m in &patch.modifications {
            if let Some((value, other)) = claims.get(&m.offset) {
                if *value != m.patched {
                    conflicts.push((other.clone(), patch.id.clone(), m.offset));
                }
            } else {
                claims.insert(m.offset, (m.patched, patch.id.clone()));
            }
        }
    }
    conflicts
}

/// Animation effect patches all detour the SAME hook site into the SAME code
/// cave; the device config byte selects which effect runs, so at most one may
/// be applied at a time. Roll back every other applied animation patch (id
/// prefix `anim-`) so applying one never layers two effects' cave bodies.
pub fn rollback_other_animations(
    firmware: &mut [u8],
    patches: &mut [Patch],
    rollback_log: &mut HashMap<usize, u8>,
    except_id: &str,
) {
    for patch in patches.iter_mut() {
        if patch.id != except_id && patch.applied && patch.id.starts_with("anim-") {
            let _ = rollback_patch(firmware, patch, rollback_log);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH_XML: &str = r#"<?xml version="1.0"?>
<Patch Name="Test Patch" Version="1.0" Author="Dev">
  <Description>Test</Description>
  <Data>
0x10: 0x00 - 0xFF
0x11: 0x01 - 0xAA
# comment
0x12: * - 0xBB
  </Data>
</Patch>"#;

    #[test]
    fn test_parse_and_apply_patch() {
        let mut patch = parse_patch(PATCH_XML, "test").unwrap();
        assert_eq!(patch.name, "Test Patch");
        assert_eq!(patch.version, "1.0");
        assert_eq!(patch.author, "Dev");
        assert_eq!(patch.modifications.len(), 3);

        let mut firmware = vec![0u8; 32];
        firmware[0x10] = 0x00;
        firmware[0x11] = 0x01;
        firmware[0x12] = 0x55;

        let mut log = HashMap::new();
        apply_patch(&mut firmware, &mut patch, &mut log).unwrap();

        assert_eq!(firmware[0x10], 0xFF);
        assert_eq!(firmware[0x11], 0xAA);
        assert_eq!(firmware[0x12], 0xBB);

        rollback_patch(&mut firmware, &mut patch, &mut log).unwrap();

        assert_eq!(firmware[0x10], 0x00);
        assert_eq!(firmware[0x11], 0x01);
        assert_eq!(firmware[0x12], 0x55);
        assert!(log.is_empty());
    }

    #[test]
    fn test_incompatible_patch() {
        let mut patch = parse_patch(PATCH_XML, "test").unwrap();
        let mut firmware = vec![0u8; 32];
        firmware[0x10] = 0x42; // does not match expected 0x00
        let mut log = HashMap::new();
        assert!(apply_patch(&mut firmware, &mut patch, &mut log).is_err());
    }

    #[test]
    fn test_find_conflicts() {
        let xml_a = r#"<Patch Name="A"><Data>0x10: 0x00 - 0xFF</Data></Patch>"#;
        let xml_b = r#"<Patch Name="B"><Data>0x10: 0x00 - 0xAA</Data></Patch>"#;
        let patch_a = parse_patch(xml_a, "a").unwrap();
        let patch_b = parse_patch(xml_b, "b").unwrap();
        let conflicts = find_conflicts(&[patch_a, patch_b]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].2, 0x10);
    }

    #[test]
    fn test_rollback_other_animations() {
        // Two animations claiming the same hook site; applying the second
        // must roll back the first so only one cave body is live.
        let xml_a = r#"<Patch Name="A"><Data>0x10: * - 0xAA</Data></Patch>"#;
        let xml_b = r#"<Patch Name="B"><Data>0x10: * - 0xBB</Data></Patch>"#;
        let xml_c = r#"<Patch Name="C"><Data>0x20: * - 0xCC</Data></Patch>"#;
        let mut patches = vec![
            parse_patch(xml_a, "anim-a").unwrap(),
            parse_patch(xml_b, "anim-b").unwrap(),
            parse_patch(xml_c, "plain-c").unwrap(),
        ];
        let mut firmware = vec![0u8; 32];
        let mut log = HashMap::new();
        apply_patch(&mut firmware, &mut patches[0], &mut log).unwrap();
        apply_patch(&mut firmware, &mut patches[2], &mut log).unwrap();

        rollback_other_animations(&mut firmware, &mut patches, &mut log, "anim-b");
        assert!(!patches[0].applied, "other animation rolled back");
        assert_eq!(firmware[0x10], 0x00, "hook site restored");
        assert!(patches[2].applied, "non-animation patch untouched");
        assert_eq!(firmware[0x20], 0xCC);

        apply_patch(&mut firmware, &mut patches[1], &mut log).unwrap();
        assert_eq!(firmware[0x10], 0xBB);
        rollback_other_animations(&mut firmware, &mut patches, &mut log, "anim-b");
        assert!(patches[1].applied, "excepted patch survives");
        assert_eq!(firmware[0x10], 0xBB);
    }

    #[test]
    fn test_apply_patch_grows_image_into_cave() {
        // A modification past the stock image end (erased-APROM code cave)
        // grows the image with 0xFF padding instead of panicking.
        let xml = r#"<Patch Name="C"><Data>0x40: * - 0xAB</Data></Patch>"#;
        let mut patch = parse_patch(xml, "c").unwrap();
        let mut firmware = vec![0u8; 0x20];
        let mut log = HashMap::new();
        apply_patch(&mut firmware, &mut patch, &mut log).unwrap();
        assert_eq!(firmware.len(), 0x41);
        assert_eq!(firmware[0x40], 0xAB);
        assert!(firmware[0x20..0x40].iter().all(|b| *b == 0xFF));
        assert_eq!(log[&0x40], 0xFF);
    }
}
