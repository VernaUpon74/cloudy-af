import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getVersion } from '@tauri-apps/api/app';

// Minimal IPC shim mapping the old Electron ipcRenderer API to Tauri invoke/events.
const handlers = {};

export const ipc = {
    send(channel, data) {
        invoke('ipc_send', { request: { channel, data } })
            .catch(err => console.error('ipc.send error', channel, err));
    },
    on(channel, callback) {
        if (!handlers[channel]) {
            handlers[channel] = [];
        }
        handlers[channel].push(callback);
        // Return a function that removes the listener.
        return () => {
            const idx = handlers[channel].indexOf(callback);
            if (idx !== -1) handlers[channel].splice(idx, 1);
        };
    }
};

// Listen to backend-emitted Tauri events and dispatch to ipc.on callbacks.
listen('ipc-event', event => {
    const { channel, data } = event.payload;
    if (handlers[channel]) {
        handlers[channel].forEach(cb => cb({ sender: {} }, data));
    }
});

export async function getLocale() {
    return invoke('get_locale');
}

export async function getAppVersion() {
    try {
        return await getVersion();
    } catch (e) {
        return '1.2.0';
    }
}

export async function closeWindow() {
    return invoke('close_window');
}

export async function readTextFile(path) {
    return invoke('read_text_file', { path });
}

export async function writeTextFile(path, contents) {
    return invoke('write_text_file', { path, contents });
}

export async function readBinaryFile(path) {
    const bytes = await invoke('read_binary_file', { path });
    return new Uint8Array(bytes);
}

export async function writeBinaryFile(path, bytes) {
    const arr = bytes instanceof Uint8Array ? Array.from(bytes) : Array.from(new Uint8Array(bytes));
    return invoke('write_binary_file', { path, bytes: arr });
}

export async function openFileDialog(filters) {
    return invoke('open_file_dialog', { filters });
}

export async function saveFileDialog(defaultName, filters) {
    return invoke('save_file_dialog', { defaultName, filters });
}

export async function showError(title, message) {
    return invoke('show_error', { title, message });
}

export async function resolveResourcePath(relativePath) {
    return invoke('resolve_resource_path', { relativePath });
}

export async function openConfig() {
    return invoke('open_config');
}

export async function saveConfig(config) {
    return invoke('save_config', { config });
}

export async function importTfr() {
    return invoke('import_tfr');
}

export async function exportTfr(table) {
    return invoke('export_tfr', { table });
}

export async function importBat() {
    return invoke('import_bat');
}

export async function exportBat(table) {
    return invoke('export_bat', { table });
}

export async function openFirmware(path) {
    return invoke('open_firmware', { path });
}

export async function listPatches(handle) {
    return invoke('list_patches', { handle });
}

export async function applyPatchCmd(handle, patchId) {
    return invoke('apply_patch_cmd', { handle, patchId });
}

export async function rollbackPatchCmd(handle, patchId) {
    return invoke('rollback_patch_cmd', { handle, patchId });
}

export async function saveFirmware(handle, path) {
    return invoke('save_firmware', { handle, path });
}

export async function closeFirmware(handle) {
    return invoke('close_firmware', { handle });
}

export async function readDeviceDataflash() {
    return invoke('read_device_dataflash');
}

export async function readDeviceProductId() {
    return invoke('read_device_product_id');
}

export async function flashFirmwareToDevice(handle) {
    return invoke('flash_firmware_to_device', { handle });
}

export async function restartDeviceCmd() {
    return invoke('restart_device_cmd');
}

export async function undoFirmwareChanges(handle) {
    return invoke('undo_firmware_changes', { handle });
}

export async function listHidDevices() {
    return invoke('list_hid_devices');
}

export async function recoveryFlash(path, expectedProductId) {
    return invoke('recovery_flash', { path, expectedProductId });
}

export function onRecoveryProgress(callback) {
    return listen('recovery-progress', (event) => callback(event.payload));
}
