import 'photonkit/dist/css/photon.css';
import './style.css';
import $ from 'jquery';
import Highcharts from 'highcharts';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { PhysicalSize } from '@tauri-apps/api/dpi';
import { ipc, getLocale, getAppVersion, readTextFile, resolveResourcePath, showError, openConfig, saveConfig } from './lib/tauri-bridge.js';
import { DEFAULT_TFR_TABLES, DEFAULT_POWER_CURVES } from './lib/default-curves.js';

let config;
let lang;
let appVersion = '1.2.0';

ipc.on('connect', (event, status) => {
    $('#connection-status').html(_('Status.Device') + ' ' + (status ? _('Status.Connected') : _('Status.Disconnected')));
});

ipc.on('config', (event, data) => {
    config = data;
    uiUpdate();
});

ipc.on('piregchange', (event, data) => {
    config.profiles[activeProfile].PIRegulatorIsEnabled = data.PIRegulatorIsEnabled;
    config.profiles[activeProfile].PIRegulatorRange = data.PIRegulatorRange;
    config.profiles[activeProfile].PIRegulatorPValue = data.PIRegulatorPValue;
    config.profiles[activeProfile].PIRegulatorIValue = data.PIRegulatorIValue;
});

ipc.on('tfrchange', (event, data) => {
    config.TFRTables[data.index] = data.table;
    uiUpdate();
});

ipc.on('pcchange', (event, data) => {
    config.PowerCurves[data.index] = data.table;
    uiUpdate();
});

ipc.on('batchange', (event, data) => {
    config.CustomBatteryProfiles[data.index] = data.table;
});

let foxfirmware = '170909';

// DEVIATION: The original fork did not expose the per-profile Celsius/Fahrenheit
// selector (IsCelcius, bit 0x20 of Flags). We normalize it here so the UI checkbox
// works for configs loaded from the device or from older .afc files.
function normalizeConfig(cfg) {
    if (!cfg || !Array.isArray(cfg.profiles)) return cfg;
    // Local-only fields (not sent to the device) default to 0 when missing
    // from older .afc files.
    if (typeof cfg.ClockAnimation !== 'number') cfg.ClockAnimation = 0;
    // Fall back to the built-in default curves when a TFR table or power
    // curve is entirely zeroed (e.g. fresh/never-customized device tables),
    // so the plots show meaningful data instead of a flat zero line.
    if (Array.isArray(cfg.TFRTables)) {
        cfg.TFRTables.forEach((tfr, i) => {
            const zeroed = !tfr.Points || tfr.Points.every(p => !p.Temperature && !p.Factor);
            if (zeroed && DEFAULT_TFR_TABLES[i]) {
                cfg.TFRTables[i] = JSON.parse(JSON.stringify(DEFAULT_TFR_TABLES[i]));
            }
        });
    }
    if (Array.isArray(cfg.PowerCurves)) {
        cfg.PowerCurves.forEach((pc, i) => {
            const zeroed = !pc.Points || pc.Points.every(p => !p.Time && !p.Percent);
            if (zeroed && DEFAULT_POWER_CURVES[i]) {
                cfg.PowerCurves[i] = JSON.parse(JSON.stringify(DEFAULT_POWER_CURVES[i]));
            }
        });
    }
    cfg.profiles.forEach(profile => {
        if (typeof profile.IsCelcius !== 'boolean') {
            if (typeof profile.Flags === 'number') {
                profile.IsCelcius = Boolean(profile.Flags & 0x20);
            } else {
                profile.IsCelcius = false;
            }
        }
    });
    return cfg;
}

ipc.on('foxfirmware', (event, data) => {
    foxfirmware = data;
    $('[data-lang="Message.ConnectDevice"]').html(lang['Message.ConnectDevice'].replace('{0}', foxfirmware).replace(/\n/g, '<br>'));
});

let activeProfile;

