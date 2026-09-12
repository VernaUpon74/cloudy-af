import 'photonkit/dist/css/photon.css';
import './style.css';
import $ from 'jquery';
import {
    ipc,
    getLocale,
    closeWindow,
    readTextFile,
    readBinaryFile,
    resolveResourcePath,
    openFileDialog,
    saveFileDialog,
    openFirmware,
    downloadStock,
    openStockBuild,
    listPatches,
    applyPatchCmd,
    rollbackPatchCmd,
    saveFirmware,
    closeFirmware,
    readDeviceProductId,
    flashFirmwareToDevice,
    undoFirmwareChanges,
    listHidDevices,
    recoveryFlash,
    onRecoveryProgress,
    onFlashProgress,
    forceProductId,
    screenshot,
    listImageTables,
    readImageCmd,
    writeImageCmd,
    listStringsCmd,
    writeStringCmd,
    listResourcePacks,
    applyResourcePackCmd
} from './lib/tauri-bridge.js';

let currentHandle = null;
let currentPatches = [];
let currentPath = null;
let currentDevice = null; // { connected: bool, productId: string, name: string, line: string }
let currentFirmwareInfo = null; // { name, encryption, definition, size, buildId }

async function loadLocale() {
    try {
        const locale = await getLocale();
        const fp = await resolveResourcePath('i18n/' + locale + '.json');
        const text = await readTextFile(fp);
        const lang = JSON.parse(text);
        $('[data-lang]').each(function () {
            const key = $(this).data('lang');
            const phrase = lang[key];
            if (phrase) {
                $(this).html(phrase);
            }
        });
    } catch (err) {
        // ignore missing locale
    }
}

function setStatus(text) {
    $('#fw-status').text(text);
}

function updateConnectionStatus(connected, productId) {
    currentDevice = currentDevice || {};
    currentDevice.connected = connected;
    if (productId) {
        currentDevice.productId = productId;
    }
    const pid = currentDevice.productId || 'unknown';
    const info = lookupDeviceInfo(pid);
    currentDevice.name = info.name;
    currentDevice.line = info.line;

    $('#fw-connection-text').text(connected ? 'is connected' : 'is disconnected');
    $('#status-connection').text(connected ? 'Connected' : 'Disconnected');
    $('#fw-device-name').text(connected ? (info.name || pid) : 'No device');

    $('#status-product-id').text(pid);
    $('#status-device-name').text(info.name || '—');
    $('#status-device-line').text(info.line || '—');
}

function lookupDeviceInfo(pid) {
    if (!deviceLib || !pid || pid === 'unknown') {
        return { name: null, line: null };
    }
    for (const [lineName, line] of Object.entries(deviceLib.lines || {})) {
        if (line.product_ids && line.product_ids.includes(pid)) {
            return { name: line.name || lineName, line: lineName };
        }
    }
    return { name: null, line: null };
}

async function refreshPatches() {
    if (!currentHandle) {
        return;
    }
    try {
        currentPatches = await listPatches(currentHandle);
        renderPatchList();
    } catch (err) {
        console.error('listPatches failed', err);
    }
}

function renderPatchList() {
    const $tbody = $('#patch-list tbody');
    $tbody.empty();
    const term = ($('#patch-search').val() || '').toLowerCase();
    currentPatches.forEach(patch => {
        const hay = (patch.name + ' ' + patch.author + ' ' + (patch.description || '')).toLowerCase();
        if (term && hay.indexOf(term) === -1) {
            return;
        }
        const statusClass = patch.applied ? 'status-applied' : 'status-pending';
        const statusText = patch.applied ? 'Applied' : 'Pending';
        const actionText = patch.applied ? 'Rollback' : 'Apply';
        const $row = $('<tr></tr>')
            .append($('<td></td>').text(patch.name))
            .append($('<td></td>').text(patch.version))
            .append($('<td></td>').text(patch.author))
            .append($('<td></td>').append($('<span></span>').addClass(statusClass).text(statusText)))
            .append($('<td></td>').append(
                $('<button></button>')
                    .addClass('btn btn-mini btn-default')
                    .text(actionText)
                    .click((e) => { e.stopPropagation(); togglePatch(patch.id); })
            ));
        $row.click(() => showPatchDetails(patch));
        $tbody.append($row);
    });
}

function showPatchDetails(patch) {
    $('#patch-details').html(
        `<strong>${patch.name}</strong> v${patch.version} by ${patch.author}<br>` +
        `<em>${patch.description || 'No description'}</em>`
    );
}

async function togglePatch(patchId) {
    if (!currentHandle) {
        return;
    }
    const patch = currentPatches.find(p => p.id === patchId);
    if (!patch) {
        return;
    }
    try {
        const result = patch.applied
            ? await rollbackPatchCmd(currentHandle, patchId)
            : await applyPatchCmd(currentHandle, patchId);
        if (result.ok) {
            await refreshPatches();
        } else {
            alert(result.error || 'Operation failed');
        }
    } catch (err) {
        console.error('togglePatch failed', err);
        alert(err.toString());
    }
}

async function applyAllPendingPatches() {
    if (!currentHandle) return;
    const pending = currentPatches.filter(p => !p.applied);
    if (pending.length === 0) {
        setStatus('No pending patches to apply');
        return;
    }
    let applied = 0;
    let failed = 0;
    for (const patch of pending) {
        try {
            const result = await applyPatchCmd(currentHandle, patch.id);
            if (result.ok) {
                applied++;
            } else {
                failed++;
                console.error('apply failed', patch.id, result.error);
            }
        } catch (err) {
            failed++;
            console.error('apply exception', patch.id, err);
        }
    }
    await refreshPatches();
    setStatus(`Applied ${applied} patch(es); ${failed} failed.`);
}

async function rollbackAllAppliedPatches() {
    if (!currentHandle) return;
    const applied = currentPatches.filter(p => p.applied);
    if (applied.length === 0) {
        setStatus('No applied patches to rollback');
        return;
    }
    let rolled = 0;
    let failed = 0;
    for (const patch of applied) {
        try {
            const result = await rollbackPatchCmd(currentHandle, patch.id);
            if (result.ok) {
                rolled++;
            } else {
                failed++;
                console.error('rollback failed', patch.id, result.error);
            }
        } catch (err) {
            failed++;
            console.error('rollback exception', patch.id, err);
        }
    }
    await refreshPatches();
    setStatus(`Rolled back ${rolled} patch(es); ${failed} failed.`);
}

$('#patch-search').on('input', () => renderPatchList());
$('#patches-apply-all').click(applyAllPendingPatches);
$('#patches-rollback-all').click(rollbackAllAppliedPatches);
$('#patches-reload').click(refreshPatches);

async function doOpenFirmware() {
    try {
        const path = await openFileDialog([
            { name: 'Firmware binary', extensions: ['bin'] },
            { name: 'All files', extensions: ['*'] }
        ]);
        if (!path) {
            return;
        }
        currentPath = path;
        const info = await openFirmware(path);
        await finishOpenFirmware(info);
    } catch (err) {
        console.error('openFirmware failed', err);
        alert(err.toString());
    }
}

