use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::firmware::definition::{parse_definition, FirmwareDefinition};
use crate::firmware::encryption::{encrypt, EncryptionType};
use crate::firmware::flasher::{flash_firmware as flash_to_device, read_dataflash, read_product_id, restart_device};
use crate::firmware::loader::load_firmware;
use crate::firmware::patch::{apply_patch, parse_patch, rollback_patch};
use crate::firmware::state::{FirmwareState, OpenFirmware};

fn backup_path(app: &AppHandle, handle: &str) -> Option<PathBuf> {
    app.path().config_dir().ok().map(|d| d.join(format!("cloudy-af/firmware-backups/{}.bin", handle)))
}

/// Read all `.xml` firmware definitions from the bundled definitions directory.
fn load_definitions(app: &AppHandle) -> Vec<FirmwareDefinition> {
    let resource_dir = app.path().resource_dir().ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));

    let candidates: Vec<PathBuf> = [
        resource_dir.clone().map(|d| d.join("definitions")),
        resource_dir.clone().map(|d| d.join("../definitions")),
        exe_dir.clone().map(|d| d.join("../lib/cloudy-af/resources/definitions")),
        exe_dir.clone().map(|d| d.join("resources/definitions")),
        exe_dir.clone().map(|d| d.join("definitions")),
        option_env!("CARGO_MANIFEST_DIR").map(|s| {
            PathBuf::from(s)
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("resources/definitions")
        }),
    ]
    .into_iter()
    .flatten()
    .collect();

    for dir in candidates {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let defs: Vec<FirmwareDefinition> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map(|x| x == "xml").unwrap_or(false))
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .filter_map(|xml| parse_definition(&xml).ok())
                .flatten()
                .collect();
            if !defs.is_empty() {
                return defs;
            }
        }
    }

    Vec::new()
}

fn patch_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let resource_dir = app.path().resource_dir().ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));

    let mut dirs: Vec<PathBuf> = [
        resource_dir.clone().map(|d| d.join("patches")),
        resource_dir.clone().map(|d| d.join("../patches")),
        exe_dir.clone().map(|d| d.join("../lib/cloudy-af/resources/patches")),
        exe_dir.clone().map(|d| d.join("resources/patches")),
        exe_dir.clone().map(|d| d.join("patches")),
        option_env!("CARGO_MANIFEST_DIR").map(|s| {
            PathBuf::from(s)
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("resources/patches")
        }),
    ]
    .into_iter()
    .flatten()
    .collect();

    if let Ok(config_dir) = app.path().config_dir() {
        dirs.push(config_dir.join("cloudy-af/patches"));
    }

    dirs
}

fn load_available_patches(
    app: &AppHandle,
    _definition: &FirmwareDefinition,
) -> Vec<(String, PathBuf)> {
    let mut result = Vec::new();
    for dir in patch_dirs(app) {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.extension().map(|x| x == "patch").unwrap_or(false) {
                    if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
                        result.push((id.to_string(), path));
                    }
                }
            }
        }
    }
    result
}

#[tauri::command]
pub async fn open_firmware(
    app: AppHandle,
    state: State<'_, FirmwareState>,
    path: String,
) -> Result<Value, String> {
    let defs = load_definitions(&app);
    let image = load_firmware(Path::new(&path), &defs).map_err(|e| e.to_string())?;

    let handle = Uuid::new_v4().to_string();
    let info = serde_json::json!({
        "handle": handle,
        "name": image.definition.name,
        "encryption": format!("{:?}", image.encryption),
    });

    // Save a backup of the original firmware bytes for the Undo Changes feature.
    let original_path = backup_path(&app, &handle);
    if let Some(ref path) = original_path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let original_encrypted = encrypt(&image.bytes, image.encryption).map_err(|e| e.to_string())?;
        let _ = std::fs::write(path, original_encrypted);
    }

    let patches = load_available_patches(&app, &image.definition);
    let parsed_patches: Vec<_> = patches
        .into_iter()
        .filter_map(|(id, path)| {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|xml| parse_patch(&xml, &id).ok())
                .map(|mut p| {
                    p.id = id;
                    p
                })
        })
        .collect();

    state.insert(
        handle.clone(),
        OpenFirmware {
            image,
            rollback_log: HashMap::new(),
            patches: parsed_patches,
            original_backup_path: original_path.map(|p| p.to_string_lossy().to_string()),
        },
    );

    Ok(info)
}

