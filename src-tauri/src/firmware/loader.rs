use std::path::Path;

use super::definition::FirmwareDefinition;
use super::encryption::{decrypt, EncryptionType};
use super::{FirmwareError, Result};

/// A loaded firmware image with its matched definition and detected encryption.
#[derive(Debug, Clone)]
pub struct FirmwareImage {
    pub definition: FirmwareDefinition,
    pub encryption: EncryptionType,
    pub bytes: Vec<u8>,
}

/// Load a firmware file from disk, decrypt it, and detect its definition by
/// scanning for a known marker.
pub fn load_firmware(path: &Path, definitions: &[FirmwareDefinition]) -> Result<FirmwareImage> {
    let data = std::fs::read(path)?;
    load_firmware_from_bytes(&data, definitions)
}

/// Testable helper that detects a definition from already-loaded bytes.
///
/// Plaintext firmware is checked first so that already-decrypted dumps or
/// unencrypted test payloads are not mangled by the decryption heuristics.
pub fn load_firmware_from_bytes(
    bytes: &[u8],
    definitions: &[FirmwareDefinition],
) -> Result<FirmwareImage> {
    if let Ok(image) = detect_definition(bytes, EncryptionType::None, definitions) {
        return Ok(image);
    }
    let (decrypted, encryption) = decrypt(bytes)?;
    detect_definition(&decrypted, encryption, definitions)
}

fn detect_definition(
    bytes: &[u8],
    encryption: EncryptionType,
    definitions: &[FirmwareDefinition],
) -> Result<FirmwareImage> {
    for def in definitions {
        if bytes.windows(def.marker.len()).any(|w| w == def.marker) {
            return Ok(FirmwareImage {
                definition: def.clone(),
                encryption,
                bytes: bytes.to_vec(),
            });
        }
    }
    Err(FirmwareError::DefinitionNotFound)
}
