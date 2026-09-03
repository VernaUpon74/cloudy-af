// DEVIATION: This Rust/Tauri backend replaces the Electron main process from the
// original hobbyquaker/arcticfox-config fork. It spawns the Node.js HID sidecar,
// bridges its stdout events to the webview via Tauri events, and exposes file
// system and dialog APIs that the renderer previously accessed through Node/Electron.
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tauri::{Emitter, Manager, State, WindowEvent, WebviewUrl, WebviewWindowBuilder};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

mod commands;
mod firmware;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct IpcEvent {
    channel: String,
    data: serde_json::Value,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SidecarEvent {
    event: String,
    payload: serde_json::Value,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SidecarErrorEvent {
    message: String,
    detail: Option<String>,
}

pub(crate) struct SidecarState {
    child: Arc<Mutex<Option<Child>>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>>,
}

impl SidecarState {
    fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            stdin: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct IpcSendRequest {
    channel: String,
    #[serde(default)]
    data: serde_json::Value,
}

fn find_sidecar_script(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let candidates = vec![
        app.path().resource_dir().map(|p| p.join("sidecar/hid-bridge.js")).ok(),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .map(|p| p.join("../lib/cloudy-af/resources/sidecar/hid-bridge.js")),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .map(|p| p.join("resources/sidecar/hid-bridge.js")),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .map(|p| p.join("sidecar/hid-bridge.js")),
        option_env!("CARGO_MANIFEST_DIR")
            .map(|s| PathBuf::from(s))
            .map(|p| p.join("../sidecar/hid-bridge.js")),
    ];

    for candidate in candidates.into_iter().flatten() {
        let canonical = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if canonical.exists() {
            return Ok(canonical);
        }
    }

    Err("Could not find sidecar/hid-bridge.js".to_string())
}

async fn sidecar_send(state: &SidecarState, mut cmd: serde_json::Value, request_id: Option<String>) -> Result<(), String> {
    if let Some(id) = request_id {
        if let serde_json::Value::Object(ref mut map) = cmd {
            map.insert("request_id".to_string(), id.into());
        }
    }
    let line = serde_json::to_string(&cmd).map_err(|e| e.to_string())?;
    let mut stdin_guard = state.stdin.lock().await;
    if let Some(stdin) = stdin_guard.as_mut() {
        stdin
            .write_all(format!("{}\n", line).as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err("Sidecar not running".to_string())
    }
}

async fn sidecar_request(state: &SidecarState, cmd: serde_json::Value) -> Result<serde_json::Value, String> {
    let id = Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    {
        let mut pending = state.pending.lock().await;
        pending.insert(id.clone(), tx);
    }
    sidecar_send(state, cmd, Some(id)).await?;
    rx.await.map_err(|_| "Sidecar response cancelled".to_string())
}

/// Tell the HID sidecar to close the device and stop polling/reconnecting.
/// Must be called before the firmware flasher talks to the device directly,
/// so config reads cannot interleave into the flash stream (brick risk).
/// Best-effort: returns Err when no sidecar is running, callers ignore it.
pub(crate) async fn suspend_sidecar(state: &SidecarState) -> Result<(), String> {
    sidecar_request(state, serde_json::json!({ "type": "suspend" })).await?;
    Ok(())
}

/// Undo `suspend_sidecar`: the sidecar reconnects and resumes polling.
pub(crate) async fn resume_sidecar(state: &SidecarState) -> Result<(), String> {
    sidecar_request(state, serde_json::json!({ "type": "resume" })).await?;
    Ok(())
}

#[tauri::command]
async fn ipc_send(
    app: tauri::AppHandle,
    state: State<'_, SidecarState>,
    request: IpcSendRequest,
) -> Result<(), String> {
    match request.channel.as_str() {
        "bat" | "tfr" | "pc" | "pireg" | "firmware" => {
            open_sub_window(&app, &request.channel, request.data).await
        }
        "piregchange" | "batchange" | "tfrchange" | "pcchange" => {
            app.emit_to(
                "main",
                "ipc-event",
                IpcEvent {
                    channel: request.channel,
                    data: request.data,
                },
            )
            .map_err(|e| e.to_string())
        }
        _ => {
            let mut cmd = serde_json::Map::new();
            cmd.insert("type".to_string(), request.channel.into());
            if !request.data.is_null() {
                cmd.insert("data".to_string(), request.data);
            }
            sidecar_send(&state, serde_json::Value::Object(cmd), None).await
        }
    }
}

async fn open_sub_window(
    app: &tauri::AppHandle,
    channel: &str,
    data: serde_json::Value,
) -> Result<(), String> {
    let (label, title, width, height, html) = match channel {
        "bat" => ("bat", "Battery Profile", 545, 520, "bat.html"),
        "tfr" => ("tfr", "TFR Profile", 545, 380, "tfr.html"),
        "pc" => ("pc", "Power Curve", 545, 520, "power.html"),
        "pireg" => ("pireg", "PI Regulator", 400, 275, "pireg.html"),
        "firmware" => ("firmware", "Firmware Editor", 900, 600, "firmware.html"),
        _ => return Err("Unknown sub-window".to_string()),
    };

    let window = if let Some(w) = app.get_webview_window(label) {
        w
    } else {
        WebviewWindowBuilder::new(app, label, WebviewUrl::App(html.into()))
            .title(title)
            .inner_size(width as f64, height as f64)
            .resizable(true)
            .build()
            .map_err(|e| e.to_string())?
    };

    window.emit(
        "ipc-event",
        IpcEvent {
            channel: "data".to_string(),
            data,
        },
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_locale() -> Result<String, String> {
    Ok(tauri_plugin_os::locale().unwrap_or_else(|| "en".to_string()))
}

#[tauri::command]
async fn close_window(window: tauri::Window) -> Result<(), String> {
    window.close().map_err(|e| e.to_string())
}

#[tauri::command]
async fn resolve_resource_path(
    app: tauri::AppHandle,
    relative_path: String,
) -> Result<String, String> {
    let resource_dir = app.path().resource_dir().map_err(|e| e.to_string())?;
    let candidates = vec![
        resource_dir.join(&relative_path),
        PathBuf::from("/app/lib/cloudy-af/resources").join(&relative_path),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .map(|p| p.join("../lib/cloudy-af/resources").join(&relative_path))
            .unwrap_or_default(),
    ];
    if let Some(manifest) = option_env!("CARGO_MANIFEST_DIR") {
        let dev_path = PathBuf::from(manifest).join("../").join(&relative_path);
        let public_path = PathBuf::from(manifest).join("../public/").join(&relative_path);
        for candidate in [dev_path, public_path] {
            if candidate.exists() {
                return Ok(candidate.to_string_lossy().to_string());
            }
        }
    }
    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.to_string_lossy().to_string());
        }
    }
    Err(format!("Resource not found: {}", relative_path))
}

#[tauri::command]
async fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

#[tauri::command]
async fn write_text_file(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| e.to_string())
}

#[tauri::command]
async fn read_binary_file(path: String) -> Result<Vec<u8>, String> {
    std::fs::read(&path).map_err(|e| e.to_string())
}

#[tauri::command]
async fn write_binary_file(path: String, bytes: Vec<u8>) -> Result<(), String> {
    std::fs::write(&path, bytes).map_err(|e| e.to_string())
}

#[derive(Serialize, Deserialize)]
struct FileFilter {
    name: String,
    extensions: Vec<String>,
}

#[tauri::command]
async fn open_file_dialog(
    dialog: tauri::State<'_, tauri_plugin_dialog::Dialog<tauri::Wry>>,
    filters: Option<Vec<FileFilter>>,
) -> Result<Option<String>, String> {
    let mut builder = dialog.file();
    if let Some(filters) = filters {
        for f in filters {
            let exts: Vec<&str> = f.extensions.iter().map(|s| s.as_str()).collect();
            builder = builder.add_filter(f.name, &exts);
        }
    }
    let path = builder.blocking_pick_file();
    Ok(path.map(|p| p.to_string()))
}

#[tauri::command]
async fn save_file_dialog(
    dialog: tauri::State<'_, tauri_plugin_dialog::Dialog<tauri::Wry>>,
    default_name: Option<String>,
    filters: Option<Vec<FileFilter>>,
) -> Result<Option<String>, String> {
    let mut builder = dialog.file();
    if let Some(name) = default_name {
        builder = builder.set_file_name(&name);
    }
    if let Some(filters) = filters {
        for f in filters {
            let exts: Vec<&str> = f.extensions.iter().map(|s| s.as_str()).collect();
            builder = builder.add_filter(f.name, &exts);
        }
    }
    let path = builder.blocking_save_file();
    Ok(path.map(|p| p.to_string()))
}

#[tauri::command]
async fn show_error(
    dialog: tauri::State<'_, tauri_plugin_dialog::Dialog<tauri::Wry>>,
    title: String,
    message: String,
) -> Result<(), String> {
    dialog.message(message).title(title).show(|_| {});
    Ok(())
}

#[tauri::command]
async fn open_config(
    state: State<'_, SidecarState>,
    dialog: tauri::State<'_, tauri_plugin_dialog::Dialog<tauri::Wry>>,
) -> Result<Option<serde_json::Value>, String> {
    let builder = dialog.file().add_filter("AFC Configuration", &["afc"]);
    let path = builder.blocking_pick_file();
    let path = match path {
        Some(p) => p,
        None => return Ok(None),
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let cmd = serde_json::json!({
        "type": "decode_afc",
        "data": base64::engine::general_purpose::STANDARD.encode(&bytes)
    });
    let res = sidecar_request(&state, cmd).await?;
    Ok(res.get("config").cloned())
}

#[tauri::command]
async fn save_config(
    state: State<'_, SidecarState>,
    dialog: tauri::State<'_, tauri_plugin_dialog::Dialog<tauri::Wry>>,
    config: serde_json::Value,
) -> Result<(), String> {
    let cmd = serde_json::json!({
        "type": "encode_afc",
        "config": config
    });
    let res = sidecar_request(&state, cmd).await?;
    if let Some(msg) = res.get("message").and_then(|v| v.as_str()) {
        return Err(format!("Sidecar error: {}", msg));
    }
    let data = res
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or("Missing encoded data")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| e.to_string())?;

    let builder = dialog
        .file()
        .add_filter("AFC Configuration", &["afc"])
        .set_file_name("config.afc");
    let path = builder.blocking_save_file();
    let path = match path {
        Some(p) => p,
        None => return Ok(()),
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())
}

#[tauri::command]
async fn import_tfr(
    state: State<'_, SidecarState>,
    dialog: tauri::State<'_, tauri_plugin_dialog::Dialog<tauri::Wry>>,
) -> Result<Option<serde_json::Value>, String> {
    let builder = dialog.file().add_filter("CSV Table", &["csv"]);
    let path = builder.blocking_pick_file();
    let path = match path {
        Some(p) => p,
        None => return Ok(None),
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    let csv = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let cmd = serde_json::json!({
        "type": "tfr_import_csv",
        "csv": csv
    });
    let res = sidecar_request(&state, cmd).await?;
    Ok(res.get("table").cloned())
}

#[tauri::command]
async fn export_tfr(
    state: State<'_, SidecarState>,
    dialog: tauri::State<'_, tauri_plugin_dialog::Dialog<tauri::Wry>>,
    table: serde_json::Value,
) -> Result<(), String> {
    let cmd = serde_json::json!({
        "type": "tfr_export_csv",
        "table": table
    });
    let res = sidecar_request(&state, cmd).await?;
    let csv = res
        .get("csv")
        .and_then(|v| v.as_str())
        .ok_or("Missing CSV output")?;
    let name = res
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("TFR");
    let builder = dialog
        .file()
        .add_filter("CSV Table", &["csv"])
        .set_file_name(&format!("{}.csv", name));
    let path = builder.blocking_save_file();
    let path = match path {
        Some(p) => p,
        None => return Ok(()),
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, csv).map_err(|e| e.to_string())
}

#[tauri::command]
async fn import_bat(
    state: State<'_, SidecarState>,
    dialog: tauri::State<'_, tauri_plugin_dialog::Dialog<tauri::Wry>>,
) -> Result<Option<serde_json::Value>, String> {
    let builder = dialog.file().add_filter("Battery Profile XML", &["xml"]);
    let path = builder.blocking_pick_file();
    let path = match path {
        Some(p) => p,
        None => return Ok(None),
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    let xml = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let cmd = serde_json::json!({
        "type": "bat_import_xml",
        "xml": xml
    });
    let res = sidecar_request(&state, cmd).await?;
    Ok(Some(res))
}

#[tauri::command]
async fn export_bat(
    state: State<'_, SidecarState>,
    dialog: tauri::State<'_, tauri_plugin_dialog::Dialog<tauri::Wry>>,
    table: serde_json::Value,
) -> Result<(), String> {
    let cmd = serde_json::json!({
        "type": "bat_export_xml",
        "table": table
    });
    let res = sidecar_request(&state, cmd).await?;
    let xml = res
        .get("xml")
        .and_then(|v| v.as_str())
        .ok_or("Missing XML output")?;
    let name = res
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Battery");
    let builder = dialog
        .file()
        .add_filter("Battery Profile XML", &["xml"])
        .set_file_name(&format!("{}.xml", name));
    let path = builder.blocking_save_file();
    let path = match path {
        Some(p) => p,
        None => return Ok(()),
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, xml).map_err(|e| e.to_string())
}

async fn spawn_sidecar(app: &tauri::AppHandle) -> Result<(), String> {
    let script = find_sidecar_script(app)?;

    let mut child = Command::new("node")
        .arg(&script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn sidecar: {}", e))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Failed to take sidecar stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to take sidecar stdout".to_string())?;
    let stderr = child.stderr.take();

    let state: State<'_, SidecarState> = app.state();
    {
        let mut child_guard = state.child.lock().await;
        *child_guard = Some(child);
        let mut stdin_guard = state.stdin.lock().await;
        *stdin_guard = Some(stdin);
    }

    let app_handle = app.clone();
    let pending = state.pending.clone();
    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(event) = serde_json::from_str::<SidecarEvent>(&line) {
                match event.event.as_str() {
                    "ipc-event" => {
                        if let Ok(ipc) = serde_json::from_value::<IpcEvent>(event.payload.clone()) {
                            let _ = app_handle.emit("ipc-event", ipc);
                        }
                    }
                    "error" => {
                        // If this error is a response to a pending request, fulfill it
                        // so the caller does not hang.
                        if let Some(id) = event.payload.get("request_id").and_then(|v| v.as_str()) {
                            let mut pending_guard = pending.lock().await;
                            if let Some(tx) = pending_guard.remove(id) {
                                let _ = tx.send(event.payload.clone());
                                continue;
                            }
                        }
                        if let Ok(err) =
                            serde_json::from_value::<SidecarErrorEvent>(event.payload.clone())
                        {
                            eprintln!("Sidecar error: {} {:?}", err.message, err.detail);
                            let _ = app_handle.emit("sidecar-error", err);
                        }
                    }
                    "ready" | "firmware_minimum" => {
                        let min = event
                            .payload
                            .get("minimumSupportedBuildNumber")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        let _ = app_handle.emit_to(
                            "main",
                            "ipc-event",
                            IpcEvent {
                                channel: "foxfirmware".to_string(),
                                data: min,
                            },
                        );
                    }
                    _ => {
                        // Check if this is a response to a pending request.
                        if let Some(id) = event.payload.get("request_id").and_then(|v| v.as_str()) {
                            let mut pending_guard = pending.lock().await;
                            if let Some(tx) = pending_guard.remove(id) {
                                let _ = tx.send(event.payload.clone());
                                continue;
                            }
                        }
                        // Otherwise emit as a general sidecar event.
                        let _ = app_handle.emit(&format!("sidecar:{}", event.event), event.payload);
                    }
                }
            }
        }
    });

    if let Some(stderr) = stderr {
        let app_handle = app.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("Sidecar stderr: {}", line);
                let _ = app_handle.emit(
                    "sidecar-log",
                    serde_json::json!({ "level": "error", "message": line }),
                );
            }
        });
    }

    Ok(())
}

