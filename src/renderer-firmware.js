import 'photonkit/dist/css/photon.css';
import './style.css';
import $ from 'jquery';
import {
    ipc,
    getLocale,
    closeWindow,
    readTextFile,
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
    onFlashProgress
} from './lib/tauri-bridge.js';

let currentHandle = null;
let currentPatches = [];
let currentPath = null;

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

async function refreshPatches() {
    // The Patches tab is disabled in this distribution build (feature under
    // development in the main tree); without the tab there is nothing to do.
    if (!currentHandle || !document.getElementById('patch-list')) {
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
    currentPatches.forEach(patch => {
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
                    .click(() => togglePatch(patch.id))
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
    setStatus(`${info.name} (${info.encryption})`);
    updateButtonStates();
    await refreshPatches();
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
        $('#tab-patches, #tab-images, #tab-strings, #tab-resourcepacks, #tab-recovery').hide();
        $('#tab-' + tab).show();
    });
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

loadLocale();
initTabs();

// Firmware Editor opened with a device already connected: load its
// appropriate stock build right away.
(async () => {
    try {
        const pid = (await readDeviceProductId()).replace(/\0+$/, '').trim();
        if (/^[A-Z0-9]{4}$/.test(pid)) {
            await autoLoadStockFirmware(pid);
        }
    } catch (err) {
        // no device connected — user can open a file or click Download FW
    }
})();