// Shared post-open path: handle storage, status, buttons, patch list.
async function finishOpenFirmware(info) {
    currentHandle = info.handle;
    currentFirmwareInfo = {
        name: info.name,
        encryption: info.encryption,
        definition: info.definition || info.name,
        size: info.size,
        buildId: info.build_id || null,
        matchKind: info.match_kind || 'file'
    };
    setStatus(`${info.name} (${info.encryption})`);
    updateStatusMetadata();
    updateButtonStates();
    await refreshPatches();
    resetEditorData();
    await refreshActiveEditorTab();
}

function updateStatusMetadata() {
    const fi = currentFirmwareInfo;
    $('#status-fw-loaded').text(fi ? 'Yes' : 'No');
    $('#status-fw-path').text(currentPath || (fi && fi.buildId ? `(stock ${fi.buildId})` : '—'));
    $('#status-fw-definition').text(fi ? fi.definition : '—');
    $('#status-fw-encryption').text(fi ? fi.encryption : '—');
    $('#status-fw-size').text(fi ? (fi.size + ' bytes') : '—');
    $('#status-fw-build').text(fi && fi.buildId ? fi.buildId : '—');
}

async function doDownloadStock() {
    try {
        const info = await downloadStock();
        await finishOpenStock(info);
    } catch (err) {
        console.error('downloadStock failed', err);
        // Only the unknown-product-id error ("unknown product id ...; known
        // lines: ...", see download_stock in commands/firmware.rs) offers the
        // explicit build picker; hardware/communication failures surface
        // the real error like doOpenFirmware does.
        if (err.toString().includes('known lines:')) {
            await promptStockBuild(err);
        } else {
            alert(err.toString());
        }
    }
}

async function finishOpenStock(info) {
    currentPath = null; // no user file behind a stock build; Save -> Save As
    await finishOpenFirmware(info);
    if (info.match_kind === 'line_only') {
        alert('Device firmware version not in the bundled library — the current image cannot be preserved; Undo will restore this stock build, not your current firmware.');
    }
}

// Auto-load the device-appropriate stock build: on Firmware Editor open when
// a device is already connected, and via the recovery-tab Product ID poll
// when one connects later. Silent on failure (no device, unknown product) —
// the user can still open a file or click Download FW.
let autoStockLoadedForPid = null;

async function autoLoadStockFirmware(pid) {
    if (currentHandle || autoStockLoadedForPid === pid) {
        return;
    }
    try {
        setStatus('Loading device firmware…');
        const info = await downloadStock();
        autoStockLoadedForPid = pid;
        await finishOpenStock(info);
    } catch (err) {
        if (!currentHandle) {
            setStatus('No firmware loaded');
        }
        console.log('auto stock load skipped:', err.toString());
    }
}

async function promptStockBuild(err) {
    let buildList = '';
    try {
        const libPath = await resolveResourcePath('firmware/devices.json');
        const lib = JSON.parse(await readTextFile(libPath));
        buildList = lib.builds.map(b => `${b.id} (${b.line})`).join('\n');
    } catch (e) {
        console.error('could not read devices.json', e);
    }
    const buildId = prompt(
        err.toString() +
        (buildList ? '\n\nAvailable stock builds:\n' + buildList : '') +
        '\n\nEnter a build id to open, or leave empty to cancel:'
    );
    if (!buildId || !buildId.trim()) {
        return;
    }
    try {
        const info = await openStockBuild(buildId.trim());
        await finishOpenStock(info);
    } catch (err2) {
        console.error('openStockBuild failed', err2);
        alert(err2.toString());
    }
}

async function doSaveFirmware() {
    if (!currentHandle) {
        return;
    }
    if (!currentPath) {
        return doSaveAsFirmware();
    }
    try {
        await saveFirmware(currentHandle, currentPath);
        setStatus($('#fw-status').text() + ' — saved');
    } catch (err) {
        console.error('saveFirmware failed', err);
        alert(err.toString());
    }
}

async function doSaveAsFirmware() {
    if (!currentHandle) {
        return;
    }
    try {
        const path = await saveFileDialog('firmware.bin', [
            { name: 'Firmware binary', extensions: ['bin'] }
        ]);
        if (!path) {
            return;
        }
        currentPath = path;
        await saveFirmware(currentHandle, path);
        setStatus($('#fw-status').text() + ' — saved');
    } catch (err) {
        console.error('saveFirmware failed', err);
        alert(err.toString());
    }
}

async function doFlashFirmware() {
    if (!currentHandle) {
        return;
    }
    let productId = 'unknown';
    try {
        productId = await readDeviceProductId();
    } catch (err) {
        console.warn('Could not read device Product ID', err);
    }
    const confirmed = confirm(`Flash modified firmware to device?\nProduct ID: ${productId}\nThis will overwrite the device firmware.`);
    if (!confirmed) {
        return;
    }
    setStatus('Flashing firmware to device…');
    flashInProgress = true; // stop recovery-tab Product ID polling during the flash
    try {
        await flashFirmwareToDevice(currentHandle);
        setStatus($('#fw-status').text() + ' — flashed');
        $('#undo-changes').prop('disabled', false);
    } catch (err) {
        console.error('flashFirmware failed', err);
        alert(err.toString());
        setStatus('Flash failed');
    } finally {
        flashInProgress = false;
    }
}

async function doUndoChanges() {
    if (!currentHandle) {
        return;
    }
    const confirmed = confirm('Restore original firmware backup to device?');
    if (!confirmed) {
        return;
    }
    setStatus('Restoring original firmware…');
    flashInProgress = true;
    try {
        await undoFirmwareChanges(currentHandle);
        setStatus('Original firmware restored');
    } catch (err) {
        console.error('undoFirmwareChanges failed', err);
        alert(err.toString());
        setStatus('Restore failed');
    } finally {
        flashInProgress = false;
    }
}

function updateButtonStates() {
    const hasHandle = !!currentHandle;
    $('#save-firmware').prop('disabled', !hasHandle);
    $('#save-as-firmware').prop('disabled', !hasHandle);
    $('#flash-firmware').prop('disabled', !hasHandle);
    // Undo stays disabled until a flash has happened.
}

function initTabs() {
    $('.tab-item').click(function () {
        $('.tab-item').removeClass('active');
        $(this).addClass('active');
        const tab = $(this).data('tab');
        currentTab = tab;
        $('#tab-status, #tab-patches, #tab-images, #tab-strings, #tab-resourcepacks, #tab-recovery').hide();
        $('#tab-' + tab).show();
        if (tab === 'images') {
            refreshImagesTab();
        } else if (tab === 'strings') {
            refreshStringsTab();
        } else if (tab === 'resourcepacks') {
            refreshResourcePacksTab();
        }
    });
    // Default to the Status tab on first open.
    $('.tab-item[data-tab="status"]').click();
}

// ---------------------------------------------------------------------------
// Emergency Recovery tab
// ---------------------------------------------------------------------------

let recoveryPath = null;
let recoveryKind = null;    // 'rescue' (stock) | 'af' (ArcticFox build) | 'custom'
let recoveryBusy = false;
let flashInProgress = false; // firmware-editor flash/undo in progress (shared HID bus)
let detectedPid = null;
let pidAuto = false;         // pid input was auto-filled (may be overwritten)
let deviceLib = null;        // parsed resources/firmware/devices.json
let lastFlashWasRescue = false;

