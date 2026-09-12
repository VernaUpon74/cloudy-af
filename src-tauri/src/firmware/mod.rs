use thiserror::Error;

pub mod definition;
pub mod encryption;
pub mod flasher;
pub mod loader;
pub mod monitoring;
pub mod patch;
pub mod resources;
pub mod state;
pub mod stock;
pub mod emu;
pub mod anim;

#[cfg(test)]
mod tests;

#[derive(Debug, Error)]
pub enum FirmwareError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("XML parse error: {0}")]
    Xml(String),
    #[error("Unknown encryption")]
    UnknownEncryption,
    #[error("Unsupported encryption: {0}")]
    UnsupportedEncryption(String),
    #[error("Definition not found")]
    DefinitionNotFound,
    #[error("Incompatible patch at offset {offset}: expected {expected:?}, found {found:?}")]
    IncompatiblePatch { offset: usize, expected: u8, found: u8 },
    #[error("Conflict with patch {other} at offset {offset}")]
    Conflict { offset: usize, other: String },
    /// The device accepted the boot-flag write and restart but never came
    /// back in LDROM updater mode. Distinct from transient IO errors so the
    /// flasher can fall back to waiting for a manual replug.
    #[error("device did not re-enumerate in bootloader mode")]
    DidNotReenumerate,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, FirmwareError>;
