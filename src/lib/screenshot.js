// Device screen capture (HID cmd 0xC1) decoding, shared by the Device
// Monitor and the Firmware Editor status page.
//
// The 1024-byte framebuffer is 1bpp HORIZONTALLY packed, MSB = leftmost
// pixel, one row = width/8 bytes. This matches NToolbox, which copies the
// bytes verbatim into a GDI+ Format1bppIndexed bitmap
// (CreateBitmapFromBytesArray) — pixel (x,y) = buf[y*(width/8) + x/8],
// bit 0x80 >> (x%8).
//
// Geometry heuristic mirrors NToolbox's TakeScreenshot: 96x16 if the
// framebuffer past 192 bytes is all zero, else 64x128. Extended with the
// 128x32 Pico 25 / Sinuous CB-80 panel (512 bytes) — a panel geometry the
// flasher knows about (see flasher.rs read_product_id) but NToolbox
// mis-renders as 64x128.
export const SCREENSHOT_GEOMS = [
    { width: 96, height: 16, bytes: 192 },
    { width: 128, height: 32, bytes: 512 },
    { width: 64, height: 128, bytes: 1024 },
];

export function detectScreenshotGeometry(raw) {
    const tailZero = (from) => {
        for (let i = from; i < raw.length; i++) {
            if (raw[i] !== 0) return false;
        }
        return true;
    };
    if (tailZero(SCREENSHOT_GEOMS[0].bytes)) return SCREENSHOT_GEOMS[0];
    if (tailZero(SCREENSHOT_GEOMS[1].bytes)) return SCREENSHOT_GEOMS[1];
    return SCREENSHOT_GEOMS[2];
}

/// Render the raw framebuffer onto `canvas` (ArcticFox LCD green on black,
/// integer zoom). Returns the detected {width, height}.
export function drawScreenshot(canvas, raw, zoom = 4) {
    const { width, height } = detectScreenshotGeometry(raw);
    canvas.width = width * zoom;
    canvas.height = height * zoom;
    const ctx = canvas.getContext('2d');
    ctx.fillStyle = '#000';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.fillStyle = '#9acd32'; // ArcticFox LCD green
    const stride = width >> 3;
    for (let y = 0; y < height; y++) {
        for (let x = 0; x < width; x++) {
            const i = y * stride + (x >> 3);
            if (i < raw.length && (raw[i] & (0x80 >> (x & 7)))) {
                ctx.fillRect(x * zoom, y * zoom, zoom, zoom);
            }
        }
    }
    return { width, height };
}

export function base64ToBytes(b64) {
    const bin = atob(b64);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) {
        out[i] = bin.charCodeAt(i);
    }
    return out;
}
