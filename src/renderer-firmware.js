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
    onRecoveryProgress
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
        currentHandle = info.handle;
        setStatus(`${info.name} (${info.encryption})`);
        updateButtonStates();
        await refreshPatches();
    } catch (err) {
        console.error('openFirmware failed', err);
        alert(err.toString());
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
    try {
        await flashFirmwareToDevice(currentHandle);
        setStatus($('#fw-status').text() + ' — flashed');
        $('#undo-changes').prop('disabled', false);
    } catch (err) {
        console.error('flashFirmware failed', err);
        alert(err.toString());
        setStatus('Flash failed');
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
    try {
        await undoFirmwareChanges(currentHandle);
        setStatus('Original firmware restored');
    } catch (err) {
        console.error('undoFirmwareChanges failed', err);
        alert(err.toString());
        setStatus('Restore failed');
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
let recoveryBusy = false;

function recoveryLog(msg) {
    const $log = $('#recovery-log');
    $log.append(document.createTextNode(new Date().toLocaleTimeString() + '  ' + msg + '\n'));
    $log.scrollTop($log[0].scrollHeight);
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
}

$('#recovery-choose').click(async () => {
    try {
        const path = await openFileDialog([{ name: 'Firmware', extensions: ['bin'] }]);
        if (path) {
            recoveryPath = path;
            $('#recovery-path').text(path);
            $('#recovery-start').prop('disabled', recoveryBusy);
        }
    } catch (err) {
        console.error('openFileDialog failed', err);
    }
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
    recoveryBusy = true;
    $('#recovery-start').prop('disabled', true);
    recoveryLog('recovery started');
    try {
        await recoveryFlash(recoveryPath, pid || null);
        recoveryLog('SUCCESS — device rebooted into the flashed firmware');
    } catch (err) {
        recoveryLog('FAILED: ' + err.toString());
    } finally {
        recoveryBusy = false;
        $('#recovery-start').prop('disabled', !recoveryPath);
    }
});

onRecoveryProgress(msg => recoveryLog(msg));
setInterval(refreshRecoveryDevices, 2000);
refreshRecoveryDevices();

$('#open-firmware').click(doOpenFirmware);
$('#save-firmware').click(doSaveFirmware);
$('#save-as-firmware').click(doSaveAsFirmware);
$('#flash-firmware').click(doFlashFirmware);
$('#undo-changes').click(doUndoChanges);

updateButtonStates();

ipc.on('data', (event, data) => {
    // Received when the window is opened via ipc.send('firmware', data).
    // Future: pre-select firmware path from data.
    console.log('firmware window received data', data);
});

loadLocale();
initTabs();