function uiInitTabs() {
    $('.tab-group#main .tab-item').click(function () {
        $('.tab-group#main .tab-item').removeClass('active');
        $(this).addClass('active');
        const view = $(this).data('view');
        $('.view-container.view-main .view').hide();
        $('.view-container.view-main #view-' + view).show();
    });

    $('.tab-group#screen .tab-item').click(function () {
        $('.tab-group#screen .tab-item').removeClass('active');
        $(this).addClass('active');
        const view = $(this).data('view');
        $('.view-container.view-screen .subview').hide();
        $('.view-container.view-screen #view-' + view).show();
    });

    $('.tab-group#screen-layout .tab-item').click(function () {
        $('.tab-group#screen-layout .tab-item').removeClass('active');
        $(this).addClass('active');
        const view = $(this).data('view');
        $('.view-container.view-screen-layout .subsubview').hide();
        $('.view-container.view-screen-layout #view-' + view).show();
    });

    $('.tab-group#advanced .tab-item').click(function () {
        $('.tab-group#advanced .tab-item').removeClass('active');
        $(this).addClass('active');
        const view = $(this).data('view');
        $('.view-container.view-advanced .subview').hide();
        $('.view-container.view-advanced #view-' + view).show();
        // Reflow curve charts when their container becomes visible so they
        // render at the correct size (they may have been initialised while hidden).
        window.setTimeout(() => {
            const prefix = view === 'advanced-powercurves' ? 'pc' : (view === 'advanced-materials' ? 'tfr' : null);
            if (prefix && window.curveCharts) {
                Object.keys(window.curveCharts).forEach(key => {
                    if (key.startsWith(prefix)) {
                        window.curveCharts[key].reflow();
                    }
                });
            }
        }, 0);
    });

    $('.tab-group#controls .tab-item').click(function () {
        $('.tab-group#controls .tab-item').removeClass('active');
        $(this).addClass('active');
        const view = $(this).data('view');
        $('.view-container.view-controls .subview').hide();
        $('.view-container.view-controls #view-' + view).show();
    });

    $('#profiles .tab-item').click(function () {
        const p = $(this).attr('id').replace('profile-', '');
        uiProfile(p);
    });
}

function uiScreenLayoutView(skin) {
    // 96x16 displays use Classic (0) and Lite (1); Lite uses the Small layout tab.
    // Large (64x128) displays expose all five skins: Classic(0), Circle(1), Foxy(2),
    // Small(3), Medium(4). NToolbox-style: only the active mode's fields are shown.
    const isSmallDisplay = config && config.DisplaySize === 1;
    const names = isSmallDisplay
        ? ['classic', 'small']
        : ['classic', 'circle', 'foxy', 'small', 'medium'];
    const skinVal = Number(skin) || 0;
    const name = names[skinVal] || 'classic';

    // Show only the active mode's subview. The manual mode tab bar is gone; this is
    // the sole driver, kept in sync with Appearance → Main Screen Skin.
    $('.view-container.view-screen-layout .subsubview').hide();
    $('.view-container.view-screen-layout #view-screen-layout-' + name).show();

    // Reflect the active mode in a small caption so the user knows which mode's
    // fields are being edited.
    const skinLabels = isSmallDisplay
        ? ['Skin.Classic', 'Skin.Lite']
        : ['Skin.Classic', 'Skin.Circle', 'Skin.Foxy', 'Skin.Small', 'Skin.Medium'];
    const label = skinLabels[skinVal] || 'Skin.Classic';
    $('#layout-mode-name').text(_(label));
    $('#layout-mode-bar').show();
}

// DEVIATION: The original fork had a fixed Classic/Circle/Foxy skin dropdown.
// ArcticFox exposes five main-screen skins (Classic, Circle, Foxy, Small, Medium);
// 96x16 displays instead use value 1 for the "Lite" skin. The Layout page reflects
// the active skin (NToolbox-style) and no longer offers a manual mode tab bar.
function uiUpdateSkinOptions() {
    const $skin = $('#MainScreenSkin');
    const isSmallDisplay = config && config.DisplaySize === 1;
    $skin.empty();
    if (isSmallDisplay) {
        $skin.append('<option value="0" data-lang="Skin.Classic">' + _('Skin.Classic') + '</option>');
        $skin.append('<option value="1" data-lang="Skin.Lite">' + _('Skin.Lite') + '</option>');
    } else {
        $skin.append('<option value="0" data-lang="Skin.Classic">' + _('Skin.Classic') + '</option>');
        $skin.append('<option value="1" data-lang="Skin.Circle">' + _('Skin.Circle') + '</option>');
        $skin.append('<option value="2" data-lang="Skin.Foxy">' + _('Skin.Foxy') + '</option>');
        $skin.append('<option value="3" data-lang="Skin.Small">' + _('Skin.Small') + '</option>');
        $skin.append('<option value="4" data-lang="Skin.Medium">' + _('Skin.Medium') + '</option>');
    }

    // Hide layout subviews that don't apply to this display size. (The classic
    // subview is the fallback and is always shown; Small applies to both, but is
    // labelled Lite on 96x16 displays.)
    const smallSub = $('.view-container.view-screen-layout #view-screen-layout-small');
    $('.view-container.view-screen-layout #view-screen-layout-circle').toggle(!isSmallDisplay);
    $('.view-container.view-screen-layout #view-screen-layout-foxy').toggle(!isSmallDisplay);
    $('.view-container.view-screen-layout #view-screen-layout-medium').toggle(!isSmallDisplay);
    if (isSmallDisplay) {
        smallSub.show();
    }
}

