import 'photonkit/dist/css/photon.css';
import './style.css';
import $ from 'jquery';
import Highcharts from 'highcharts';
import { getLocale, readTextFile, resolveResourcePath, readMonitoringData, screenshot } from './lib/tauri-bridge.js';

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
    // Same flat-key lookup for tooltip titles (data-lang-title).
    $('[data-lang-title]').each(function () {
        const phrase = lang[$(this).data('langTitle')];
        if (phrase) {
            $(this).attr('title', phrase);
        }
    });
}

// Sensor registry. `value(sample)` returns the value to plot; battery cells
// return null when the cell is absent (raw 0) so the trace breaks, like
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
    { id: 'temperature', color: '#ff0000', unitOf: s => s.is_celsius ? '°C' : '°F', langKey: 'Monitor.Temperature', value: s => s.temperature }, // red
    { id: 'temperatureSet', color: '#8b0000', unitOf: s => s.is_celsius ? '°C' : '°F', langKey: 'Monitor.TemperatureSet', value: s => s.temperature_set }, // dark red
    { id: 'outputCurrent', color: '#ffa500', unit: 'A', langKey: 'Monitor.OutputCurrent', value: s => s.output_current }, // orange
    { id: 'outputVoltage', color: '#87cefa', unit: 'V', langKey: 'Monitor.OutputVoltage', value: s => s.output_voltage }, // light sky blue
    { id: 'resistance', color: '#ee82ee', unit: 'Ω', langKey: 'Monitor.Resistance', value: s => s.resistance }, // violet
    { id: 'realResistance', color: '#8a2be2', unit: 'Ω', langKey: 'Monitor.RealResistance', value: s => s.real_resistance }, // blue violet
    { id: 'boardTemperature', color: '#8b4513', unitOf: s => s.is_celsius ? '°C' : '°F', langKey: 'Monitor.BoardTemperature', value: s => s.board_temperature }, // saddle brown
];

// The firmware reports Temperature/TemperatureSet/BoardTemperature as WHOLE
// degrees in the device's configured unit (IsCelcius) — NToolbox's
// DeviceMonitorWindow passes them to the chart unscaled and only switches the
// "°C"/"°F" label, and only PowerSet/V/A/Ω carry a documented scale factor.
// Verified live against a device in °F mode (70 = 21 °C coil, 500 = 500 °F
// setpoint, 104 = 40 °C charging board). So display raw, with the unit label
// following the device setting via `unitOf`.
function unitOf(sensor, sample) {
    return sensor.unitOf ? sensor.unitOf(sample) : sensor.unit;
}

const POLL_MS = 100;
const WINDOW_MS = 30000;
// Points per series kept in the chart: window + one second of slack, shifted
// out as new points arrive so the chart never grows unbounded.
const MAX_POINTS = Math.ceil(WINDOW_MS / POLL_MS) + 10;
const MAX_FAILS = 3;

let chart = null;
let paused = false;
let timer = null;
let fails = 0;
let lastLang = {};

function phrase(key, fallback) {
    return lang[key] || lastLang[key] || fallback;
}