function recoveryLog(msg) {
    const $log = $('#recovery-log');
    $log.append(document.createTextNode(new Date().toLocaleTimeString() + '  ' + msg + '\n'));
    $log.scrollTop($log[0].scrollHeight);
}

async function loadDeviceLib() {
    try {
        const libPath = await resolveResourcePath('firmware/devices.json');
        deviceLib = JSON.parse(await readTextFile(libPath));
    } catch (err) {
        console.error('could not load firmware library', err);
    }
}

function lineForPid(pid) {
    if (!deviceLib || !pid) {
        return null;
    }
    for (const [name, line] of Object.entries(deviceLib.lines)) {
        if (line.product_ids.includes(pid)) {
            return name;
        }
    }
    return null;
}

// Newest bundled ArcticFox build for a device line (build ids end in YYMMDD).
function latestAfBuildForLine(line) {
    const builds = (deviceLib.builds || []).filter(b => b.line === line);
    builds.sort((a, b) => b.id.localeCompare(a.id));
    return builds[0] || null;
}

function setPidGuard(pid) {
    $('#recovery-pid').val(pid);
    pidAuto = true;
}

// Auto-select the image appropriate for the detected device:
//  - detected device with a bundled stock rescue image -> that image
//    (after the original firmware is restored, ArcticFox can be flashed);
//  - detected device without one -> the newest bundled ArcticFox build for
//    its line (AF images are universal within a device line);
//  - no device detected yet -> the single bundled rescue default, guarded
//    by its Product ID so it can never land on a wrong device.
async function autoSelectRecoveryImage() {
    if (!deviceLib || recoveryBusy) {
        return;
    }
    if (recoveryPath && recoveryKind === 'custom') {
        return; // user picked a file explicitly; don't override
    }
    try {
        const rescue = (deviceLib.rescue || []).find(r => detectedPid && r.product_ids.includes(detectedPid))
            || (!detectedPid && (deviceLib.rescue || [])[0]) || null;
        if (rescue) {
            recoveryPath = await resolveResourcePath('firmware/' + rescue.file);
            recoveryKind = 'rescue';
            $('#recovery-path').text(rescue.label + ' (' + rescue.file + ')');
            if (!detectedPid && rescue.product_ids.length === 1 && (!$('#recovery-pid').val() || pidAuto)) {
                setPidGuard(rescue.product_ids[0]);
            }
        } else {
            // No bundled stock image for the detected device. Do NOT fall back
            // to ArcticFox here: a bricked/looping device does not boot AF
            // directly (established on hardware — see test-fixtures/rescue/
            // README.md); it must get the original manufacturer firmware
            // first. And never keep a stale auto-selected image around — with
            // the guard auto-filled to the device's own Product ID, a leftover
            // Pico image would flash onto a non-Pico device.
            if (recoveryKind !== 'custom') {
                recoveryPath = null;
                recoveryKind = null;
                $('#recovery-path').text(
                    'no bundled stock image for ' + (detectedPid || 'this device') +
                    ' — choose the original manufacturer firmware file'
                );
                $('#recovery-start').prop('disabled', true);
            }
            return;
        }
        $('#recovery-start').prop('disabled', recoveryBusy);
    } catch (err) {
        console.error('could not resolve bundled image', err);
    }
}

async function refreshRecoveryDevices() {
    try {
        const devices = await listHidDevices();
        const $list = $('#recovery-devices');
        $list.html('');
        if (devices.length === 0) {
            $list.append('<li>none</li>');
        } else {
            devices.forEach(d => {
                $list.append($('<li>').text(`${d.path} — ${d.product || 'HID Transfer'}${d.serial ? ' (S/N ' + d.serial + ')' : ''}`));
            });
        }
    } catch (err) {
        // hidapi unavailable; leave list as-is
    }
    // Detect the device's Product ID so the right images auto-select. Skip
    // while any flash is running: a 0x35 dataflash read interleaved into a
    // 0xC3 firmware stream corrupts the flash (brick risk).
    if (!recoveryBusy && !flashInProgress) {
        try {
            const pid = (await readDeviceProductId()).replace(/\0+$/, '').trim();
            if (!/^[A-Z0-9]{4}$/.test(pid)) {
                return; // garbage read while the device flaps — not a Product ID
            }
            if (pid !== detectedPid) {
                detectedPid = pid;
                recoveryLog('detected device Product ID: ' + pid);
                if (!$('#recovery-pid').val() || pidAuto) {
                    setPidGuard(pid);
                }
                if (!$('#force-pid').val()) {
                    $('#force-pid').val(pid);
                }
                await autoSelectRecoveryImage();
                await autoLoadStockFirmware(pid);
            }
        } catch (err) {
            // no device, or dataflash unreadable while it flaps — ignore
        }
    }
}

async function runRecoveryFlash() {
    const pid = ($('#recovery-pid').val() || '').trim();
    const wasRescue = recoveryKind === 'rescue';
    recoveryBusy = true;
    $('#recovery-start').prop('disabled', true);
    recoveryLog('recovery started: ' + recoveryPath);
    try {
        await recoveryFlash(recoveryPath, pid || null);
        recoveryLog('SUCCESS — device rebooted into the flashed firmware');
        lastFlashWasRescue = wasRescue;
    } catch (err) {
        recoveryLog('FAILED: ' + err.toString());
        lastFlashWasRescue = false;
    } finally {
        recoveryBusy = false;
        $('#recovery-start').prop('disabled', !recoveryPath);
    }
}

$('#recovery-choose').click(async () => {
    try {
        const path = await openFileDialog([{ name: 'Firmware', extensions: ['bin'] }]);
        if (path) {
            recoveryPath = path;
            recoveryKind = 'custom';
            $('#recovery-path').text(path);
            $('#recovery-start').prop('disabled', recoveryBusy);
        }
    } catch (err) {
        console.error('openFileDialog failed', err);
    }
});

$('#recovery-pid').on('input', () => {
    pidAuto = false; // manual edit wins over auto-detection
});

$('#recovery-start').click(async () => {
    if (!recoveryPath || recoveryBusy) {
        return;
    }
    const pid = ($('#recovery-pid').val() || '').trim();
    const confirmed = confirm(
        'Start emergency recovery flash?\n\n' +
        'Image: ' + recoveryPath + '\n' +
        (pid ? 'Only a device with Product ID ' + pid + ' will be flashed.\n' : 'WARNING: no Product ID guard — ANY connected device will be flashed!\n') +
        '\nThe flasher waits for the device; unplug and replug it now.'
    );
    if (!confirmed) {
        return;
    }
    await runRecoveryFlash();
    // After restoring the original manufacturer firmware the device is ready
    // for ArcticFox — offer the matching bundled build right away.
    if (lastFlashWasRescue) {
        recoveryLog('original firmware restored — the device can now receive ArcticFox');
        const line = lineForPid(detectedPid || pid);
        const af = latestAfBuildForLine(line);
        if (!af) {
            recoveryLog('no bundled ArcticFox build for this device line; use the Firmware Editor');
            return;
        }
        if (confirm(`Original firmware restored.\n\nFlash ArcticFox build ${af.id} to the device now?`)) {
            try {
                recoveryPath = await resolveResourcePath('firmware/' + af.file);
                recoveryKind = 'af'; // AF follow-up is not a stock-rescue image
                $('#recovery-path').text('ArcticFox ' + af.id + ' (' + af.file + ')');
                recoveryLog('flashing ArcticFox ' + af.id + '…');
                await runRecoveryFlash();
            } catch (err) {
                recoveryLog('ArcticFox flash setup failed: ' + err.toString());
            }
        }
    }
});