function uiPreheat(val) {
    switch (Number(val)) {
        case 0:
            $('.fox-curveonly').hide();
            $('.fox-notcurve').show();
            $('#preheatUnit').html('W');
            break;
        case 1:
            $('.fox-curveonly').hide();
            $('.fox-notcurve').show();
            $('#preheatUnit').html('%');
            break;
        case 2:
            $('.fox-curveonly').show();
            $('.fox-notcurve').hide();
            break;
    }
}

function uiTempControl(val) {
    if (val) {
        $('.fox-tconly').show();
    } else {
        $('.fox-tconly').hide();
    }
}

function uiTcr(material) {
    if (material === '4') {
        $('#TCR').show();
    } else {
        $('#TCR').hide();
    }
}

function uiInitChangeHandlers() {
    $('#mode').change(function () {
        uiTempControl($(this).val() === 'tc');
    });

    $('#PreheatType').change(function () {
        uiPreheat($(this).val());
    });

    $('#Material').change(function () {
        uiTcr($('#Material').val());
    });

    $('#MainScreenSkin').change(function () {
        uiScreenLayoutView($(this).val());
    });
}

function uiProfile(p) {
    activeProfile = p;
    $('#profiles .tab-item').removeClass('active');
    const $tab = $('#profiles #profile-' + p);
    $tab.addClass('active');

    $('.fox-pval').each(function () {
        const id = $(this).attr('id');
        const val = config.profiles[p][id];
        if ($(this).is('input')) {
            if ($(this).attr('type') === 'checkbox') {
                $(this).prop('checked', val);
            } else {
                $(this).val(val);
            }
        } else if ($(this).is('select')) {
            $(this).find('option[value="' + val + '"]').prop('selected', true);
        }
    });

    // DEVIATION: Show the profile temperature unit as plain text derived from the
    // global TemperatureUnits regional setting. Keep the per-profile IsCelcius
    // flag in sync so uploads reflect the same choice.
    const isCelsius = Number(config.TemperatureUnits) === 1;
    config.profiles[p].IsCelcius = isCelsius;
    $('#IsCelcius').text(isCelsius ? '°C' : '°F');
    $('#Temperature').attr('step', isCelsius ? '5' : '10');

    uiPreheat($('#PreheatType').val());
    uiTcr(config.profiles[p].Material);
    uiTempControl(config.profiles[p].Material !== 0);

    $('#mode option[value="' + (config.profiles[p].Material !== 0 ? 'tc' : 'vw') + '"]').prop('selected', true);
    if (config.profiles[p].Material === 4) {
        $('#TCR').show();
    } else {
        $('#TCR').hide();
    }
}

async function loadDefaultConfig() {
    try {
        const fp = await resolveResourcePath('default.afc.json');
        const text = await readTextFile(fp);
        return JSON.parse(text);
    } catch (err) {
        console.error('failed to load default config', err);
        showError('Error', 'Failed to load default configuration');
        return null;
    }
}

function uiInitButtons() {
    $('#tc-setup').click(function () {
        ipc.send('pireg', config.profiles[activeProfile]);
    });

    // Use event delegation for footer buttons; some WebKit/Tauri builds don't
    // fire directly-bound clicks on Photon toolbar buttons.
    $(document).on('click', '#download-settings', function () {
        ipc.send('download');
    });

    $(document).on('click', '#upload-settings', function () {
        window.uploadSettings();
    });

    $(document).on('click', '#reset-settings', async function () {
        config = await loadDefaultConfig();
        ipc.send('upload', config);
        uiUpdate();
    });

    $('#BatteryModel').change(function () {
        if ($(this).val() > 0) {
            $('#battery-edit').show();
        } else {
            $('#battery-edit').hide();
        }
    });

    $('#battery-edit').click(function () {
        const index = $('#BatteryModel').val() - 1;
        ipc.send('bat', { index, table: config.CustomBatteryProfiles[index] });
    });

    $('#firmware-editor').click(function () {
        ipc.send('firmware', {});
    });

    $('#device-monitor').click(function () {
        ipc.send('monitor', {});
    });
}

