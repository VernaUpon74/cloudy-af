use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;
use base64::Engine;

use crate::firmware::anim::asm::AnimError;
use crate::firmware::anim::effects::{
    build_center_pulse_patch, build_diagonal_sweep_patch, build_gradient_fade_patch,
    image_supports_animation, load_animation_desc,
};
use crate::firmware::definition::{parse_definition, FirmwareDefinition};
use crate::firmware::encryption::{decrypt, encrypt, EncryptionType};
use crate::firmware::flasher::{flash_firmware_guarded, read_dataflash, read_product_id, restart_device};
use crate::firmware::loader::{load_firmware, FirmwareImage};
use crate::firmware::patch::{apply_patch, parse_patch, rollback_patch, rollback_other_animations, Patch};
use crate::firmware::state::{FirmwareState, OpenFirmware};
use crate::firmware::stock::{line_for_definition, line_for_product, load_library, match_build, MatchKind};

/// Serializes every command that talks to the device directly (flashes AND
/// dataflash reads). Two concurrent flashes interleave 0x35/0xC3 traffic and
/// brick the device — this is what the single-instance plugin cannot prevent
/// (two windows of the SAME instance can start a flash each), and a dataflash
/// read dispatched in the same millisecond a flash starts can still land
/// inside the stream.
static FLASH_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Run a short direct-device operation with the sidecar suspended for the
/// duration. The sidecar's hidraw reader thread consumes every report the
/// device answers with, so a direct read without suspend loses its response
/// to the sidecar and times out. Resume fires before returning in every
/// path; the sidecar reconnects and re-downloads the config afterwards.
async fn with_device<T, F>(sidecar: &crate::SidecarState, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let _ = crate::suspend_sidecar(sidecar).await;
    let _guard = FLASH_MUTEX.lock().await;
    let result = tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?;
    let _ = crate::resume_sidecar(sidecar).await;
    result
}

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

/// Locate bundled animation descriptors (`resources/animations/*.json`),
/// mirroring the candidate-list pattern of `patch_dirs`.
fn animation_descriptor_paths(app: &AppHandle) -> Vec<PathBuf> {
    let resource_dir = app.path().resource_dir().ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));

    let dirs: Vec<PathBuf> = [
        resource_dir.clone().map(|d| d.join("animations")),
        resource_dir.clone().map(|d| d.join("../animations")),
        exe_dir.clone().map(|d| d.join("../lib/cloudy-af/resources/animations")),
        exe_dir.clone().map(|d| d.join("resources/animations")),
        exe_dir.clone().map(|d| d.join("animations")),
        option_env!("CARGO_MANIFEST_DIR").map(|s| {
            PathBuf::from(s)
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("resources/animations")
        }),
    ]
    .into_iter()
    .flatten()
    .collect();

    let mut result = Vec::new();
    for dir in dirs {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.extension().map(|x| x == "json").unwrap_or(false) {
                    result.push(path);
                }
            }
        }
    }
    result
}

/// Animation-effect patches for an opened image: one patch per effect for
/// every bundled descriptor whose build matches the image. The effects are
/// only offered when the image passes `image_supports_animation` (stock
/// hook-site bytes + erased code cave, per the af_190602 RE notes), so they
/// never attach to an incompatible or already-patched image.
fn animation_patches(app: &AppHandle, image: &[u8]) -> Vec<Patch> {
    let mut result = Vec::new();
    let mut seen_descs = std::collections::HashSet::new();
    for path in animation_descriptor_paths(app) {
        // Candidate dirs can overlap (dev resource dir next to the exe plus
        // the CARGO_MANIFEST_DIR fallback); list each descriptor only once.
        if !seen_descs.insert(path.file_name().map(|n| n.to_owned()).unwrap_or_default()) {
            continue;
        }
        let Ok(desc_json) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(anim) = load_animation_desc(&desc_json) else {
            continue;
        };
        if !image_supports_animation(image, &anim) {
            continue;
        }
        for build in [
            build_gradient_fade_patch as fn(&str) -> Result<Patch, AnimError>,
            build_center_pulse_patch,
            build_diagonal_sweep_patch,
        ] {
            if let Ok(patch) = build(&desc_json) {
                result.push(patch);
            }
        }
    }
    result
}