onRecoveryProgress(msg => recoveryLog(msg));
setInterval(refreshRecoveryDevices, 2000);
loadDeviceLib().then(() => autoSelectRecoveryImage());
refreshRecoveryDevices();

// NFE "Force PID": repair an erased/wrong dataflash product ID (see the
// recovery tab text). Prefill from the auto-detected ID when available.
$('#force-pid-btn').click(async () => {
    const pid = ($('#force-pid').val() || '').trim().toUpperCase();
    if (!/^[A-Z0-9]{4}$/.test(pid)) {
        alert('Enter a 4-character Product ID (e.g. M065).');
        return;
    }
    if (flashInProgress || recoveryBusy) {
        alert('Wait for the current flash to finish.');
        return;
    }
    const confirmed = confirm(
        `Repair the device dataflash Product ID to ${pid}?\n\n` +
        'Rewrites bytes 316..320, clears the boot flag, restarts the device, ' +
        'and verifies the write. Use this after a failed flash that left the ' +
        'device crash-looping or undetected.'
    );
    if (!confirmed) {
        return;
    }
    recoveryLog('force PID: writing ' + pid + '…');
    try {
        await forceProductId(pid);
        recoveryLog('SUCCESS — product ID repaired, device rebooted into firmware');
    } catch (err) {
        recoveryLog('FAILED: ' + err.toString());
    }
});

$('#open-firmware').click(doOpenFirmware);
$('#download-stock').click(doDownloadStock);
$('#save-firmware').click(doSaveFirmware);
$('#save-as-firmware').click(doSaveAsFirmware);
$('#flash-firmware').click(doFlashFirmware);
$('#undo-changes').click(doUndoChanges);

// Flash progress (incl. the recovery-style wait-for-replug fallback) updates
// the status line live, so the user knows to unplug/replug the device when
// it refuses to soft-switch into bootloader mode.
onFlashProgress((msg) => setStatus(msg));

updateButtonStates();

ipc.on('data', (event, data) => {
    // Received when the window is opened via ipc.send('firmware', data).
    // Future: pre-select firmware path from data.
    console.log('firmware window received data', data);
});

// Connection status events from the HID sidecar (same channel the main window uses).
ipc.on('connect', (event, status) => {
    updateConnectionStatus(Boolean(status), currentDevice && currentDevice.productId);
});

$('#status-screenshot').click(async () => {
    try {
        setStatus('Capturing screenshot…');
        const b64 = await screenshot();
        const img = new Image();
        img.onload = () => {
            const c = $('#status-screenshot-canvas')[0];
            c.width = img.width;
            c.height = img.height;
            const ctx = c.getContext('2d');
            ctx.drawImage(img, 0, 0);
            $('#status-screenshot-preview').show();
            c.toBlob((blob) => {
                const url = URL.createObjectURL(blob);
                $('#status-screenshot-link').attr('href', url).show().text('Download ' + img.width + '×' + img.height + ' PNG');
            }, 'image/png');
        };
        img.src = 'data:image/png;base64,' + b64;
        setStatus('Screenshot captured');
    } catch (err) {
        console.error('screenshot failed', err);
        setStatus('Screenshot failed: ' + err.toString());
        alert('Screenshot failed: ' + err.toString());
    }
});

loadLocale();
initTabs();
attachImageCanvasHandlers();

// Keyboard shortcuts for the Firmware Editor window.
$(document).on('keydown', (e) => {
    if (e.ctrlKey || e.metaKey) {
        if (e.key === 's') {
            e.preventDefault();
            doSaveFirmware();
        } else if (e.key === 'o') {
            e.preventDefault();
            doOpenFirmware();
        }
    }
    if (e.key === 'Escape') {
        selectedCell = -1;
        renderStringCells();
    }
});

// Firmware Editor opened with a device already connected: load its
// appropriate stock build right away and populate the Status page.
(async () => {
    try {
        const pid = (await readDeviceProductId()).replace(/\0+$/, '').trim();
        if (/^[A-Z0-9]{4}$/.test(pid)) {
            await autoLoadStockFirmware(pid);
            updateConnectionStatus(true, pid);
        }
    } catch (err) {
        // no device connected — user can open a file or click Download FW
        updateConnectionStatus(false, null);
    }
})();
// ---------------------------------------------------------------------------
// Images / Strings / Resource Packs editor tabs
// ---------------------------------------------------------------------------
// The currently active editor tab (set by initTabs) so the open-firmware
// transition can refresh whichever pane is visible.
let currentTab = 'patches';

// Invalidate all per-firmware caches whenever the open firmware changes.
function resetEditorData() {
    imageTables = null;
    fwDefinition = '';
    imageSel = null;
    currentImagesBlock = null;
    stringsList = [];
    stringSel = null;
    stringGlyphs = [];
    stringSlots = 0;
    selectedCell = -1;
    glyphCache = new Map();
    glyphMaxIndex = 0;
    block1Slots = [];
    resourcePacks = [];
    rpSel = null;
}

async function refreshActiveEditorTab() {
    if (!currentHandle) {
        return;
    }
    try {
        if (currentTab === 'images') {
            await refreshImagesTab();
        } else if (currentTab === 'strings') {
            await refreshStringsTab();
        } else if (currentTab === 'resourcepacks') {
            await refreshResourcePacksTab();
        }
    } catch (err) {
        console.error('refresh active editor tab failed', err);
    }
}

// Shared 1bpp pixel helpers. Images/strings/resource packs store 1 bit per
// pixel, row-major (bit y*width+x), packed MSB-first into bytes with the last
// byte zero-padded.
function pixelsFromBase64(b64, w, h) {
    const raw = atob(b64);
    const bits = new Uint8Array(w * h);
    const limit = Math.min(raw.length, Math.ceil((w * h) / 8));
    for (let i = 0; i < limit; i++) {
        const byte = raw.charCodeAt(i);
        for (let bit = 0; bit < 8; bit++) {
            const idx = i * 8 + bit;
            if (idx >= w * h) {
                break;
            }
            bits[idx] = (byte >> (7 - bit)) & 1;
        }
    }
    return bits;
}

function bitsToBase64(bits) {
    const bytes = new Uint8Array(Math.ceil(bits.length / 8));
    for (let i = 0; i < bits.length; i++) {
        if (bits[i]) {
            bytes[i >> 3] |= 0x80 >> (i & 7);
        }
    }
    let s = '';
    for (let i = 0; i < bytes.length; i++) {
        s += String.fromCharCode(bytes[i]);
    }
    return btoa(s);
}

function themeColor(name) {
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function drawBits(canvas, bits, w, h, scale) {
    canvas.width = w * scale;
    canvas.height = h * scale;
    const ctx = canvas.getContext('2d');
    ctx.fillStyle = themeColor('--bg-color') || '#000';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.fillStyle = themeColor('--fg-color') || '#fff';
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            if (bits[y * w + x]) {
                ctx.fillRect(x * scale, y * scale, scale, scale);
            }
        }
    }
}