function uiUpdate() {
    if (!config) {
        return;
    }
    normalizeConfig(config);

    $('#startscreen').hide();

    $('#ProductName').val(config.ProductName);

    uiUpdateSkinOptions();

    const $Material = $('#Material');
    const $MaterialTable = $('#table-material');
    $Material.html('');
    $MaterialTable.html('');
    // DEVIATION: Keep the original ArcticFox coil material labels so the dropdown
    // matches the mod's firmware choices: Nickel 200, Titanium 1, SS 316, TCR,
    // and the eight user-editable TFR tables (TFR1..TFR8).
    $Material.append('<option value="1">Nickel 200</option>');
    $Material.append('<option value="2">Titanium 1</option>');
    $Material.append('<option value="3">SS 316</option>');
    $Material.append('<option value="4">TCR</option>');

    // DEVIATION: Render the user-editable TFR tables in the same grid layout the
    // original NToolbox/NFirmwareEditor Advanced Materials list used: each item
    // shows a curve preview above the [TFR] name, and clicking the card opens the
    // TFR plot editor. Fixed display names match the NFE defaults: Ni, Ti, 304,
    // 316, 316L, 321, NF30, NiFe.
    const tfrDisplayNames = ['Ni', 'Ti', '304', '316', '316L', '321', 'NF30', 'NiFe'];
    $MaterialTable.addClass('curve-grid');
    config.TFRTables.forEach((tfr, index) => {
        $Material.append('<option value="' + (index + 5) + '">TFR' + (index + 1) + '</option>');
        const displayName = tfrDisplayNames[index] || tfr.Name.replace(/\u0000/g, '');
        $MaterialTable.append(
            '<div class="curve-card tfr-card" data-tfr="' + index + '">' +
            '<div class="curve-preview tfr-preview" id="tfr' + index + '"></div>' +
            '<div class="curve-label">[TFR] ' + displayName + '</div>' +
            '</div>'
        );
        window.curveCharts = window.curveCharts || {};
        window.curveCharts['tfr' + index] = new Highcharts.Chart({
            chart: {
                renderTo: 'tfr' + index,
                width: 100,
                height: 48,
                margin: [0, 0, 0, 0],
                style: { overflow: 'visible' }
            },
            title: { text: '' },
            credits: { enabled: false },
            legend: { enabled: false },
            xAxis: {
                labels: { enabled: false },
                tickLength: 0,
                lineWidth: 0,
                min: 0,
                max: 800
            },
            yAxis: {
                title: { text: null },
                maxPadding: 0,
                minPadding: 0,
                gridLineWidth: 0,
                endOnTick: false,
                labels: { enabled: false },
                min: 1,
                max: 4
            },
            tooltip: { enabled: false },
            plotOptions: {
                series: {
                    enableMouseTracking: false,
                    lineWidth: 1,
                    shadow: false,
                    marker: { enabled: false }
                }
            },
            series: [{
                type: 'spline',
                color: '#9acd32',
                data: tfr.Points.map(p => [p.Temperature, p.Factor])
            }]
        });
    });

    $(document).on('click', '.tfr-card', function () {
        const index = $(this).data('tfr');
        ipc.send('tfr', { index, table: config.TFRTables[index] });
    });

    const $PowerTable = $('#table-power');
    $PowerTable.html('');

    // DEVIATION: Render power curves in the original NToolbox grid layout with
    // fixed display names: Soft, Boost 1s, Boost 2s, Sine 1, Sine 2, Cooldown,
    // Triangle, Linear. Clicking a card opens the Power Curve plot editor.
    const powerCurveDisplayNames = ['Soft', 'Boost 1s', 'Boost 2s', 'Sine 1', 'Sine 2', 'Cooldown', 'Triangle', 'Linear'];
    $PowerTable.addClass('curve-grid');
    config.PowerCurves.forEach((pc, index) => {
        const displayName = powerCurveDisplayNames[index] || pc.Name;
        $PowerTable.append(
            '<div class="curve-card pc-card" data-pc="' + index + '">' +
            '<div class="curve-preview pc-preview" id="pc' + index + '"></div>' +
            '<div class="curve-label">' + displayName + '</div>' +
            '</div>'
        );
        const data = [];
        pc.Points.forEach(p => {
            data.push({ x: p.Time, y: p.Percent });
        });
        window.curveCharts = window.curveCharts || {};
        window.curveCharts['pc' + index] = new Highcharts.Chart({
            chart: {
                renderTo: 'pc' + index,
                width: 100,
                height: 48,
                margin: [0, 0, 0, 0],
                style: { overflow: 'visible' }
            },
            title: { text: '' },
            credits: { enabled: false },
            legend: { enabled: false },
            xAxis: {
                labels: { enabled: false },
                tickLength: 0,
                lineWidth: 0,
                min: 0,
                max: 8
            },
            yAxis: {
                title: { text: null },
                maxPadding: 0,
                minPadding: 0,
                gridLineWidth: 0,
                endOnTick: false,
                labels: { enabled: false },
                min: 0,
                max: 250
            },
            tooltip: { enabled: false },
            plotOptions: {
                series: {
                    enableMouseTracking: false,
                    lineWidth: 1,
                    shadow: false,
                    marker: { enabled: false }
                }
            },
            series: [{
                fillColor: 'rgba(154, 205, 50, 0.25)',
                lineColor: '#9acd32',
                type: 'area',
                name: displayName,
                data
            }]
        });
    });

    $(document).on('click', '.pc-card', function () {
        const index = $(this).data('pc');
        ipc.send('pc', { index, table: config.PowerCurves[index] });
    });

    const $SelectedCurve = $('#SelectedCurve');
    $SelectedCurve.html('');
    config.PowerCurves.forEach((pc, index) => {
        const displayName = powerCurveDisplayNames[index] || pc.Name;
        $SelectedCurve.append('<option value="' + index + '">' + displayName + '</option>');
    });

    $('.fox-val').each(function () {
        const id = $(this).attr('id');
        let val = config[id];

        if (id === 'HardwareVersion') {
            val = Number(val).toFixed(2);
        }

        if ($(this).is('input')) {
            if ($(this).attr('type') === 'checkbox') {
                $(this).prop('checked', val);
            } else {
                $(this).val(val);
            }
        } else if ($(this).is('select')) {
            $(this).find('option[value="' + val + '"]').prop('selected', true);
        }
    });

    uiScreenLayoutView(config.MainScreenSkin);
    uiProfile(config.SelectedProfile);
}