pub fn run() {
    // DEVIATION: WebKit's internal WebProcess/GPU sandbox can crash on some
    // NVIDIA systems (ABRT inside libnvidia-gpucomp). We keep the sandbox
    // enabled by default and expose runtime escape hatches so users can opt in
    // only when needed instead of weakening the package's default posture.
    if std::env::args().any(|arg| arg == "--disable-webkit-sandbox") {
        std::env::set_var("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS", "1");
    }
    if std::env::args().any(|arg| arg == "--software-rendering") {
        // Force WebKit to render without the GPU. This avoids the NVIDIA
        // driver path while leaving the WebProcess sandbox intact.
        std::env::set_var("WEBKIT_FORCE_SOFTWARE_RENDERING", "1");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_shell::init())
        .manage(SidecarState::new())
        .manage(firmware::state::FirmwareState::new())
        .setup(|app| {
            let window = app.get_webview_window("main").unwrap();
            let _ = window.set_title("Cloudy AF");

            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = spawn_sidecar(&app_handle).await {
                    eprintln!("Failed to spawn sidecar: {}", e);
                    let _ = app_handle.emit(
                        "sidecar-error",
                        serde_json::json!({ "message": "Failed to spawn HID sidecar", "detail": e }),
                    );
                } else {
                    tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
                    let state: State<'_, SidecarState> = app_handle.state();
                    let cmd = serde_json::json!({ "type": "connect", "autoconnect": true });
                    let _ = sidecar_send(&state, cmd, None).await;
                }
            });

            let app_handle = app.handle().clone();
            let window = app.get_webview_window("main").unwrap();
            window.on_window_event(move |event| {
                if let WindowEvent::Destroyed = event {
                    let app_handle = app_handle.clone();
                    tauri::async_runtime::block_on(async move {
                        let state: State<'_, SidecarState> = app_handle.state();
                        let mut child_guard = state.child.lock().await;
                        if let Some(mut child) = child_guard.take() {
                            let _ = child.start_kill();
                            let _ = child.wait().await;
                        }
                    });
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc_send,
            get_locale,
            close_window,
            resolve_resource_path,
            read_text_file,
            write_text_file,
            read_binary_file,
            write_binary_file,
            open_file_dialog,
            save_file_dialog,
            show_error,
            open_config,
            save_config,
            import_tfr,
            export_tfr,
            import_bat,
            export_bat,
            commands::firmware::open_firmware,
            commands::firmware::download_stock,
            commands::firmware::open_stock_build,
            commands::firmware::list_patches,
            commands::firmware::apply_patch_cmd,
            commands::firmware::rollback_patch_cmd,
            commands::firmware::save_firmware,
            commands::firmware::close_firmware,
            commands::firmware::read_device_dataflash,
            commands::firmware::read_device_product_id,
            commands::firmware::flash_firmware_to_device,
            commands::firmware::restart_device_cmd,
            commands::firmware::undo_firmware_changes,
            commands::firmware::list_hid_devices,
            commands::firmware::recovery_flash,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