function bitsFromCanvas(canvas, w, h) {
    const ctx = canvas.getContext('2d');
    const data = ctx.getImageData(0, 0, w, h).data;
    const bits = new Uint8Array(w * h);
    for (let i = 0; i < w * h; i++) {
        const lum = 0.299 * data[i * 4] + 0.587 * data[i * 4 + 1] + 0.114 * data[i * 4 + 2];
        bits[i] = lum >= 128 ? 1 : 0;
    }
    return bits;
}

function downloadBlob(blob, filename) {
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// Mark the firmware as modified (enable Save + Undo) after any successful
// mutation, mirroring how the patch toggle switches these buttons.
function markMutated() {
    $('#save-firmware').prop('disabled', false);
    $('#undo-changes').prop('disabled', false);
}
// ---------------------------------------------------------------------------
// Images tab
// ---------------------------------------------------------------------------
let imageTables = null;      // raw list_image_tables result for the open firmware
let fwDefinition = '';       // current firmware's image-table definition name
let imageSel = null;         // { block, index, ref, data, w, h, bits }
let currentImagesBlock = null; // active block (1-based) for the Images pane
let imageTool = 'draw';      // 'draw' | 'erase' | 'move'
let imageDrag = false;       // mouse is down on canvas
let imageLastCell = null;    // last edited cell while dragging
let imageCanvasScale = 4;    // current zoom level
let imageUndoStack = [];     // snapshots of imageSel.bits for undo
let imageRedoStack = [];     // snapshots for redo
let imageClipboard = null;   // { w, h, bits } copied from an image slot

function pushImageUndo() {
    if (!imageSel) return;
    imageUndoStack.push(imageSel.bits.slice());
    if (imageUndoStack.length > 50) imageUndoStack.shift();
    imageRedoStack = [];
    updateImageUndoButtons();
}

function updateImageUndoButtons() {
    $('#images-undo').prop('disabled', imageUndoStack.length === 0 || !imageSel);
    $('#images-redo').prop('disabled', imageRedoStack.length === 0 || !imageSel);
}

function imageUndo() {
    if (!imageSel || imageUndoStack.length === 0) return;
    imageRedoStack.push(imageSel.bits.slice());
    imageSel.bits = imageUndoStack.pop();
    drawBits($('#images-canvas')[0], imageSel.bits, imageSel.w, imageSel.h, imageCanvasScale);
    updateImageUndoButtons();
    persistImage();
}

function imageRedo() {
    if (!imageSel || imageRedoStack.length === 0) return;
    imageUndoStack.push(imageSel.bits.slice());
    imageSel.bits = imageRedoStack.pop();
    drawBits($('#images-canvas')[0], imageSel.bits, imageSel.w, imageSel.h, imageCanvasScale);
    updateImageUndoButtons();
    persistImage();
}

function flipImageBits(bits, w, h, horizontal) {
    const out = new Array(bits.length);
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            const srcX = horizontal ? (w - 1 - x) : x;
            const srcY = horizontal ? y : (h - 1 - y);
            out[y * w + x] = bits[srcY * w + srcX];
        }
    }
    return out;
}

function rotateImageBits(bits, w, h, clockwise) {
    const out = new Array(bits.length);
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            let dx, dy;
            if (clockwise) {
                dx = h - 1 - y;
                dy = x;
            } else {
                dx = y;
                dy = w - 1 - x;
            }
            out[dy * h + dx] = bits[y * w + x];
        }
    }
    return out;
}

