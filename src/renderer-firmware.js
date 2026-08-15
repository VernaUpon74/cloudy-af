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
    closeFirmware
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

function initTabs() {
    $('.tab-item').click(function () {
        $('.tab-item').removeClass('active');
        $(this).addClass('active');
        const tab = $(this).data('tab');
        $('#tab-patches, #tab-images, #tab-strings, #tab-resourcepacks').hide();
        $('#tab-' + tab).show();
    });
}

$('#open-firmware').click(doOpenFirmware);
$('#save-firmware').click(doSaveFirmware);
$('#save-as-firmware').click(doSaveAsFirmware);

ipc.on('data', (event, data) => {
    // Received when the window is opened via ipc.send('firmware', data).
    // Future: pre-select firmware path from data.
    console.log('firmware window received data', data);
});

loadLocale();
initTabs();