function buildLegend() {
    const $legend = $('#monitor-legend').empty();
    SENSORS.forEach((s, i) => {
        const $row = $('<label class="monitor-sensor"></label>');
        $row.append($('<input type="checkbox" checked>').attr('data-sensor', i));
        $row.append($('<span class="swatch"></span>').css('background', s.color));
        $row.append($('<span class="name"></span>').attr('data-lang', s.langKey).text(phrase(s.langKey, s.id)));
        $row.append($('<span class="value">—</span>').attr('id', 'val-' + s.id));
        $legend.append($row);
    });
    $('#monitor-legend').on('change', 'input[type=checkbox]', function () {
        chart.series[$(this).data('sensor')].setVisible($(this).is(':checked'), true);
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
            $('#val-' + s.id).text(v.toFixed(2) + ' ' + unitOf(s, sample));
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

// Chart chrome follows the system theme (see the prefers-color-scheme block
// in style.css); sensor trace colors stay identical in both themes.
function chartTheme() {
    const light = window.matchMedia('(prefers-color-scheme: light)').matches;
    return light
        ? { bg: '#ffffff', border: '#c8c8c8', label: '#555555', grid: '#e2e2e2',
            tooltipBg: 'rgba(250, 250, 250, 0.95)', tooltipFg: '#222222', tooltipBorder: '#bbbbbb' }
        : { bg: '#1b1b1b', border: '#444444', label: '#999999', grid: '#3a3a3a',
            tooltipBg: 'rgba(20, 20, 20, 0.92)', tooltipFg: '#dddddd', tooltipBorder: '#555555' };
}

function axisStyle(theme) {
    return {
        labels: { style: { color: theme.label, fontSize: '10px' } },
        gridLineColor: theme.grid,
        lineColor: theme.grid,
        tickColor: theme.grid,
        title: { style: { color: theme.label } },
    };
}

function chartOptions(theme) {
    const axis = axisStyle(theme);
    return {
        chart: {
            backgroundColor: theme.bg,
            borderColor: theme.border,
            borderWidth: 1,
            borderRadius: 3,
            animation: false,
            marginLeft: 55,
        },
        title: { text: null },
        credits: { enabled: false },
        legend: { enabled: false }, // custom left-hand column instead
        xAxis: {
            type: 'datetime',
            minRange: 1000,
            ...axis,
            dateTimeLabelFormats: { second: '%H:%M:%S' },
        },
        // Single y-axis from 0-640 to accommodate all sensor values including resistance.
        yAxis: {
            min: 0,
            max: 640,
            ...axis,
            title: { text: null },
            tickInterval: 100,
            labels: {
                style: { fontSize: '10px' },
                step: 1
            }
        },
        tooltip: {
            shared: true,
            crosshairs: true,
            backgroundColor: theme.tooltipBg,
            borderColor: theme.tooltipBorder,
            style: { color: theme.tooltipFg, fontSize: '11px' },
            headerFormat: '<b>{point.key:%H:%M:%S.%L}</b><br/>',
            pointFormatter: function () {
                const sensor = SENSORS[this.series.index];
                const unit = unitOf(sensor, this.series.userOptions._sample || {});
                return '<span style="color:' + this.color + '">\u25CF</span> ' +
                    this.series.name + ': <b>' +
                    (this.y === null ? '—' : this.y.toFixed(2)) + '</b>' +
                    (unit ? ' ' + unit : '') + '<br/>';
            },
        },
    };
}

function buildChart() {
    const theme = chartTheme();
    chart = Highcharts.chart('monitor-chart', {
        ...chartOptions(theme),
        plotOptions: {
            line: {
                animation: false,
                marker: {
                    enabled: false,
                    radius: 2.5,
                    symbol: 'circle',
                },
                states: {
                    hover: { lineWidthPlus: 1, halo: { size: 3 } },
                },
                // Shared tooltips require default (sticky) tracking —
                // stickyTracking:false breaks hover detection entirely.
                tooltip: { snap: 15 },
            },
        },
        series: SENSORS.map(s => ({
            name: phrase(s.langKey, s.id),
            color: s.color,
            lineWidth: 1.4,
            yAxis: s.yAxis || 0,
            data: [],
            _sample: null,
        })),
    });
    // Re-skin the chart when the system theme changes mid-session.
    window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => {
        chart.update(chartOptions(chartTheme()), true, false);
    });
    window.__monitorChart = chart; // debug/testing hook
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
            const t = Date.now();
            SENSORS.forEach((s, i) => {
                const series = chart.series[i];
                series.userOptions._sample = data;
                series.addPoint([t, s.value(data)], false, series.data.length >= MAX_POINTS, false);
            });
            chart.xAxis[0].setExtremes(t - WINDOW_MS, t, false);
            chart.redraw();
            updateStatus(data);
            updateLegend(data);
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

function setPaused(p) {
    paused = p;
    $('#monitor-pause').text(phrase(paused ? 'Monitor.ResumeButton' : 'Monitor.PauseButton', paused ? 'Resume' : 'Pause'));
}

$('#monitor-pause').click(() => setPaused(!paused));

// --- Screenshot (0xC1) -------------------------------------------------
// 1024 raw framebuffer bytes, 1bpp vertical packing: byte = x + (y/8)*width,
// bit y%8, LSB = top pixel. Geometry: 96x16 if bytes 192..1024 are all zero
// (its 1bpp image is 96*16/8 = 192 bytes and the tail is padding), else 64x128
// (exactly 64*128/8 = 1024 bytes). ArcticFox firmware only — stock firmware
// has no 0xC1 handler and the read times out.
const SCREENSHOT_GEOMS = [
    { width: 96, height: 16 },
    { width: 64, height: 128 },
];

function decodeScreenshot(raw) {
    let geom = SCREENSHOT_GEOMS[1];
    let tailZero = true;
    for (let i = 192; i < raw.length; i++) {
        if (raw[i] !== 0) { tailZero = false; break; }
    }
    if (tailZero) geom = SCREENSHOT_GEOMS[0];
    const { width, height } = geom;
    const canvas = document.getElementById('screenshot-canvas');
    const ZOOM = 4;
    canvas.width = width * ZOOM;
    canvas.height = height * ZOOM;
    const ctx = canvas.getContext('2d');
    ctx.fillStyle = '#000';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.fillStyle = '#9acd32'; // ArcticFox LCD green
    for (let y = 0; y < height; y++) {
        for (let x = 0; x < width; x++) {
            if (raw[x + (y >> 3) * width] & (1 << (y & 7))) {
                ctx.fillRect(x * ZOOM, y * ZOOM, ZOOM, ZOOM);
            }
        }
    }
    return geom;
}

function openScreenshotModal() {
    $('#screenshot-modal').show();
    $('#monitor-screenshot').focus();
}

function closeScreenshotModal() {
    $('#screenshot-modal').hide();
}

$('#monitor-screenshot').click(() => {
    const $btn = $('#monitor-screenshot').prop('disabled', true);
    screenshot()
        .then(b64 => {
            const bin = atob(b64);
            const raw = new Uint8Array(bin.length);
            for (let i = 0; i < bin.length; i++) raw[i] = bin.charCodeAt(i);
            decodeScreenshot(raw);
            openScreenshotModal();
        })
        .catch(err => {
            $('#monitor-status').addClass('disconnected')
                .text(phrase('Monitor.ScreenshotFailed', 'Screenshot failed') + ': ' + (err.message || err));
        })
        .finally(() => $btn.prop('disabled', false));
});

$('#screenshot-save').click(() => {
    const canvas = document.getElementById('screenshot-canvas');
    canvas.toBlob(blob => {
        const a = document.createElement('a');
        a.href = URL.createObjectURL(blob);
        a.download = 'screenshot_' + new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19) + '.png';
        a.click();
        URL.revokeObjectURL(a.href);
    });
});

$('#screenshot-close').click(closeScreenshotModal);

// Esc closes the modal; the pause shortcut ignores it (not Space).
$(document).on('keydown', (e) => {
    if (e.code === 'Escape' && $('#screenshot-modal').is(':visible')) {
        closeScreenshotModal();
    }
});

// Space pauses/resumes too. When a control has focus, leave Space to its
// native behavior (a focused pause button still toggles via its own click;
// a focused checkbox toggles itself) so nothing fires twice.
$(document).on('keydown', (e) => {
    if (e.code !== 'Space' || e.repeat) return;
    const t = e.target;
    if (t && (t.tagName === 'BUTTON' || t.tagName === 'INPUT' || t.tagName === 'SELECT' ||
              t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
    e.preventDefault();
    setPaused(!paused);
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
    buildChart();
    start();
})();