function setImagesVisible(hasHandle) {
function setImagesVisible(hasHandle) {
    $('#images-min').css('display', hasHandle ? 'none' : '');
    $('#images-main').css('display', hasHandle ? '' : 'none');
}

async function refreshImagesTab() {
    if (!currentHandle) {
        setImagesVisible(false);
        return;
    }
    setImagesVisible(true);
    try {
        imageTables = await listImageTables(currentHandle);
        fwDefinition = imageTables.definition || '';
    } catch (err) {
        console.error('list_image_tables failed', err);
        setStatus('Image tab load failed: ' + err.toString());
    }
    renderImageBlocks();
}

function renderImageBlocks() {
    const $blocks = $('#images-blocks');
    $blocks.empty();
    $('#images-export, #images-import, #images-invert, #images-clear').prop('disabled', true);
    $('#images-info').text('');
    const c = $('#images-canvas')[0];
    c.width = 0;
    c.height = 0;
    if (!imageTables) {
        return;
    }
    (imageTables.blocks || []).forEach(tb => {
        $blocks.append($('<div></div>').addClass('fw-title').text('Block ' + tb.block));
        (tb.slots || []).forEach(slot => {
            const label = '0x' + slot.index.toString(16) + ' — ' + slot.width + '×' + slot.height;
            const $entry = $('<div></div>')
                .addClass('fw-entry')
                .attr('data-block', tb.block)
                .attr('data-index', slot.index)
                .text(label);
            $entry.click(() => selectImage(tb.block, slot));
            $blocks.append($entry);
        });
    });
}

async function selectImage(block, slot) {
    if (!currentHandle) {
        return;
    }
    try {
        const img = await readImageCmd(currentHandle, block, slot.index);
        imageSel = {
            block,
            index: slot.index,
            ref: slot.reference_offset,
            data: slot.data_offset,
            w: img.width,
            h: img.height,
            bits: pixelsFromBase64(img.pixels_base64, img.width, img.height)
        };
        currentImagesBlock = block;
        renderImageSelection();
    } catch (err) {
        alert('Failed to read image: ' + err.toString());
    }
}

function renderImageSelection() {
    $('#images-blocks .fw-entry').removeClass('selected');
    if (imageSel) {
        $('#images-blocks .fw-entry[data-block="' + imageSel.block + '"][data-index="' + imageSel.index + '"]').addClass('selected');
        drawBits($('#images-canvas')[0], imageSel.bits, imageSel.w, imageSel.h, imageCanvasScale);
        $('#images-info').text(
            'Image 0x' + imageSel.index.toString(16) + ', ' + imageSel.w + 'x' + imageSel.h +
            ', ref 0x' + imageSel.ref.toString(16) + ', data 0x' + imageSel.data.toString(16) +
            ', zoom ' + imageCanvasScale + 'x'
        );
        $('#images-export, #images-export-all, #images-import, #images-invert, #images-clear, #images-flip-h, #images-flip-v, #images-rotate-cw, #images-rotate-ccw, #images-shift-l, #images-shift-r, #images-shift-u, #images-shift-d, #images-copy').prop('disabled', false);
        $('#images-paste').prop('disabled', !imageClipboard || (imageSel.w !== imageClipboard.w || imageSel.h !== imageClipboard.h));
        updateImageUndoButtons();
    }
}

function imageCellFromEvent(e) {
    const c = $('#images-canvas')[0];
    const rect = c.getBoundingClientRect();
    const x = Math.floor((e.clientX - rect.left) / imageCanvasScale);
    const y = Math.floor((e.clientY - rect.top) / imageCanvasScale);
    if (x < 0 || y < 0 || x >= imageSel.w || y >= imageSel.h) {
        return null;
    }
    return { x, y, idx: y * imageSel.w + x };
}

function applyImageTool(cell, erase) {
    if (!imageSel || !cell) {
        return;
    }
    if (imageLastCell && imageLastCell.idx === cell.idx) {
        return;
    }
    const value = erase ? 0 : 1;
    if (imageSel.bits[cell.idx] !== value) {
        pushImageUndo();
        imageSel.bits[cell.idx] = value;
        drawBits($('#images-canvas')[0], imageSel.bits, imageSel.w, imageSel.h, imageCanvasScale);
    }
    imageLastCell = cell;
}

function setImageTool(tool) {
    imageTool = tool;
    $('#images-tool-draw, #images-tool-erase, #images-tool-move').removeClass('active');
    $('#images-tool-' + tool).addClass('active');
    const c = $('#images-canvas')[0];
    if (tool === 'move') {
        c.style.cursor = 'grab';
    } else {
        c.style.cursor = 'crosshair';
    }
}

function attachImageCanvasHandlers() {
    const $c = $('#images-canvas');
    $c.on('mousedown', (e) => {
        if (!imageSel || imageTool === 'move') return;
        imageDrag = true;
        imageLastCell = null;
        const cell = imageCellFromEvent(e);
        applyImageTool(cell, imageTool === 'erase' || e.shiftKey);
    });
    $c.on('mousemove', (e) => {
        if (!imageDrag || !imageSel || imageTool === 'move') return;
        const cell = imageCellFromEvent(e);
        applyImageTool(cell, imageTool === 'erase' || e.shiftKey);
    });
    $(window).on('mouseup', () => {
        if (imageDrag) {
            imageDrag = false;
            imageLastCell = null;
            if (imageSel) {
                persistImage();
            }
        }
    });
    $c.on('wheel', (e) => {
        if (!imageSel) return;
        e.preventDefault();
        const delta = e.originalEvent.deltaY > 0 ? -1 : 1;
        const newScale = Math.max(1, Math.min(16, imageCanvasScale + delta));
        if (newScale !== imageCanvasScale) {
            imageCanvasScale = newScale;
            renderImageSelection();
        }
    });
}

// Persist the currently selected image (bits already edited in-memory), then
// refresh the slot list (slot dims may have changed) and reselect the image.
async function persistImage() {
    const s = imageSel;
    if (!s) {
        return;
    }
    await writeImageCmd(currentHandle, s.block, s.index, s.w, s.h, bitsToBase64(s.bits));
    markMutated();
    await refreshImagesTab();
    const tb = (imageTables.blocks || []).find(b => b.block === s.block);
    const slot = tb && tb.slots.find(sl => sl.index === s.index);
    if (slot) {
        await selectImage(s.block, slot);
    }
}

$('#images-export').click(() => {
    if (!imageSel) {
        return;
    }
    const canvas = $('#images-canvas')[0];
    const filename = 'image_0x' + imageSel.index.toString(16) + '.png';
    canvas.toBlob((blob) => {
        downloadBlob(blob, filename);
    }, 'image/png');
});

async function exportAllImages() {
    if (!currentHandle || !imageTables) {
        return;
    }
    let exported = 0;
    for (const tb of imageTables.blocks || []) {
        for (const slot of tb.slots || []) {
            try {
                const img = await readImageCmd(currentHandle, tb.block, slot.index);
                const bits = pixelsFromBase64(img.pixels_base64, img.width, img.height);
                const off = document.createElement('canvas');
                off.width = img.width;
                off.height = img.height;
                const ctx = off.getContext('2d');
                ctx.fillStyle = '#000';
                ctx.fillRect(0, 0, off.width, off.height);
                ctx.fillStyle = '#fff';
                for (let y = 0; y < img.height; y++) {
                    for (let x = 0; x < img.width; x++) {
                        if (bits[y * img.width + x]) {
                            ctx.fillRect(x, y, 1, 1);
                        }
                    }
                }
                const filename = 'block' + tb.block + '_0x' + slot.index.toString(16) + '.png';
                off.toBlob((blob) => {
                    downloadBlob(blob, filename);
                }, 'image/png');
                exported++;
            } catch (err) {
                console.error('export image failed', slot.index, err);
            }
        }
    }
    setStatus('Exported ' + exported + ' images');
}

$('#images-export-all').click(exportAllImages);
$('#images-tool-draw').click(() => setImageTool('draw'));
$('#images-tool-erase').click(() => setImageTool('erase'));
$('#images-tool-move').click(() => setImageTool('move'));

$('#images-invert').click(async () => {
    if (!imageSel) {
        return;
    }
    pushImageUndo();
    for (let i = 0; i < imageSel.bits.length; i++) {
        imageSel.bits[i] = 1 - imageSel.bits[i];
    }
    await persistImage();
});

$('#images-clear').click(async () => {
    if (!imageSel) {
        return;
    }
    pushImageUndo();
    imageSel.bits.fill(0);
    await persistImage();
});

$('#images-flip-h').click(async () => {
    if (!imageSel) return;
    pushImageUndo();
    imageSel.bits = flipImageBits(imageSel.bits, imageSel.w, imageSel.h, true);
    await persistImage();
});

$('#images-flip-v').click(async () => {
    if (!imageSel) return;
    pushImageUndo();
    imageSel.bits = flipImageBits(imageSel.bits, imageSel.w, imageSel.h, false);
    await persistImage();
});

$('#images-rotate-cw').click(async () => {
    if (!imageSel) return;
    pushImageUndo();
    imageSel.bits = rotateImageBits(imageSel.bits, imageSel.w, imageSel.h, true);
    [imageSel.w, imageSel.h] = [imageSel.h, imageSel.w];
    await persistImage();
});

$('#images-rotate-ccw').click(async () => {
    if (!imageSel) return;
    pushImageUndo();
    imageSel.bits = rotateImageBits(imageSel.bits, imageSel.w, imageSel.h, false);
    [imageSel.w, imageSel.h] = [imageSel.h, imageSel.w];
    await persistImage();
});

$('#images-undo').click(imageUndo);
$('#images-redo').click(imageRedo);

function shiftImageBits(bits, w, h, dx, dy) {
    const out = new Array(bits.length).fill(0);
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            const sx = x - dx;
            const sy = y - dy;
            if (sx >= 0 && sx < w && sy >= 0 && sy < h) {
                out[y * w + x] = bits[sy * w + sx];
            }
        }
    }
    return out;
}

async function shiftImage(dx, dy) {
    if (!imageSel) return;
    pushImageUndo();
    imageSel.bits = shiftImageBits(imageSel.bits, imageSel.w, imageSel.h, dx, dy);
    await persistImage();
}

$('#images-shift-l').click(() => shiftImage(-1, 0));
$('#images-shift-r').click(() => shiftImage(1, 0));
$('#images-shift-u').click(() => shiftImage(0, -1));
$('#images-shift-d').click(() => shiftImage(0, 1));

$('#images-copy').click(() => {
    if (!imageSel) return;
    imageClipboard = { w: imageSel.w, h: imageSel.h, bits: imageSel.bits.slice() };
    updateImageUndoButtons();
    $('#images-paste').prop('disabled', false);
    setStatus('Image copied to clipboard');
});

$('#images-paste').click(async () => {
    if (!imageSel || !imageClipboard) return;
    if (imageSel.w !== imageClipboard.w || imageSel.h !== imageClipboard.h) {
        alert('Clipboard image dimensions do not match this slot (' + imageClipboard.w + '×' + imageClipboard.h + ' vs ' + imageSel.w + '×' + imageSel.h + ').');
        return;
    }
    pushImageUndo();
    imageSel.bits = imageClipboard.bits.slice();
    await persistImage();
});

