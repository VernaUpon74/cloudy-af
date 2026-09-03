/**
 * DEVIATION: This sidecar is a Tauri-era replacement for the Electron main-process
 * HID code in the original hobbyquaker/arcticfox-config fork. It runs as a separate
 * Node.js process spawned by the Rust backend and communicates over stdin/stdout.
 * Notable behavioral changes from the original fork:
 *   - hidraw backend is used on Linux so the Flatpak sandbox can detect
 *     hotplugged HID devices via /dev/hidraw* (libusb relies on /dev/bus/usb
 *     which Flatpak does not expose for devices plugged in after startup).
 *   - supportedSettingsVersion is bumped from 11 to 12 for current firmware builds.
 *   - Device auto-reconnect is implemented explicitly in this bridge.
 *   - Configuration strings are sanitized before being emitted to the renderer.
 */
const fs = require('fs');
const path = require('path');
// Force node-hid to use the libusb backend on Linux, matching the behavior
// of the original Electron app (node-hid 0.5.x) and avoiding hidraw report-id
// shifts that confuse the arcticfox parser.
const nodehid = require('node-hid');
// DEVIATION: hidraw is used instead of libusb so the Flatpak sandbox can see
// hotplugged HID devices via /dev/hidraw*. The libusb backend relies on
// /dev/bus/usb which Flatpak does not expose for devices plugged in after
// the app starts.
if (nodehid.setDriverType) {
    nodehid.setDriverType('hidraw');
}
const fox = require('arcticfox');
const xml2js = require('xml2js');
const AfcFile = require('./afcfile');

const afc = new AfcFile();
let autoconnect = true;
// While the Rust backend flashes firmware it owns the HID device; suspend
// stops polling and reconnects so nothing interleaves into the flash stream.
let suspended = false;

// The device firmware reports SettingsVersion 12; the arcticfox module
// supports it natively (PuffCutOff widened to u16 in the v12 layout).

let currentRequestId = null;

// DEVIATION: Explicit auto-reconnect logic. The original arcticfox module schedules
// its own reconnect inside disconnect(); we override disconnect() below and use this
// timer so the sidecar can reconnect when a device is unplugged and re-plugged.
let reconnectTimer = null;
const RECONNECT_INTERVAL_MS = 2000;

function clearReconnectTimer() {
    if (reconnectTimer) {
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
    }
}

function scheduleReconnect() {
    clearReconnectTimer();
    if (!suspended && !fox.connected) {
        reconnectTimer = setTimeout(() => {
            reconnectTimer = null;
            if (!fox.connected) {
                try {
                    fox.connect();
                } catch (err) {
                    // Device still not present; retry later.
                }
                scheduleReconnect();
            }
        }, RECONNECT_INTERVAL_MS);
    }
}

function send(event, payload) {
    if (currentRequestId) {
        payload.request_id = currentRequestId;
    }
    const line = JSON.stringify({ event, payload });
    process.stdout.write(line + '\n');
}

function sendError(message, detail) {
    send('error', { message, error: true, detail: detail ? String(detail) : undefined });
}

// Remove control characters and Unicode replacement characters from strings so
// profile/TFR/battery names display cleanly and xml2js can build valid XML.
function sanitizeConfigStrings(obj) {
    if (typeof obj === 'string') {
        return obj.replace(/[\x00-\x08\x0b-\x0c\x0e-\x1f\ufffd]/g, '').trim();
    }
    if (Array.isArray(obj)) {
        return obj.map(sanitizeConfigStrings);
    }
    if (obj && typeof obj === 'object') {
        const result = {};
        for (const key of Object.keys(obj)) {
            result[key] = sanitizeConfigStrings(obj[key]);
        }
        return result;
    }
    return obj;
}

function emit(channel, data) {
    send('ipc-event', { channel, data });
}

function onConnect() {
    clearReconnectTimer();
    emit('connect', true);
    if (autoconnect) {
        fox.setDateTime(new Date());
        downloadConfig();
    }
}

function onClose() {
    emit('connect', false);
    scheduleReconnect();
}

function onError(err) {
    sendError('HID error', err);
}

// Disable the arcticfox module's internal reconnect loop so this bridge controls
// reconnection timing and avoids duplicate concurrent connection attempts.
fox.disconnect = function() {
    if (fox.hid && fox.hid.close) {
        try { fox.hid.close(); } catch (e) {}
    }
    if (fox.connected) {
        fox.connected = false;
        fox.emit('close');
    }
};

fox.on('connect', () => { onConnect(); });
fox.on('close', () => { onClose(); });
fox.on('error', err => { onError(err); });

function downloadConfig() {
    fox.readConfiguration((err, data) => {
        if (err) {
            console.error(err.toString());
            if (err.toString() === 'Error: Outdated Toolbox') {
                sendError('Incompatible Firmware', 'Outdated Toolbox');
            } else if (err.toString() === 'Error: Outdated Firmware') {
                sendError('Incompatible Firmware', 'Connect device with firmware build >= ' + fox.minimumSupportedBuildNumber);
            } else {
                sendError('Read configuration failed', err);
            }
            return;
        }
        const cleanData = sanitizeConfigStrings(data);
        emit('config', cleanData);
    });
}