#[derive(serde::Serialize)]
pub struct PatchInfo {
    id: String,
    name: String,
    version: String,
    author: String,
    description: String,
    applied: bool,
}

#[tauri::command]
pub async fn list_patches(
    state: State<'_, FirmwareState>,
    handle: String,
) -> Result<Vec<PatchInfo>, String> {
    state
        .with(&handle, |fw| {
            fw.patches
                .iter()
                .map(|p| PatchInfo {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    version: p.version.clone(),
                    author: p.author.clone(),
                    description: p.description.clone(),
                    applied: p.applied,
                })
                .collect()
        })
        .ok_or_else(|| "Firmware handle not found".to_string())
}

#[tauri::command]
pub async fn apply_patch_cmd(
    state: State<'_, FirmwareState>,
    handle: String,
    patch_id: String,
) -> Result<Value, String> {
    state
        .with(&handle, |fw| {
            let patch_index = fw.patches.iter().position(|p| p.id == patch_id);
            if let Some(index) = patch_index {
                let patch = &mut fw.patches[index];
                match apply_patch(&mut fw.image.bytes, patch, &mut fw.rollback_log) {
                    Ok(()) => serde_json::json!({ "ok": true }),
                    Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                }
            } else {
                serde_json::json!({ "ok": false, "error": "Patch not found" })
            }
        })
        .ok_or_else(|| "Firmware handle not found".to_string())
}

#[tauri::command]
pub async fn rollback_patch_cmd(
    state: State<'_, FirmwareState>,
    handle: String,
    patch_id: String,
) -> Result<Value, String> {
    state
        .with(&handle, |fw| {
            let patch_index = fw.patches.iter().position(|p| p.id == patch_id);
            if let Some(index) = patch_index {
                let patch = &mut fw.patches[index];
                match rollback_patch(&mut fw.image.bytes, patch, &mut fw.rollback_log) {
                    Ok(()) => serde_json::json!({ "ok": true }),
                    Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                }
            } else {
                serde_json::json!({ "ok": false, "error": "Patch not found" })
            }
        })
        .ok_or_else(|| "Firmware handle not found".to_string())
}

#[tauri::command]
pub async fn save_firmware(
    state: State<'_, FirmwareState>,
    handle: String,
    path: String,
    encryption: Option<String>,
) -> Result<(), String> {
    let (bytes, enc) = state
        .with(&handle, |fw| {
            let enc = match encryption.as_deref() {
                Some("None") => EncryptionType::None,
                Some("Joyetech") => EncryptionType::Joyetech,
                Some("ArcticFox") => EncryptionType::ArcticFox,
                Some("ArcticFox2") => EncryptionType::ArcticFox2,
                _ => fw.image.encryption,
            };
            (fw.image.bytes.clone(), enc)
        })
        .ok_or_else(|| "Firmware handle not found".to_string())?;

    let encrypted = encrypt(&bytes, enc).map_err(|e| e.to_string())?;
    std::fs::write(&path, encrypted).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn close_firmware(
    state: State<'_, FirmwareState>,
    handle: String,
) -> Result<(), String> {
    state.remove(&handle);
    Ok(())
}

#[tauri::command]
pub async fn read_device_dataflash() -> Result<Vec<u8>, String> {
    read_dataflash().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn read_device_product_id() -> Result<String, String> {
    read_product_id().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn flash_firmware_to_device(
    state: State<'_, FirmwareState>,
    handle: String,
) -> Result<(), String> {
    let (bytes, enc) = state
        .with(&handle, |fw| {
            (fw.image.bytes.clone(), fw.image.encryption)
        })
        .ok_or_else(|| "Firmware handle not found".to_string())?;

    let encrypted = encrypt(&bytes, enc).map_err(|e| e.to_string())?;
    flash_to_device(&encrypted).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn restart_device_cmd() -> Result<(), String> {
    restart_device().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn undo_firmware_changes(
    state: State<'_, FirmwareState>,
    handle: String,
) -> Result<(), String> {
    let backup = state
        .with(&handle, |fw| fw.original_backup_path.clone())
        .ok_or_else(|| "Firmware handle not found".to_string())?;

    let backup = backup.ok_or_else(|| "No original firmware backup available".to_string())?;
    let bytes = std::fs::read(&backup).map_err(|e| e.to_string())?;
    flash_to_device(&bytes).map_err(|e| e.to_string())
}