async function uiTranslate() {
    try {
        const locale = (await getLocale()).substr(0, 2);
        const fp = await resolveResourcePath('i18n/' + locale + '.json');
        const text = await readTextFile(fp);
        lang = JSON.parse(text);
    } catch (err) {
        return;
    }
    $('[data-lang]').each(function () {
        const key = $(this).data('lang');
        let phrase = lang[key];
        if (key === 'Message.ConnectDevice') {
            phrase = phrase.replace('{0}', foxfirmware);
        }
        if (phrase) {
            $(this).html(phrase);
        }
    });
    $('[data-lang-title]').each(function () {
        const key = $(this).data('lang-title');
        const phrase = lang[key];
        if (phrase) {
            $(this).attr('title', phrase);
        }
    });

    // Fallback: give every labeled setting row a tooltip using its label text so
    // every checkbox/combobox/label has at least a basic vape-function hint.
    $('tr').each(function () {
        const $row = $(this);
        if ($row.attr('title')) return;
        const $label = $row.find('td.c1').first();
        const labelText = $label.text().trim();
        if (labelText && labelText.length > 0 && labelText.length < 120) {
            $row.attr('title', labelText);
        }
    });
}

function _(key) {
    if (lang && lang[key]) {
        return lang[key];
    } else {
        return key;
    }
}

function uiInitMenu() {
    // Replace Electron remote Menu with a simple DOM dropdown.
    const menuHtml = '<ul id="configuration-menu-dropdown" class="nav-group" style="display:none;position:absolute;z-index:10000;background:#fff;border:1px solid #ccc;list-style:none;padding:4px 0;margin:0;min-width:120px;">' +
        '<li id="menu-new" style="padding:4px 12px;cursor:pointer;">' + _('ConfigurationMenu.New') + '</li>' +
        '<li id="menu-open" style="padding:4px 12px;cursor:pointer;">' + _('ConfigurationMenu.Open') + '</li>' +
        '<li id="menu-save" style="padding:4px 12px;cursor:pointer;">' + _('ConfigurationMenu.SaveAs') + '</li>' +
        '</ul>';
    $('body').append(menuHtml);

    $('#menu-new').click(async function () {
        try {
            const newConfig = await loadDefaultConfig();
            if (newConfig) {
                config = newConfig;
                uiUpdate();
            }
        } catch (err) {
            console.error('new config failed', err);
            showError('New Configuration', err.toString());
        }
        $('#configuration-menu-dropdown').hide();
    });

    $('#menu-open').click(async function () {
        try {
            const res = await openConfig();
            if (res) {
                config = res;
                uiUpdate();
            }
        } catch (err) {
            console.error('open config failed', err);
            showError('Open Configuration', err.toString());
        }
        $('#configuration-menu-dropdown').hide();
    });

    $('#menu-save').click(async function () {
        if (!config) {
            showError('Save Configuration', 'No configuration loaded');
            $('#configuration-menu-dropdown').hide();
            return;
        }
        try {
            await saveConfig(config);
        } catch (err) {
            console.error('save config failed', err);
            showError('Save Configuration', err.toString());
        }
        $('#configuration-menu-dropdown').hide();
    });

    $('#configuration-menu').click(function () {
        const offset = $('#configuration-menu').offset();
        const $dropdown = $('#configuration-menu-dropdown');
        const menuWidth = $dropdown.outerWidth() || 120;
        const windowWidth = $(window).width();
        let left = Math.floor(offset.left);
        if (left + menuWidth > windowWidth) {
            left = windowWidth - menuWidth - 10;
        }
        $dropdown.css({
            left: Math.max(10, left),
            top: 50
        }).toggle();
    });

    $(document).click(function (e) {
        if (!$(e.target).closest('#configuration-menu, #configuration-menu-dropdown').length) {
            $('#configuration-menu-dropdown').hide();
        }
    });
}