function handleCommand(cmd) {
    switch (cmd.type) {
        case 'connect':
            try {
                autoconnect = cmd.autoconnect !== false;
                if (cmd.data && typeof cmd.data.autoconnect === 'boolean') {
                    autoconnect = cmd.data.autoconnect;
                }
                fox.connect();
                // DEVIATION: If autoconnect is enabled and no device was found at
                // startup, start the reconnect loop so plugging in the device later
                // is detected without restarting the app.
                if (autoconnect && !fox.connected) {
                    scheduleReconnect();
                }
                // connect() is synchronous; events will be emitted on stdout.
                send('connect_ack', { connected: fox.connected });
            } catch (err) {
                sendError('Connect failed', err);
            }
            break;

        case 'disconnect':
            try {
                fox.close();
                send('disconnect_ack', {});
            } catch (err) {
                sendError('Disconnect failed', err);
            }
            break;

        case 'suspend':
            // The Rust firmware flasher is about to own the HID device:
            // drop our handle and stop the reconnect loop until 'resume'.
            suspended = true;
            clearReconnectTimer();
            try {
                fox.close();
            } catch (err) {
                // already closed — fine
            }
            send('suspend_ack', {});
            break;

        case 'resume':
            suspended = false;
            if (autoconnect && !fox.connected) {
                try {
                    fox.connect();
                } catch (err) {
                    // Device not present; the reconnect loop will retry.
                }
                if (!fox.connected) {
                    scheduleReconnect();
                }
            }
            send('resume_ack', {});
            break;

        case 'download':
            if (fox.connected) {
                fox.setDateTime(new Date());
                downloadConfig();
            } else {
                autoconnect = true;
                fox.connect();
                if (!fox.connected) {
                    scheduleReconnect();
                }
            }
            break;

        case 'upload':
            if (fox.connected) {
                try {
                    const ok = fox.writeConfiguration(cmd.data);
                    if (ok) {
                        send('upload_ack', {});
                    } else {
                        sendError('Upload failed', 'writeConfiguration returned false');
                    }
                } catch (err) {
                    sendError('Upload failed', err);
                }
            } else {
                sendError('No compatible USB device', 'connect first');
            }
            break;

        case 'set_datetime':
            if (fox.connected) {
                fox.setDateTime(new Date());
            }
            break;

        case 'decode_afc':
            try {
                const xml = afc.decodeAfc(Buffer.from(cmd.data, 'base64'));
                afc.xml2conf(xml, (err, res) => {
                    if (err) {
                        sendError('Decode AFC failed', err);
                    } else {
                        send('decode_afc_result', { config: res });
                    }
                });
            } catch (err) {
                sendError('Decode AFC failed', err);
            }
            break;

        case 'encode_afc':
            try {
                const cleanConfig = sanitizeConfigStrings(cmd.config);
                const xml = afc.conf2xml(cleanConfig);
                const buf = afc.encodeAfc(xml);
                send('encode_afc_result', { data: buf.toString('base64') });
            } catch (err) {
                sendError('Encode AFC failed', err);
            }
            break;

        case 'tfr_import_csv':
            try {
                const lines = cmd.csv.replace(/\r/g, '').split('\n');
                const table = [];
                lines.forEach(line => {
                    let [temp, factor] = line.split(',');
                    temp = parseInt(temp, 10);
                    factor = parseFloat(factor);
                    if (factor) {
                        table.push({ Temperature: temp, Factor: factor });
                    }
                });
                send('tfr_import_result', { table });
            } catch (err) {
                sendError('TFR import failed', err);
            }
            break;

        case 'tfr_export_csv':
            try {
                const name = cmd.table.Name.replace(/\u0000/g, '');
                let out = '"Temperature (degF)","Electrical Resistivity"';
                cmd.table.Points.forEach(p => {
                    out += ('\n' + p.Temperature + ',' + p.Factor);
                });
                send('tfr_export_result', { csv: out, name });
            } catch (err) {
                sendError('TFR export failed', err);
            }
            break;

        case 'bat_export_xml':
            try {
                const name = cmd.table.Name.replace(/\u0000/g, '');
                const obj = {
                    BatteryProfile: {
                        Cutoff: cmd.table.Cutoff,
                        Data: {
                            Point: []
                        }
                    }
                };
                cmd.table.PercentsVoltage.forEach(p => {
                    obj.BatteryProfile.Data.Point.push({
                        $: {
                            Percent: p.Percents,
                            Voltage: p.Voltage
                        }
                    });
                });
                const builder = new xml2js.Builder({ headless: true });
                const xml = builder.buildObject(obj);
                send('bat_export_result', { xml, name });
            } catch (err) {
                sendError('Battery export failed', err);
            }
            break;

        case 'bat_import_xml':
            try {
                xml2js.parseString(cmd.xml, (err, result) => {
                    if (err) {
                        sendError('Battery import failed', err);
                        return;
                    }
                    const data = {
                        table: {
                            Cutoff: result.BatteryProfile.Cutoff,
                            PercentsVoltage: []
                        }
                    };
                    result.BatteryProfile.Data[0].Point.forEach(p => {
                        data.table.PercentsVoltage.push({
                            Percents: parseInt(p.$.Percent, 10),
                            Voltage: parseFloat(p.$.Voltage)
                        });
                    });
                    send('bat_import_result', data);
                });
            } catch (err) {
                sendError('Battery import failed', err);
            }
            break;

        case 'get_firmware_minimum':
            send('firmware_minimum', fox.minimumSupportedBuildNumber);
            break;

        default:
            sendError('Unknown command', cmd.type);
    }
}

let buffer = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', chunk => {
    buffer += chunk;
    let idx;
    while ((idx = buffer.indexOf('\n')) !== -1) {
        const line = buffer.slice(0, idx).trim();
        buffer = buffer.slice(idx + 1);
        if (!line) continue;
        try {
            const cmd = JSON.parse(line);
            currentRequestId = cmd.request_id || null;
            handleCommand(cmd);
            currentRequestId = null;
        } catch (err) {
            currentRequestId = null;
            sendError('Invalid JSON command', err);
        }
    }
});

process.stdin.on('end', () => {
    fox.close();
    process.exit(0);
});

// Emit firmware minimum once ready.
send('ready', { minimumSupportedBuildNumber: fox.minimumSupportedBuildNumber });