$('#images-import').click(async () => {
    if (!imageSel) {
        return;
    }
    const path = await openFileDialog([
        { name: 'Image', extensions: ['png', 'jpg', 'jpeg', 'bmp', 'gif'] },
        { name: 'All files', extensions: ['*'] }
    ]);
    if (!path) {
        return;
    }
    try {
        const bytes = await readBinaryFile(path);
        const blob = new Blob([bytes], { type: 'image/png' });
        const url = URL.createObjectURL(blob);
        const img = new Image();
        img.onload = async () => {
            URL.revokeObjectURL(url);
            const w = imageSel.w;
            const h = imageSel.h;
            const off = document.createElement('canvas');
            off.width = w;
            off.height = h;
            const ctx = off.getContext('2d');
            ctx.imageSmoothingEnabled = true;
            ctx.drawImage(img, 0, 0, w, h);
            imageSel.bits = bitsFromCanvas(off, w, h);
            await persistImage();
        };
        img.onerror = () => {
            URL.revokeObjectURL(url);
            alert('Could not decode image');
        };
        img.src = url;
    } catch (err) {
        alert('Import failed: ' + err.toString());
    }
});
// ---------------------------------------------------------------------------
// Strings tab
// ---------------------------------------------------------------------------
let stringsList = [];        // list_strings_cmd result
let stringSel = null;        // selected string object { index, byte_length, glyphs }
let stringGlyphs = [];       // in-memory glyph array for the selected string
let stringSlots = 0;         // byte_length (fixed slot count) of the selected string
let selectedCell = -1;       // currently selected glyph cell
let glyphCache = new Map();  // glyph index -> { w, h, bits }
let glyphMaxIndex = 0;       // highest Block-1 slot index (palette extent)
let block1Slots = [];        // Block-1 slots (the font/glyph table)

// Glyphs that NFE renders shifted down 2px in the preview only (writes are
// unaffected by this cosmetic offset).
const DESCENDERS = new Set([0x53, 0x56, 0x5C, 0x5D, 0x65]);

function setStringsVisible(hasHandle) {
    $('#strings-min').css('display', hasHandle ? 'none' : '');
    $('#strings-main').css('display', hasHandle ? '' : 'none');
}

async function refreshStringsTab() {
    if (!currentHandle) {
        setStringsVisible(false);
        return;
    }
    setStringsVisible(true);
    try {
        const tables = await listImageTables(currentHandle);
        fwDefinition = tables.definition || '';
        block1Slots = [];
        (tables.blocks || []).forEach(tb => {
            if (tb.block === 1) {
                block1Slots = tb.slots || [];
            }
        });
        glyphMaxIndex = block1Slots.reduce((m, s) => Math.max(m, s.index), 1);
        stringsList = await listStringsCmd(currentHandle);
        stringSel = null;
        stringGlyphs = [];
        stringSlots = 0;
        selectedCell = -1;
        renderStringsList();
        await renderStringsEditor();
        renderStringPalette();
    } catch (err) {
        console.error('strings tab load failed', err);
        setStatus('Strings tab load failed: ' + err.toString());
    }
}

function renderStringsList() {
    const $list = $('#strings-list');
    $list.empty();
    const term = ($('#strings-search').val() || '').toLowerCase();
    stringsList.forEach(s => {
        const glyphStr = (s.glyphs || []).join(' ');
        const idxStr = '0x' + s.index.toString(16);
        const hay = (idxStr + ' ' + glyphStr).toLowerCase();
        if (term && hay.indexOf(term) === -1) {
            return;
        }
        const $entry = $('<div></div>')
            .addClass('fw-entry')
            .attr('data-index', s.index)
            .text(idxStr + (glyphStr ? ' — ' + glyphStr : ''));
        $entry.click(() => selectString(s));
        $list.append($entry);
    });
    renderStringsListSelection();
}

$('#strings-search').on('input', () => renderStringsList());

function renderStringsListSelection() {
    $('#strings-list .fw-entry').removeClass('selected');
    if (stringSel) {
        $('#strings-list .fw-entry[data-index="' + stringSel.index + '"]').addClass('selected');
    }
}

function selectString(s) {
    stringSel = s;
    stringSlots = s.byte_length;
    stringGlyphs = fullGlyphArray((s.glyphs || []).slice());
    selectedCell = -1;
    renderStringsListSelection();
    renderStringsEditor();
}

// Normalize a glyph array to the string's fixed length (pad with 0, cap at the
// slot count) for display and for write_string_cmd.
function fullGlyphArray(source) {
    const arr = source.slice(0, stringSlots);
    while (arr.length < stringSlots) {
        arr.push(0);
    }
    return arr;
}

async function renderStringsEditor() {
    if (!stringSel) {
        const c = $('#strings-canvas')[0];
        c.width = 1;
        c.height = 1;
        $('#strings-cells').empty();
        $('#strings-capacity').text('Select a string');
        $('#strings-truncate').prop('disabled', true);
        return;
    }
    const used = stringGlyphs.filter(g => g !== 0).length;
    $('#strings-capacity').text(used + '/' + (stringSlots - 1) + ' glyphs (string length is fixed)');
    await renderStringPreview();
    await renderStringCells();
}

async function getGlyph(index) {
    if (index === 0) {
        return { w: 1, h: 1, bits: new Uint8Array(1) };
    }
    if (glyphCache.has(index)) {
        return glyphCache.get(index);
    }
    const img = await readImageCmd(currentHandle, 1, index);
    const m = {
        w: img.width,
        h: img.height,
        bits: pixelsFromBase64(img.pixels_base64, img.width, img.height)
    };
    glyphCache.set(index, m);
    return m;
}

async function renderStringPreview() {
    const $cv = $('#strings-canvas');
    const c = $cv[0];
    c.width = 1;
    c.height = 1;
    const ctx = c.getContext('2d');
    ctx.fillStyle = themeColor('--bg-color') || '#000';
    ctx.fillRect(0, 0, 1, 1);
    // Glyphs up to the first terminator (0) are the visible string.
    const strip = [];
    for (const g of stringGlyphs) {
        if (g === 0) {
            break;
        }
        strip.push(g);
    }
    if (strip.length === 0) {
        return;
    }
    const scale = 2;
    const metas = [];
    let stripW = 0;
    let stripH = 32;
    for (const g of strip) {
        const m = await getGlyph(g);
        metas.push({ g: g, m: m });
        stripW += m.w * scale + 2;
        stripH = Math.max(stripH, m.h * scale + (DESCENDERS.has(g) ? 4 : 2));
    }
    c.width = Math.max(1, stripW);
    c.height = stripH;
    const cctx = c.getContext('2d');
    cctx.fillStyle = themeColor('--bg-color') || '#000';
    cctx.fillRect(0, 0, c.width, c.height);
    cctx.fillStyle = themeColor('--fg-color') || '#fff';
    let x = 0;
    metas.forEach(({ g, m }) => {
        const dy = DESCENDERS.has(g) ? 2 : 0;
        for (let yy = 0; yy < m.h; yy++) {
            for (let xx = 0; xx < m.w; xx++) {
                if (m.bits[yy * m.w + xx]) {
                    cctx.fillRect(x + xx * scale, dy + yy * scale, scale, scale);
                }
            }
        }
        x += m.w * scale + 2;
    });
}
async function renderStringCells() {
    const $cells = $('#strings-cells');
    $cells.empty();
    const count = Math.max(0, Math.min(stringSlots, 64));
    for (let i = 0; i < count; i++) {
        const glyph = stringGlyphs.length > i ? stringGlyphs[i] : 0;
        const $cv = $('<canvas></canvas>').addClass('fw-cell').attr('data-cell', i);
        if (i === selectedCell) {
            $cv.addClass('selected');
        }
        const c = $cv[0];
        c.width = 32;
        c.height = 32;
        const ctx = c.getContext('2d');
        ctx.fillStyle = themeColor('--bg-color') || '#000';
        ctx.fillRect(0, 0, 32, 32);
        if (glyph !== 0) {
            const m = await getGlyph(glyph);
            ctx.fillStyle = themeColor('--fg-color') || '#fff';
            for (let yy = 0; yy < m.h; yy++) {
                for (let xx = 0; xx < m.w; xx++) {
                    if (m.bits[yy * m.w + xx]) {
                        ctx.fillRect(xx, yy, 1, 1);
                    }
                }
            }
        }
        $cv.click(() => { selectedCell = i; renderStringCells(); });
        $cells.append($cv);
    }
    $('#strings-truncate').prop('disabled', selectedCell < 0);
}

