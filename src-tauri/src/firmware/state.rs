use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::loader::FirmwareImage;
use super::patch::Patch;

/// Mutable state for one open firmware image.
#[derive(Debug)]
pub struct OpenFirmware {
    pub image: FirmwareImage,
    pub rollback_log: HashMap<usize, u8>,
    pub patches: Vec<Patch>,
}

/// Shared, thread-safe map of open firmware images keyed by handle.
#[derive(Debug, Clone, Default)]
pub struct FirmwareState(Arc<Mutex<HashMap<String, Mutex<OpenFirmware>>>>);

impl FirmwareState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, handle: String, fw: OpenFirmware) {
        self.0.lock().unwrap().insert(handle, Mutex::new(fw));
    }

    pub fn with<F, R>(&self, handle: &str, f: F) -> Option<R>
    where
        F: FnOnce(&mut OpenFirmware) -> R,
    {
        self.0
            .lock()
            .unwrap()
            .get(handle)
            .map(|m| f(&mut m.lock().unwrap()))
    }

    pub fn remove(&self, handle: &str) -> Option<OpenFirmware> {
        self.0.lock().unwrap().remove(handle).map(|m| m.into_inner().unwrap())
    }
}