/// Locate the bundled `firmware` resource directory (stock builds +
/// devices.json), mirroring the candidate-list pattern of `load_definitions`.
fn firmware_resource_dir(app: &AppHandle) -> Option<PathBuf> {
    let resource_dir = app.path().resource_dir().ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));

    let candidates: Vec<PathBuf> = [
        resource_dir.clone().map(|d| d.join("firmware")),
        resource_dir.clone().map(|d| d.join("../firmware")),
        exe_dir.clone().map(|d| d.join("../lib/cloudy-af/resources/firmware")),
        exe_dir.clone().map(|d| d.join("resources/firmware")),
        exe_dir.clone().map(|d| d.join("firmware")),
        option_env!("CARGO_MANIFEST_DIR").map(|s| {
            PathBuf::from(s)
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("resources/firmware")
        }),
    ]
    .into_iter()
    .flatten()
    .collect();

    candidates.into_iter().find(|d| d.is_dir())
}

/// Insert an already-loaded image into the firmware state: save a backup of
/// the original bytes, collect the available patches, and return the info
/// JSON (`handle`, `name`, `encryption`). Callers extend the JSON with
/// provenance fields (`build_id`, `match_kind`, ...).
fn insert_opened(
    app: &AppHandle,
    state: &FirmwareState,
    image: FirmwareImage,
) -> Result<Value, String> {
    let handle = Uuid::new_v4().to_string();
    let info = serde_json::json!({
        "handle": handle,
        "name": image.definition.name,
        "encryption": format!("{:?}", image.encryption),
    });

    // Save a backup of the original firmware bytes for the Undo Changes feature.
    let original_path = backup_path(app, &handle);
    if let Some(ref path) = original_path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let original_encrypted = encrypt(&image.bytes, image.encryption).map_err(|e| e.to_string())?;
        let _ = std::fs::write(path, original_encrypted);
    }

    let patches = load_available_patches(app, &image.definition);
    let mut parsed_patches: Vec<_> = patches
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
    parsed_patches.extend(animation_patches(app, &image.bytes));

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

#[tauri::command]
pub async fn open_firmware(
    app: AppHandle,
    state: State<'_, FirmwareState>,
    path: String,
) -> Result<Value, String> {
    let defs = load_definitions(&app);
    let image = load_firmware(Path::new(&path), &defs).map_err(|e| e.to_string())?;

    let mut info = insert_opened(&app, &state, image)?;
    info["build_id"] = Value::Null;
    info["match_kind"] = Value::from("file");
    Ok(info)
}

/// Load the stock library (devices.json) from the firmware resource dir.
fn load_stock_library(app: &AppHandle) -> Result<(crate::firmware::stock::StockLibrary, PathBuf), String> {
    let dir = firmware_resource_dir(app)
        .ok_or_else(|| "firmware resource directory not found".to_string())?;
    let json = std::fs::read_to_string(dir.join("devices.json"))
        .map_err(|e| format!("cannot read devices.json: {e}"))?;
    let lib = load_library(&json).map_err(|e| e.to_string())?;
    Ok((lib, dir))
}

/// Load a stock build file into the firmware state, exactly like
/// `open_firmware` does for user-picked files.
fn open_stock_file(
    app: &AppHandle,
    state: &FirmwareState,
    dir: &Path,
    build: &crate::firmware::stock::StockBuild,
    match_kind: &str,
    extra: &[(&str, Value)],
) -> Result<Value, String> {
    let path = dir.join(&build.file);
    if !path.is_file() {
        return Err(format!("stock build file missing: {}", path.display()));
    }
    let defs = load_definitions(app);
    let image = load_firmware(&path, &defs).map_err(|e| e.to_string())?;

    let mut info = insert_opened(app, state, image)?;
    info["build_id"] = Value::from(build.id.clone());
    info["match_kind"] = Value::from(match_kind);
    for (key, value) in extra {
        info[*key] = value.clone();
    }
    Ok(info)
}

