import 'photonkit/dist/css/photon.css';
import './style.css';
import $ from 'jquery';
import { getLocale, readTextFile, resolveResourcePath, readMonitoringData } from './lib/tauri-bridge.js';

let lang = {};

async function uiTranslate() {
    try {
        // Locale files are two-letter (de.json, en.json); getLocale() returns
        // full BCP-47 tags like "de-DE" that would miss every file.
        const locale = (await getLocale()).substr(0, 2);
        const fp = await resolveResourcePath('i18n/' + locale + '.json');
        const text = await readTextFile(fp);
        lang = JSON.parse(text);
    } catch (err) {
        return;
    }
    $('[data-lang]').each(function () {
        const phrase = lang[$(this).data('lang')];
        if (phrase) {
            $(this).html(phrase);
        }
    });
}

// Sensor registry. `value(sample)` returns the SI-unit value to plot; battery
// cells return null when the cell is absent (raw 0) so the row hides, like
// NToolbox. Colors follow NToolbox's Device Monitor series colors where the
// brief documents them; battery cells use distinct hues of their own.
const SENSORS = [
    { id: 'battery1', color: '#1f77b4', unit: 'V', langKey: 'Monitor.Battery1', value: s => s.battery[0] || null },
    { id: 'battery2', color: '#2ca02c', unit: 'V', langKey: 'Monitor.Battery2', value: s => s.battery[1] || null },
    { id: 'battery3', color: '#17becf', unit: 'V', langKey: 'Monitor.Battery3', value: s => s.battery[2] || null },
    { id: 'battery4', color: '#8c564b', unit: 'V', langKey: 'Monitor.Battery4', value: s => s.battery[3] || null },
    { id: 'batteryPack', color: '#000000', unit: 'V', langKey: 'Monitor.BatteryPack', value: s => s.battery_pack > 0 ? s.battery_pack : null },
    { id: 'power', color: '#00ff00', unit: 'W', langKey: 'Monitor.Power', value: s => s.power }, // lime
    { id: 'powerSet', color: '#008000', unit: 'W', langKey: 'Monitor.PowerSet', value: s => s.power_set }, // green
    { id: 'temperature', color: '#ff0000', unit: '°C', langKey: 'Monitor.Temperature', value: s => tempToC(s.temperature, s) }, // red
    { id: 'temperatureSet', color: '#8b0000', unit: '°C', langKey: 'Monitor.TemperatureSet', value: s => tempToC(s.temperature_set, s) }, // dark red
    { id: 'outputCurrent', color: '#ffa500', unit: 'A', langKey: 'Monitor.OutputCurrent', value: s => s.output_current }, // orange
    { id: 'outputVoltage', color: '#87cefa', unit: 'V', langKey: 'Monitor.OutputVoltage', value: s => s.output_voltage }, // light sky blue
    { id: 'resistance', color: '#ee82ee', unit: 'Ω', langKey: 'Monitor.Resistance', value: s => s.resistance }, // violet
    { id: 'realResistance', color: '#8a2be2', unit: 'Ω', langKey: 'Monitor.RealResistance', value: s => s.real_resistance }, // blue violet
    { id: 'boardTemperature', color: '#8b4513', unit: '°C', langKey: 'Monitor.BoardTemperature', value: s => s.board_temperature }, // saddle brown
];

// The firmware reports temperature in 0.1-degree steps, Celsius or Fahrenheit
// depending on IsCelcius; normalize everything to Celsius for the chart.
function tempToC(raw, sample) {
    const t = raw / 10;
    return sample.is_celsius ? t : (t - 32) / 1.8;
}

const POLL_MS = 100;
const WINDOW_MS = 30000;
const MAX_FAILS = 3;

const samples = []; // { t: ms epoch, values: {sensorId: number} }
const visible = {};
let paused = false;
let timer = null;
let fails = 0;
let lastLang = {};

function phrase(key, fallback) {
    return lang[key] || lastLang[key] || fallback;
}

function buildLegend() {
    const $legend = $('#monitor-legend').empty();
    SENSORS.forEach(s => {
        visible[s.id] = true;
        const $row = $('<label class="monitor-sensor"></label>');
        $row.append($('<input type="checkbox" checked>').attr('data-sensor', s.id));
        $row.append($('<span class="swatch"></span>').css('background', s.color));
        $row.append($('<span class="name"></span>').attr('data-lang', s.langKey).text(phrase(s.langKey, s.id)));
        $row.append($('<span class="value">—</span>').attr('id', 'val-' + s.id));
        $legend.append($row);
    });
    $('#monitor-legend').on('change', 'input[type=checkbox]', function () {
        visible[$(this).data('sensor')] = $(this).is(':checked');
        draw();
    });
}

function updateLegend(sample) {
    SENSORS.forEach(s => {
        const v = s.value(sample);
        const $row = $('#val-' + s.id).closest('.monitor-sensor');
        if (v === null) {
            $row.hide();
        } else {
            $row.show();
            $('#val-' + s.id).text(v.toFixed(2) + ' ' + s.unit);
        }
    });
}

function updateStatus(sample) {
    const $status = $('#monitor-status').removeClass('disconnected');
    let text = phrase('Monitor.Connected', 'Connected');
    if (sample.is_firing) {
        text += ' — ' + phrase('Monitor.Firing', 'FIRING');
    } else if (sample.is_charging) {
        text += ' — ' + phrase('Monitor.Charging', 'Charging');
    }
    $status.text(text + (sample.is_celsius ? ' (°C)' : ' (°F)'));
}