function renderStringPalette() {
    const $pal = $('#strings-palette');
    $pal.empty();
    for (let i = 1; i <= glyphMaxIndex; i++) {
        const $cv = $('<canvas></canvas>')
            .addClass('fw-thumb')
            .attr('data-glyph', i);
        $cv.click(() => palettePick(i));
        $pal.append($cv);
    }
    for (let i = 1; i <= glyphMaxIndex; i++) {
        drawPaletteThumb(i);
    }
}

async function drawPaletteThumb(i) {
    const c = $('#strings-palette canvas[data-glyph="' + i + '"]')[0];
    if (!c) {
        return;
    }
    const m = await getGlyph(i);
    c.width = m.w;
    c.height = m.h;
    const ctx = c.getContext('2d');
    ctx.fillStyle = themeColor('--bg-color') || '#000';
    ctx.fillRect(0, 0, c.width, c.height);
    ctx.fillStyle = themeColor('--fg-color') || '#fff';
    for (let yy = 0; yy < m.h; yy++) {
        for (let xx = 0; xx < m.w; xx++) {
            if (m.bits[yy * m.w + xx]) {
                ctx.fillRect(xx, yy, 1, 1);
            }
        }
    }
}

async function palettePick(glyph) {
    if (selectedCell < 0) {
        alert('Select a cell first.');
        return;
    }
    if (!currentHandle || !stringSel) {
        return;
    }
    stringGlyphs = fullGlyphArray(stringGlyphs);
    stringGlyphs[selectedCell] = glyph;
    await writeStringCmd(currentHandle, stringSel.index, stringGlyphs);
    markMutated();
    await renderStringsEditor();
    renderStringsListSelection();
}

$('#strings-truncate').click(async () => {
    if (selectedCell < 0 || !currentHandle || !stringSel) {
        return;
    }
    stringGlyphs = fullGlyphArray(stringGlyphs);
    for (let i = selectedCell; i < stringGlyphs.length; i++) {
        stringGlyphs[i] = 0;
    }
    await writeStringCmd(currentHandle, stringSel.index, stringGlyphs);
    markMutated();
    await renderStringsEditor();
    renderStringsListSelection();
});
// ---------------------------------------------------------------------------
// Resource Packs tab
// ---------------------------------------------------------------------------
let resourcePacks = [];      // list_resource_packs result
let rpSel = null;            // selected compatible pack

function setRpVisible(hasHandle) {
    $('#rp-min').css('display', hasHandle ? 'none' : '');
    $('#rp-main').css('display', hasHandle ? '' : 'none');
}

async function refreshResourcePacksTab() {
    if (!currentHandle) {
        setRpVisible(false);
        return;
    }
    setRpVisible(true);
    try {
        const tables = await listImageTables(currentHandle);
        fwDefinition = tables.definition || '';
        resourcePacks = await listResourcePacks();
        rpSel = null;
        renderRpTable();
        renderRpImages();
        $('#rp-apply').prop('disabled', true);
    } catch (err) {
        console.error('resource packs tab load failed', err);
        setStatus('Resource packs load failed: ' + err.toString());
    }
}

// A pack matches the open firmware when one of its comma-separated definition
// names equals the firmware's image-table definition name.
function packCompatible(pack) {
    if (!fwDefinition) {
        return true;
    }
    const names = (pack.definition || '').split(',').map(s => s.trim());
    return names.indexOf(fwDefinition) !== -1;
}

function renderRpTable() {
    const $tbody = $('#rp-table tbody');
    $tbody.empty();
    resourcePacks.forEach(pack => {
        const ok = packCompatible(pack);
        const $tr = $('<tr></tr>');
        $tr.append($('<td></td>').text(pack.name));
        $tr.append($('<td></td>').text(pack.version));
        $tr.append($('<td></td>').text(pack.author));
        $tr.append($('<td></td>').text(ok ? String(pack.image_count) : 'incompatible'));
        if (!ok) {
            $tr.addClass('fw-pack-bad');
        } else {
            $tr.addClass('fw-entry');
            $tr.click(() => selectResourcePack(pack));
        }
        $tbody.append($tr);
    });
}

function selectResourcePack(pack) {
    rpSel = pack;
    renderRpTable();
    renderRpImages();
    $('#rp-apply').prop('disabled', false);
}

function renderRpImages() {
    const $imgs = $('#rp-images');
    $imgs.empty();
    if (!rpSel) {
        $imgs.append($('<div class="fw-info"></div>').text('Select a compatible pack to preview its images.'));
        return;
    }
    $imgs.append($('<div class="fw-title"></div>').text(rpSel.name + ' — images'));
    const $grid = $('<div class="fw-pack-list"></div>');
    (rpSel.images || []).forEach(img => {
        const $box = $('<div></div>');
        const $cv = $('<canvas></canvas>').addClass('fw-pack-img');
        const c = $cv[0];
        const bits = pixelsFromBase64(img.pixels_base64, img.width, img.height);
        drawBits(c, bits, img.width, img.height, 1);
        $box.append($cv);
        $box.append($('<div class="fw-info"></div>').text('0x' + img.index.toString(16) + ' ' + img.width + '×' + img.height));
        $grid.append($box);
    });
    $imgs.append($grid);
}

$('#rp-apply').click(async () => {
    if (!currentHandle || !rpSel) {
        return;
    }
    const pack = rpSel;
    if (!confirm('Apply resource pack "' + pack.name + '" v' + pack.version + ' by ' + pack.author + '?')) {
        return;
    }
    try {
        const res = await applyResourcePackCmd(currentHandle, pack.path);
        setStatus('Resource pack applied — ' + res.applied + ' applied, ' + res.skipped + ' skipped');
        markMutated();
        glyphCache.clear(); // slot dims/index set may have changed
        await refreshImagesTab(); // slot dims may have changed
        await refreshResourcePacksTab();
    } catch (err) {
        alert('Apply failed: ' + err.toString());
    }
});