async function uiInit() {
    try {
        appVersion = await getAppVersion();
    } catch (e) {
    }
    $('#version').html('v' + appVersion);
    await uiTranslate();
    uiInitButtons();
    uiInitMenu();
    uiInitTabs();
    uiInitChangeHandlers();

    $('#link-new').click(async function () {
        config = await loadDefaultConfig();
        uiUpdate();
    });

    $('#link-open').click(async function () {
        try {
            const res = await openConfig();
            if (res) {
                config = res;
                uiUpdate();
            }
        } catch (err) {
            console.error('open config failed', err);
            showError('Open Configuration', err.toString());
        }
    });

    $('#link-download').click(function (event) {
        event.preventDefault();
        $('#connection-status').html(_('Status.Device') + ' ' + _('Status.Connecting'));
        ipc.send('download');
    });

    $(document).on('change', '.fox-val', function () {
        const id = $(this).attr('id');
        const currentVal = config[id];
        let newVal;
        if ($(this).attr('type') === 'checkbox') {
            newVal = $(this).is(':checked');
        } else {
            newVal = $(this).val();
        }

        switch (typeof currentVal) {
            case 'number':
                newVal = parseFloat(newVal);
                break;
            case 'boolean':
                if (newVal === 'false') {
                    newVal = false;
                } else {
                    newVal = Boolean(newVal);
                }
                break;
            default:
        }
        config[id] = newVal;
    });

    $(document).on('change', '.fox-pval', function () {
        const id = $(this).attr('id');
        // DEVIATION: IsCelcius is now a read-only label driven by TemperatureUnits;
        // ignore change events from it.
        if (id === 'IsCelcius') {
            return;
        }
        const currentVal = config.profiles[activeProfile][id];
        let newVal;
        if ($(this).attr('type') === 'checkbox') {
            newVal = $(this).is(':checked');
        } else {
            newVal = $(this).val();
        }

        switch (typeof currentVal) {
            case 'number':
                newVal = parseFloat(newVal);
                break;
            case 'boolean':
                if (newVal === 'false') {
                    newVal = false;
                } else {
                    newVal = Boolean(newVal);
                }
                break;
            default:
        }
        config.profiles[activeProfile][id] = newVal;
    });

    uiUpdate();
}

// Ensure the config object matches the current DOM values before uploading.
// This protects against missing/late change events on selects/checkboxes.
function syncConfigFromUi() {
    if (!config) return;

    $('.fox-val').each(function () {
        const id = $(this).attr('id');
        if (!id) return;
        const currentVal = config[id];
        let newVal;
        if ($(this).attr('type') === 'checkbox') {
            newVal = $(this).is(':checked');
        } else {
            newVal = $(this).val();
        }
        switch (typeof currentVal) {
            case 'number':
                newVal = parseFloat(newVal);
                break;
            case 'boolean':
                newVal = (newVal === 'false') ? false : Boolean(newVal);
                break;
            default:
        }
        config[id] = newVal;
    });

    $('.fox-pval').each(function () {
        const id = $(this).attr('id');
        // DEVIATION: IsCelcius is a read-only label driven by TemperatureUnits.
        if (!id || !config.profiles[activeProfile] || id === 'IsCelcius') return;
        const currentVal = config.profiles[activeProfile][id];
        let newVal;
        if ($(this).attr('type') === 'checkbox') {
            newVal = $(this).is(':checked');
        } else {
            newVal = $(this).val();
        }
        switch (typeof currentVal) {
            case 'number':
                newVal = parseFloat(newVal);
                break;
            case 'boolean':
                newVal = (newVal === 'false') ? false : Boolean(newVal);
                break;
            default:
        }
        config.profiles[activeProfile][id] = newVal;
    });
}

// Expose handlers for inline onclick attributes, menu items and keyboard shortcuts.
window.downloadSettings = function () {
    ipc.send('download');
};
window.uploadSettings = function () {
    if (!config) {
        showError('Upload Settings', 'No configuration loaded');
        return;
    }
    syncConfigFromUi();
    ipc.send('upload', config);
};
window.resetSettings = async function () {
    config = await loadDefaultConfig();
    ipc.send('upload', config);
    uiUpdate();
};

$(document).on('keydown', function (e) {
    if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA' || e.target.tagName === 'SELECT') {
        return;
    }
    if (e.key === 'd' || e.key === 'D') {
        window.downloadSettings();
    } else if (e.key === 'u' || e.key === 'U') {
        window.uploadSettings();
    }
});

let statsIconRotation = 0;
let statsIconVelocity = 0;
let statsIconScale = 1;
let statsIconLastClickTime = 0;
let statsIconAnimating = false;

const STATS_ICON_FRICTION = 0.94;
const STATS_ICON_MIN_VELOCITY = 0.1;
const STATS_ICON_MAX_BOOST = 40;
const STATS_ICON_MAX_SCALE = 2.0;