function draw() {
    const canvas = document.getElementById('monitor-chart');
    // The canvas fills its container via CSS; keep the backing store in sync
    // with the laid-out size so the graph tracks window resizes.
    const cw = canvas.clientWidth || 640;
    const ch = canvas.clientHeight || 400;
    if (canvas.width !== cw) canvas.width = cw;
    if (canvas.height !== ch) canvas.height = ch;
    const ctx = canvas.getContext('2d');
    const w = canvas.width;
    const h = canvas.height;
    const padL = 45, padR = 8, padT = 8, padB = 18;
    const plotW = w - padL - padR;
    const plotH = h - padT - padB;

    ctx.fillStyle = '#1b1b1b';
    ctx.fillRect(0, 0, w, h);

    const now = samples.length ? samples[samples.length - 1].t : Date.now();
    const x0 = now - WINDOW_MS;

    // Visible max over the window -> auto-scaled Y axis floored at 0. A fixed
    // 0..100 axis (NToolbox's default) flattens every mixed-unit trace here;
    // auto-scaling keeps volts, watts and ohms readable on one shared axis.
    let max = 0;
    samples.forEach(sm => {
        if (sm.t < x0) return;
        SENSORS.forEach(s => {
            if (!visible[s.id]) return;
            const v = sm.values[s.id];
            if (v !== null && v !== undefined && isFinite(v) && v > max) max = v;
        });
    });
    if (max <= 0) max = 1;
    max *= 1.05;

    // Grid + Y labels.
    ctx.strokeStyle = '#3a3a3a';
    ctx.fillStyle = '#999';
    ctx.font = '10px sans-serif';
    ctx.textAlign = 'right';
    ctx.lineWidth = 1;
    for (let i = 0; i <= 4; i++) {
        const y = padT + plotH - (plotH * i / 4);
        ctx.beginPath();
        ctx.moveTo(padL, y);
        ctx.lineTo(w - padR, y);
        ctx.stroke();
        ctx.fillText((max * i / 4).toFixed(1), padL - 4, y + 3);
    }

    // X labels: left edge = -30 s, middle = -15 s, right = now.
    ctx.textAlign = 'center';
    ['-30s', '-15s', '0s'].forEach((label, i) => {
        ctx.fillText(label, padL + plotW * i / 2, h - 5);
    });

    // One polyline per visible sensor.
    SENSORS.forEach(s => {
        if (!visible[s.id]) return;
        ctx.strokeStyle = s.color;
        ctx.lineWidth = 1.4;
        ctx.beginPath();
        let started = false;
        for (const sm of samples) {
            if (sm.t < x0) continue;
            const v = sm.values[s.id];
            if (v === null || v === undefined || !isFinite(v)) {
                started = false;
                continue;
            }
            const x = padL + ((sm.t - x0) / WINDOW_MS) * plotW;
            const y = padT + plotH - (v / max) * plotH;
            if (started) {
                ctx.lineTo(x, y);
            } else {
                ctx.moveTo(x, y);
                started = true;
            }
        }
        ctx.stroke();
    });
}

function showDisconnected() {
    stop();
    $('#monitor-status').addClass('disconnected').text(phrase('Monitor.Disconnected', 'Device disconnected.'));
    $('#monitor-overlay').show();
    $('#monitor-overlay-text').text(phrase('Monitor.Disconnected', 'Device disconnected.'));
}

function stop() {
    if (timer) {
        clearTimeout(timer);
        timer = null;
    }
}

function tick() {
    if (paused) {
        timer = setTimeout(tick, POLL_MS);
        return;
    }
    readMonitoringData()
        .then(data => {
            fails = 0;
            const sm = { t: Date.now(), values: {} };
            SENSORS.forEach(s => {
                sm.values[s.id] = s.value(data);
            });
            samples.push(sm);
            const cutoff = sm.t - WINDOW_MS - 1000;
            while (samples.length && samples[0].t < cutoff) {
                samples.shift();
            }
            updateStatus(data);
            updateLegend(data);
            draw();
            timer = setTimeout(tick, POLL_MS);
        })
        .catch(() => {
            fails += 1;
            if (fails >= MAX_FAILS) {
                showDisconnected();
            } else {
                timer = setTimeout(tick, POLL_MS);
            }
        });
}

function start() {
    stop();
    fails = 0;
    $('#monitor-overlay').hide();
    $('#monitor-status').removeClass('disconnected').text(phrase('Monitor.Connecting', 'Connecting…'));
    timer = setTimeout(tick, 0);
}

$('#monitor-pause').click(function () {
    paused = !paused;
    $(this).text(phrase(paused ? 'Monitor.ResumeButton' : 'Monitor.PauseButton', paused ? 'Resume' : 'Pause'));
});

$('#monitor-retry').click(start);

(async () => {
    buildLegend();
    lastLang = {
        'Monitor.Connected': 'Connected', 'Monitor.Firing': 'FIRING', 'Monitor.Charging': 'Charging',
        'Monitor.Disconnected': 'Device disconnected.', 'Monitor.RetryButton': 'Retry',
        'Monitor.PauseButton': 'Pause', 'Monitor.ResumeButton': 'Resume', 'Monitor.Connecting': 'Connecting…'
    };
    await uiTranslate();
    start();
})();