/// Read the connected device's dataflash, pick the best matching bundled
/// stock build, and open it like `open_firmware`. `NoLine` (unknown product
/// id) returns an error listing the known lines; the UI can then fall back
/// to `open_stock_build`.
#[tauri::command]
pub async fn download_stock(
    app: AppHandle,
    sidecar: State<'_, crate::SidecarState>,
    state: State<'_, FirmwareState>,
) -> Result<Value, String> {
    // Suspend the sidecar: its hidraw reader thread would otherwise consume
    // the 0x35 response and this read would time out.
    let df = with_device(&sidecar, || read_dataflash().map_err(|e| e.to_string())).await?;
    if df.len() < 320 {
        return Err("dataflash too short for product id".to_string());
    }
    let product_id = String::from_utf8_lossy(&df[316..320])
        .trim_matches(char::from(0))
        .trim()
        .to_string();
    let fw_version = crate::firmware::flasher::parse_fw_version(&df).map_err(|e| e.to_string())?;

    let (lib, dir) = load_stock_library(&app)?;
    let (kind, build) = match_build(&lib, &product_id, fw_version);
    let (kind, build) = match (kind, build) {
        (MatchKind::NoLine, _) => {
            let lines: Vec<&str> = lib.lines.iter().map(|l| l.name.as_str()).collect();
            return Err(format!(
                "unknown product id {product_id:?}; known lines: {}",
                lines.join(", ")
            ));
        }
        (_, Some(b)) => (kind, b),
        (_, None) => return Err("no stock builds available".to_string()),
    };
    let match_kind = match kind {
        MatchKind::ExactVersion => "exact_version",
        MatchKind::LineOnly => "line_only",
        MatchKind::NoLine => unreachable!(),
    };
    open_stock_file(
        &app,
        &state,
        &dir,
        build,
        match_kind,
        &[
            ("product_id", Value::from(product_id)),
            ("fw_version", Value::from(fw_version)),
        ],
    )
}

