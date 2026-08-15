use thiserror::Error;

pub mod definition;
pub mod encryption;
pub mod loader;
pub mod patch;
pub mod state;

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
    #[error("Definition not found")]
    DefinitionNotFound,
    #[error("Incompatible patch at offset {offset}: expected {expected:?}, found {found:?}")]
    IncompatiblePatch { offset: usize, expected: u8, found: u8 },
    #[error("Conflict with patch {other} at offset {offset}")]
    Conflict { offset: usize, other: String },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, FirmwareError>;