function updateStatsIconTransform() {
    const icon = document.getElementById('stats-icon');
    if (icon) {
        icon.style.transform = `rotate(${statsIconRotation.toFixed(2)}deg) scale(${statsIconScale.toFixed(3)})`;
    }
}

function updateStatsIconFrame() {
    if (Math.abs(statsIconVelocity) < STATS_ICON_MIN_VELOCITY) {
        statsIconVelocity = 0;
    }

    if (statsIconVelocity === 0 && Math.abs(statsIconScale - 1) < 0.005) {
        statsIconScale = 1;
        statsIconAnimating = false;
        updateStatsIconTransform();
        return;
    }

    statsIconRotation = (statsIconRotation + statsIconVelocity) % 360;
    statsIconVelocity *= STATS_ICON_FRICTION;

    const targetScale = 1 + Math.min(statsIconVelocity / STATS_ICON_MAX_BOOST, 1) * (STATS_ICON_MAX_SCALE - 1);
    statsIconScale += (targetScale - statsIconScale) * 0.2;

    updateStatsIconTransform();
    requestAnimationFrame(updateStatsIconFrame);
}

function onStatsIconClick() {
    const now = performance.now();
    const dt = now - statsIconLastClickTime;
    statsIconLastClickTime = now;

    const boost = Math.min(Math.max(0, (500 - dt) / 40), STATS_ICON_MAX_BOOST);
    statsIconVelocity += boost;

    // Immediate size pop on each click.
    statsIconScale = Math.min(statsIconScale + 0.08, STATS_ICON_MAX_SCALE);
    updateStatsIconTransform();

    if (!statsIconAnimating) {
        statsIconAnimating = true;
        requestAnimationFrame(updateStatsIconFrame);
    }
}

function uiInitStatsIcon() {
    const icon = document.getElementById('stats-icon');
    if (!icon) return;
    icon.addEventListener('click', onStatsIconClick);
    icon.addEventListener('error', () => {
        icon.style.display = 'none';
    });
}

let bootIconRotation = 0;
let bootIconVelocity = 0;
let bootIconScale = 1;
let bootIconLastClickTime = 0;
let bootIconAnimating = false;
let bootIconSupercharged = false;
let bootIconAltActive = false;

const BOOT_ICON_SPRING = 0.015;
const BOOT_ICON_DAMPING = 0.97;
const BOOT_ICON_MIN_VELOCITY = 0.05;
const BOOT_ICON_MAX_BOOST = 65;
const BOOT_ICON_MAX_SCALE = 2.2;
const BOOT_ICON_SUPERCHARGE_THRESHOLD = 25;

function updateBootIconTransform() {
    const icon = document.getElementById('boot-icon');
    const alt = document.getElementById('boot-icon-alt');
    const transform = `rotate(${bootIconRotation.toFixed(2)}deg) scale(${bootIconScale.toFixed(3)})`;
    if (icon) icon.style.transform = transform;
    if (alt) alt.style.transform = transform;
}

function shortestRotationToRest(rotation) {
    // Signed shortest distance from `rotation` to the nearest multiple of 360.
    const mod = rotation % 360;
    const offset = (mod + 360) % 360;
    if (offset > 180) return offset - 360;
    return offset;
}

function updateBootIconFrame() {
    const displacement = shortestRotationToRest(bootIconRotation);

    bootIconVelocity -= BOOT_ICON_SPRING * displacement;
    bootIconVelocity *= BOOT_ICON_DAMPING;
    bootIconScale += (1 - bootIconScale) * 0.08;
    bootIconRotation += bootIconVelocity;

    if (bootIconVelocity > BOOT_ICON_SUPERCHARGE_THRESHOLD && !bootIconSupercharged) {
        bootIconSupercharged = true;
        bootIconAltActive = !bootIconAltActive;
        setBootIconCrossfade(bootIconAltActive);
    } else if (bootIconVelocity < BOOT_ICON_SUPERCHARGE_THRESHOLD * 0.5) {
        bootIconSupercharged = false;
    }

    if (Math.abs(bootIconVelocity) < BOOT_ICON_MIN_VELOCITY &&
        Math.abs(displacement) < 0.5 &&
        Math.abs(bootIconScale - 1) < 0.005) {
        bootIconRotation -= displacement;
        bootIconScale = 1;
        bootIconVelocity = 0;
        bootIconAnimating = false;
        updateBootIconTransform();
        return;
    }

    updateBootIconTransform();
    requestAnimationFrame(updateBootIconFrame);
}