/// Explicitly open a bundled stock build by its id (see devices.json).
#[tauri::command]
pub async fn open_stock_build(
    app: AppHandle,
    state: State<'_, FirmwareState>,
    build_id: String,
) -> Result<Value, String> {
    let (lib, dir) = load_stock_library(&app)?;
    let build = lib
        .builds
        .iter()
        .find(|b| b.id == build_id)
        .ok_or_else(|| format!("unknown stock build {build_id:?}"))?;
    // Null-fill the device provenance keys so the response has the same
    // 7-key shape as `download_stock` (UI reads them unconditionally).
    open_stock_file(
        &app,
        &state,
        &dir,
        build,
        "manual",
        &[("product_id", Value::Null), ("fw_version", Value::Null)],
    )
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
                // The animation effects share one hook site and one code
                // cave; the device config byte selects the active effect, so
                // only one may be applied at a time.
                rollback_other_animations(
                    &mut fw.image.bytes,
                    &mut fw.patches,
                    &mut fw.rollback_log,
                    &patch_id,
                );
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
pub async fn read_device_dataflash(
    sidecar: State<'_, crate::SidecarState>,
) -> Result<Vec<u8>, String> {
    with_device(&sidecar, || read_dataflash().map_err(|e| e.to_string())).await
}

#[tauri::command]
pub async fn read_device_product_id(
    sidecar: State<'_, crate::SidecarState>,
) -> Result<String, String> {
    with_device(&sidecar, || read_product_id().map_err(|e| e.to_string())).await
}

/// Flash the opened image, refusing when the connected device's Product ID
/// belongs to a different device line than the firmware's definition (e.g.
/// an STM32-line build onto a Nuvoton device).
fn flash_guarded_for_image(app: &AppHandle, bytes: &[u8], definition: &str) -> Result<(), String> {
    let pid = read_product_id().map_err(|e| e.to_string())?;
    let (lib, _) = load_stock_library(app)?;
    let device_line = line_for_product(&lib, &pid)
        .ok_or_else(|| format!("refusing to flash: unknown device product id {pid:?}"))?;
    let image_line = line_for_definition(definition)
        .ok_or_else(|| format!("refusing to flash: no device line known for definition {definition:?}"))?;
    if device_line.name != image_line {
        return Err(format!(
            "refusing to flash: firmware targets the {image_line} line, but the device ({pid}) is {}",
            device_line.name
        ));
    }
    flash_firmware_guarded(bytes, Some(&pid)).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn flash_firmware_to_device(
    app: AppHandle,
    sidecar: State<'_, crate::SidecarState>,
    state: State<'_, FirmwareState>,
    handle: String,
) -> Result<(), String> {
    let (bytes, enc, definition) = state
        .with(&handle, |fw| {
            (
                fw.image.bytes.clone(),
                fw.image.encryption,
                fw.image.definition.name.clone(),
            )
        })
        .ok_or_else(|| "Firmware handle not found".to_string())?;

    let encrypted = encrypt(&bytes, enc).map_err(|e| e.to_string())?;

    // The HID sidecar polls the device for configuration; suspend it so it
    // cannot interleave commands into the flash stream.
    let _ = crate::suspend_sidecar(&sidecar).await;
    let _flash_guard = FLASH_MUTEX.lock().await;
    let result = tauri::async_runtime::spawn_blocking(move || {
        flash_guarded_for_image(&app, &encrypted, &definition)
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = crate::resume_sidecar(&sidecar).await;
    result
}

#[tauri::command]
pub async fn restart_device_cmd(
    sidecar: State<'_, crate::SidecarState>,
) -> Result<(), String> {
    with_device(&sidecar, || restart_device().map_err(|e| e.to_string())).await
}

/// Read and decode one live monitoring sample (0x66) for the Device Monitor
/// window. Served by the HID sidecar, which already owns the open device:
/// suspending it per sample used to close/reopen the hidraw handle every
/// poll, racing node-hid's read thread (SIGABRT / "free(): invalid pointer")
/// and re-downloading the full config after every sample.
#[tauri::command]
pub async fn read_monitoring_data_cmd(
    sidecar: State<'_, crate::SidecarState>,
) -> Result<crate::firmware::monitoring::MonitoringData, String> {
    let res = crate::sidecar_request(&sidecar, serde_json::json!({ "type": "monitoring" })).await?;
    if let Some(msg) = res.get("message").and_then(|v| v.as_str()) {
        return Err(format!("Sidecar error: {}", msg));
    }
    let b64 = res
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or("Missing monitoring data in sidecar reply")?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| e.to_string())?;
    crate::firmware::monitoring::decode_monitoring_data(&raw).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn undo_firmware_changes(
    sidecar: State<'_, crate::SidecarState>,
    state: State<'_, FirmwareState>,
    handle: String,
) -> Result<(), String> {
    let backup = state
        .with(&handle, |fw| fw.original_backup_path.clone())
        .ok_or_else(|| "Firmware handle not found".to_string())?;

    let backup = backup.ok_or_else(|| "No original firmware backup available".to_string())?;
    let bytes = std::fs::read(&backup).map_err(|e| e.to_string())?;
    let _ = crate::suspend_sidecar(&sidecar).await;
    let _flash_guard = FLASH_MUTEX.lock().await;
    let result = tauri::async_runtime::spawn_blocking(move || {
        flash_firmware_guarded(&bytes, None).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = crate::resume_sidecar(&sidecar).await;
    result
}

#[tauri::command]
pub async fn list_hid_devices() -> Result<Vec<crate::firmware::flasher::DeviceInfo>, String> {
    crate::firmware::flasher::list_devices().map_err(|e| e.to_string())
}

/// Emergency recovery: wait for the device, flash the given image file,
/// restart, and verify. Progress is reported via `recovery-progress` events.
///
/// The recovery LDROM updater expects a plaintext firmware image. If the user
/// selects an encrypted ArcticFox/Joyetech/VandalProof package, decrypt it
/// before streaming it to the device.
#[tauri::command]
pub async fn recovery_flash(
    app: AppHandle,
    sidecar: State<'_, crate::SidecarState>,
    path: String,
    expected_product_id: Option<String>,
) -> Result<(), String> {
    use tauri::Emitter;
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let (plain, _enc) = decrypt(&bytes).map_err(|e| format!("cannot decrypt {path}: {e}"))?;
    let app2 = app.clone();
    let _ = crate::suspend_sidecar(&sidecar).await;
    let _flash_guard = FLASH_MUTEX.lock().await;
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::firmware::flasher::recovery_flash(&plain, expected_product_id.as_deref(), |msg| {
            let _ = app2.emit("recovery-progress", msg);
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = crate::resume_sidecar(&sidecar).await;
    result
}