function onBootIconClick() {
    const now = performance.now();
    const dt = now - bootIconLastClickTime;
    bootIconLastClickTime = now;

    const boost = Math.min(Math.max(0, (500 - dt) / 20), BOOT_ICON_MAX_BOOST);
    bootIconVelocity += boost;

    bootIconScale = Math.min(bootIconScale + 0.08, BOOT_ICON_MAX_SCALE);
    updateBootIconTransform();

    if (!bootIconAnimating) {
        bootIconAnimating = true;
        requestAnimationFrame(updateBootIconFrame);
    }
}

function setBootIconCrossfade(showAlt) {
    const icon = document.getElementById('boot-icon');
    const alt = document.getElementById('boot-icon-alt');
    if (!icon || !alt) return;
    icon.style.opacity = showAlt ? '0' : '1';
    alt.style.opacity = showAlt ? '1' : '0';
}

function uiInitBootIcon() {
    const stack = document.getElementById('boot-icon-stack');
    if (!stack) return;
    stack.addEventListener('click', onBootIconClick);
}

// Wrap the main tab bar and the view container in a full-width wrapper so
// the tab bars extend to the window borders while the view content scales
// from the center. The HTML keeps them as direct children of .window-content
// for backward compatibility; we relocate them at runtime so the transform
// applies consistently.
function uiWrapContentForScaling() {
    const main = document.getElementById('main');
    const content = document.querySelector('.window-content');
    const view = document.querySelector('.window-content > .view-container.view-main');
    if (!main || !content || !view) return;
    if (content.querySelector('.content-scale-wrap')) return;

    const wrap = document.createElement('div');
    wrap.className = 'content-scale-wrap';
    wrap.appendChild(main);
    wrap.appendChild(view);
    content.appendChild(wrap);
}

// Scale the view content to fit the window while keeping the header/footer
// at their natural size. The wrapper is sized to the base window dimensions
// and centered in the content area; the tab bars stretch to the wrapper edges
// and scale with it so they reach the window borders while staying centered.
function updateContentZoom() {
    const header = document.querySelector('.toolbar-header');
    const footer = document.querySelector('.toolbar-footer');
    const content = document.querySelector('.window-content');
    if (!header || !footer || !content) return;

    const baseWidth = 536;
    const baseHeight = 621;
    const headerHeight = header.offsetHeight;
    const footerHeight = footer.offsetHeight;

    const baseContentHeight = baseHeight - headerHeight - footerHeight;
    // Measure the REAL flex container instead of reconstructing it from
    // window.innerHeight - header - footer: transient header/footer heights
    // (web fonts, i18n text) made the first estimate wrong, and with
    // zoom-based layout an oversized wrapper gets clipped under the header
    // until the next resize.
    const availableWidth = content.clientWidth;
    const availableContentHeight = content.clientHeight;

    const scale = Math.min(availableWidth / baseWidth, availableContentHeight / baseContentHeight);
    document.documentElement.style.setProperty('--base-width', `${baseWidth}px`);
    document.documentElement.style.setProperty('--base-content-height', `${baseContentHeight}px`);
    document.documentElement.style.setProperty('--content-scale', scale.toFixed(4));
}

// DEVIATION: Lock the Tauri window to the base aspect ratio so every resize is
// diagonal. This keeps the fixed 536x621 UI layout proportional and avoids the
// dead-space / tab-bar-stretch problems that come from free-form resizing.
const BASE_WINDOW_WIDTH = 536;
const BASE_WINDOW_HEIGHT = 621;
const BASE_ASPECT = BASE_WINDOW_WIDTH / BASE_WINDOW_HEIGHT;

function lockWindowAspectRatio() {
    const win = getCurrentWindow();
    let enforcing = false;
    win.onResized(({ payload: size }) => {
        if (enforcing) return;
        const { width, height } = size;
        const aspect = width / height;
        if (Math.abs(aspect - BASE_ASPECT) < 0.001) return;
        let newWidth = width;
        let newHeight = height;
        if (aspect > BASE_ASPECT) {
            newWidth = Math.round(height * BASE_ASPECT);
        } else {
            newHeight = Math.round(width / BASE_ASPECT);
        }
        enforcing = true;
        win.setSize(new PhysicalSize(newWidth, newHeight)).finally(() => {
            enforcing = false;
        });
    });
}

window.addEventListener('resize', updateContentZoom);

uiWrapContentForScaling();
uiInit();
uiInitStatsIcon();
uiInitBootIcon();
updateContentZoom();
lockWindowAspectRatio();

// Re-run once fonts and i18n text have settled: the header/footer heights at
// first paint can differ from their final values, which would leave the
// zoom-scaled wrapper mis-sized (tab bar clipped under the header) until the
// next window resize.
if (document.fonts && document.fonts.ready) {
    document.fonts.ready.then(updateContentZoom);
}
requestAnimationFrame(() => requestAnimationFrame(updateContentZoom));
window.addEventListener('load', updateContentZoom);
