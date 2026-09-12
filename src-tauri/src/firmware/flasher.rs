use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::Duration;

use super::{FirmwareError, Result};

#[derive(Debug, Clone, Copy)]
struct DeviceId {
    vid: u16,
    pid: u16,
}

/// MCU family of a connected device, keyed by USB vendor ID (NCore.USB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum McuFamily {
    /// Nuvoton M451 line (Pico and most Joyetech/Eleaf ArcticFox devices).
    Nuvoton = 0,
    /// STMicro STM32 line (Eleaf iStick Rim C et al., VID 0483).
    Stm = 1,
}

impl McuFamily {
    fn from_vid(vid: u16) -> McuFamily {
        match vid {
            0x0483 => McuFamily::Stm,
            _ => McuFamily::Nuvoton,
        }
    }

    /// Command-packet signature bytes (NCore.USB.HidConnector): the STM32
    /// bootloader answers nothing to the Nuvoton "HIDC" signature.
    fn hid_signature(self) -> [u8; 4] {
        match self {
            McuFamily::Nuvoton => *b"HIDC",
            McuFamily::Stm => [0x5C, 0xCA, 0x37, 0x75],
        }
    }

    /// APROM address sent as arg1 of the 0xC3 WriteFirmware command
    /// (NCore FirmwareStartAddress: 0 for Nuvoton, flash base for STM).
    fn firmware_start_address(self) -> i32 {
        match self {
            McuFamily::Nuvoton => 0,
            McuFamily::Stm => 0x0800C000,
        }
    }
}

/// Family of the most recently opened device. Flashing/monitoring operations
/// are strictly sequential (the sidecar is suspended while flashing), so a
/// static is safe and avoids threading a parameter through every call site.
static CURRENT_FAMILY: AtomicU8 = AtomicU8::new(McuFamily::Nuvoton as u8);

fn current_family() -> McuFamily {
    match CURRENT_FAMILY.load(Ordering::SeqCst) {
        x if x == McuFamily::Stm as u8 => McuFamily::Stm,
        _ => McuFamily::Nuvoton,
    }
}

const SUPPORTED_DEVICES: &[DeviceId] = &[
    DeviceId { vid: 0x0416, pid: 0x5020 }, // Pico / Joyetech (Nuvoton line)
    DeviceId { vid: 0x0483, pid: 0x5750 }, // Eleaf Rim C / STM32-line ArcticFox
];

const CMD_READ_DATAFLASH: u8 = 0x35;
const CMD_WRITE_DATAFLASH: u8 = 0x53;
const CMD_WRITE_DATA: u8 = 0xC3;
const CMD_RESTART: u8 = 0xB4;
const CMD_SET_LOGO: u8 = 0xA5;
const CMD_READ_MONITORING_DATA: u8 = 0x66;
/// Monitoring telemetry payload size in bytes.
const MONITORING_DATA_SIZE: usize = 64;

const DATAFLASH_SIZE: usize = 2048;
const REPORT_SIZE: usize = 64;

/// APROM offset of the logo blocks (from NFirmwareEditor HidConnector).
const LOGO_OFFSET: i32 = 102400;
/// Two 512-byte image blocks.
const LOGO_LENGTH: usize = 1024;

/// Connect to the first bootloader HID interface found.
pub fn open_device() -> Result<hidapi::HidDevice> {
    let api = hidapi::HidApi::new().map_err(|e| FirmwareError::Other(e.to_string()))?;
    for dev_id in SUPPORTED_DEVICES {
        if let Ok(device) = api.open(dev_id.vid, dev_id.pid) {
            CURRENT_FAMILY.store(McuFamily::from_vid(dev_id.vid) as u8, Ordering::SeqCst);
            return Ok(device);
        }
    }
    Err(FirmwareError::Other("No supported HID device found".into()))
}

/// Information about one connected device.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceInfo {
    /// Stable identifier within a session: the HID device path.
    pub path: String,
    pub serial: String,
    pub product: String,
}

/// Enumerate all connected bootloader HID interfaces.
pub fn list_devices() -> Result<Vec<DeviceInfo>> {
    let api = hidapi::HidApi::new().map_err(|e| FirmwareError::Other(e.to_string()))?;
    let mut out = Vec::new();
    for d in api.device_list() {
        let is_supported = SUPPORTED_DEVICES.iter().any(|id| 
            d.vendor_id() == id.vid && d.product_id() == id.pid
        );
        if is_supported {
            out.push(DeviceInfo {
                path: d.path().to_string_lossy().into_owned(),
                serial: d.serial_number().unwrap_or_default().to_string(),
                product: d.product_string().unwrap_or_default().to_string(),
            });
        }
    }
    Ok(out)
}

/// Connect to a specific device by HID path (from [`list_devices`]).
pub fn open_device_by_path(path: &str) -> Result<hidapi::HidDevice> {
    let api = hidapi::HidApi::new().map_err(|e| FirmwareError::Other(e.to_string()))?;
    if let Some(info) = api.device_list().find(|d| d.path().to_string_lossy() == path) {
        CURRENT_FAMILY.store(McuFamily::from_vid(info.vendor_id()) as u8, Ordering::SeqCst);
    }
    let device = api
        .open_path(std::ffi::CString::new(path).map_err(|e| FirmwareError::Other(e.to_string()))?.as_c_str())
        .map_err(|e| FirmwareError::Other(format!("cannot open HID device {path}: {e}")))?;
    Ok(device)
}

fn create_command(cmd: u8, arg1: i32, arg2: i32) -> [u8; 18] {
    let mut packet = [0u8; 18];
    packet[0] = cmd;
    packet[1] = 0x0E;
    packet[2..6].copy_from_slice(&arg1.to_le_bytes());
    packet[6..10].copy_from_slice(&arg2.to_le_bytes());
    packet[10..14].copy_from_slice(&current_family().hid_signature());
    let sum: i32 = packet[0..14].iter().map(|b| *b as i32).sum();
    packet[14..18].copy_from_slice(&sum.to_le_bytes());
    packet
}

fn send_command(device: &mut hidapi::HidDevice, cmd: u8, arg1: i32, arg2: i32) -> Result<()> {
    let packet = create_command(cmd, arg1, arg2);
    let mut report = vec![0u8; REPORT_SIZE + 1];
    report[1..1 + packet.len()].copy_from_slice(&packet);
    device.write(&report).map_err(|e| FirmwareError::Other(e.to_string()))?;
    Ok(())
}

/// Test-only re-export for hardware experiments.
pub fn read_exact_pub(device: &mut hidapi::HidDevice, len: usize) -> Result<Vec<u8>> {
    read_exact(device, len)
}

/// Test-only: 0x35 with arbitrary args, for the absread-patched firmware
/// (pico_absread.bin turns 0x35 into an absolute memory read).
pub fn read_abs_pub(device: &mut hidapi::HidDevice, addr: u32, len: u32) -> Result<Vec<u8>> {
    send_command(device, CMD_READ_DATAFLASH, addr as i32, len as i32)?;
    read_exact(device, len as usize)
}

/// Test-only re-export for hardware experiments.
pub fn send_command_pub(device: &mut hidapi::HidDevice, cmd: u8, arg1: i32, arg2: i32) -> Result<()> {
    send_command(device, cmd, arg1, arg2)
}

fn read_exact(device: &mut hidapi::HidDevice, len: usize) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(len);
    let mut chunk = [0u8; REPORT_SIZE + 1];
    while buf.len() < len {
        let n = device
            .read_timeout(&mut chunk, 5000)
            .map_err(|e| FirmwareError::Other(e.to_string()))?;
        if n == 0 {
            return Err(FirmwareError::Other(format!(
                "HID read timed out (got {} of {len} bytes)",
                buf.len()
            )));
        }
        // Devices with report ID 0 return no ID prefix on hidraw: every
        // returned byte is payload.
        buf.extend_from_slice(&chunk[..n]);
    }
    buf.truncate(len);
    Ok(buf)
}

/// Read the device dataflash (2048 bytes: 4-byte checksum + 2044 bytes data).
pub fn read_dataflash() -> Result<Vec<u8>> {
    let mut device = open_device()?;
    read_dataflash_from(&mut device)
}

/// Read dataflash from an already-open device.
pub fn read_dataflash_from(device: &mut hidapi::HidDevice) -> Result<Vec<u8>> {
    send_command(device, CMD_READ_DATAFLASH, 0, DATAFLASH_SIZE as i32)?;
    read_exact(device, DATAFLASH_SIZE)
}

/// Firmware version from a raw dataflash buffer (4-byte checksum prefix +
/// 2044 bytes data; version is a little-endian i32 at data offset 256).
pub fn parse_fw_version(dataflash: &[u8]) -> Result<i32> {
    if dataflash.len() < 4 + 260 {
        return Err(FirmwareError::Other("dataflash too short".into()));
    }
    Ok(i32::from_le_bytes(dataflash[4 + 256..4 + 260].try_into().unwrap()))
}

/// Read the device firmware version over HID.
pub fn read_fw_version() -> Result<i32> {
    parse_fw_version(&read_dataflash()?)
}


fn dataflash_checksum(data: &[u8]) -> u32 {
    data.iter().fold(0u32, |a, b| a.wrapping_add(*b as u32))
}

/// Write the device dataflash.
pub fn write_dataflash(data: &[u8; 2044]) -> Result<()> {
    let mut device = open_device()?;
    let mut payload = Vec::with_capacity(DATAFLASH_SIZE);
    payload.extend_from_slice(&dataflash_checksum(data).to_le_bytes());
    payload.extend_from_slice(data);

    send_command(&mut device, CMD_WRITE_DATAFLASH, 0, DATAFLASH_SIZE as i32)?;

    for chunk in payload.chunks(REPORT_SIZE) {
        let mut report = vec![0u8; REPORT_SIZE + 1];
        report[1..1 + chunk.len()].copy_from_slice(chunk);
        device.write(&report).map_err(|e| FirmwareError::Other(e.to_string()))?;
    }
    Ok(())
}

/// Restart the device.
pub fn restart_device() -> Result<()> {
    let mut device = open_device()?;
    send_command(&mut device, CMD_RESTART, 0, 0)?;
    Ok(())
}

fn write_all(device: &mut hidapi::HidDevice, data: &[u8]) -> Result<()> {
    for chunk in data.chunks(REPORT_SIZE) {
        let mut report = vec![0u8; REPORT_SIZE + 1];
        report[1..1 + chunk.len()].copy_from_slice(chunk);
        device.write(&report).map_err(|e| FirmwareError::Other(e.to_string()))?;
    }
    Ok(())
}

/// Hot-swap the on-screen logo blocks without entering bootloader mode
/// (NFE "WriteLogoHot"). `block1`/`block2` are up to 512 bytes each; pass
/// empty/zero blocks to remove the logo.
pub fn set_logo(device: &mut hidapi::HidDevice, block1: &[u8], block2: &[u8]) -> Result<()> {
    if block1.len() > 512 || block2.len() > 512 {
        return Err(FirmwareError::Other("logo block too big (max 512 bytes)".into()));
    }
    let mut data = vec![0u8; LOGO_LENGTH];
    data[..block2.len()].copy_from_slice(block2);
    data[512..512 + block1.len()].copy_from_slice(block1);
    send_command(device, CMD_SET_LOGO, LOGO_OFFSET, LOGO_LENGTH as i32)?;
    write_all(device, &data)
}

/// Remove the on-screen logo (writes zeroed blocks).
pub fn clear_logo(device: &mut hidapi::HidDevice) -> Result<()> {
    set_logo(device, &[], &[])
}

const CMD_SCREENSHOT: u8 = 0xC1;
const SCREENSHOT_SIZE: usize = 0x400;

/// Capture the current screen contents (1024 bytes, 64x128 vertical packing).
/// ArcticFox firmware only — stock Joyetech v1.00 has no 0xC1 handler (its
/// dispatcher services 0x35/0x3C/0x53/0x7C/0xB4) and silently drops the
/// command, so on stock firmware this fails with an HID read timeout.
pub fn screenshot(device: &mut hidapi::HidDevice) -> Result<Vec<u8>> {
    send_command(device, CMD_SCREENSHOT, 0, SCREENSHOT_SIZE as i32)?;
    read_exact(device, SCREENSHOT_SIZE)
}

const CMD_SET_DATETIME: u8 = 0x64;

/// Set the device clock (also wakes the display).
/// Payload: [year LE u16, month, day, hour, minute, second].
pub fn set_date_time(device: &mut hidapi::HidDevice, y: u16, mo: u8, d: u8, h: u8, mi: u8, s: u8) -> Result<()> {
    send_command(device, CMD_SET_DATETIME, 0, 0)?;
    let mut payload = vec![0u8; 7];
    payload[0..2].copy_from_slice(&y.to_le_bytes());
    payload[2] = mo; payload[3] = d; payload[4] = h; payload[5] = mi; payload[6] = s;
    write_all(device, &payload)
}

/// Read the device monitoring telemetry (64 bytes), opening the device automatically.
pub fn read_monitoring_data_auto() -> Result<Vec<u8>> {
    let mut device = open_device()?;
    read_monitoring_data(&mut device)
}

/// Read device monitoring telemetry (64 bytes).
pub fn read_monitoring_data(device: &mut hidapi::HidDevice) -> Result<Vec<u8>> {
    send_command(device, CMD_READ_MONITORING_DATA, 0, MONITORING_DATA_SIZE as i32)?;
    read_exact(device, MONITORING_DATA_SIZE)
}

/// Read the product ID from dataflash bytes 316..320 (ASCII), opening the
/// device automatically.
///
/// **Why this 4-byte string matters beyond identification** (NFE ground
/// truth, traced from af_190602): ArcticFox is ONE universal binary per MCU
/// line — there are no per-device or per-screen firmware builds. Boot
/// dispatch (`0x302C`) reads the PID from dataflash offset `0x13C` (316),
/// maps it to a device class (`0x20002C2E`), then a display model
/// (`0x20002750`), then a panel init table + render geometry. Unknown/erased
/// PID → no dispatch match → the firmware hangs in an infinite loop
/// (`bl 0x2FF8`) and the device crash-loops or drops off USB. Screen size is
/// therefore selected at RUNTIME per PID:
///
/// | Panel | Devices (NFE IDs) | Framebuffer |
/// |---|---|---|
/// | 64×128 (default/fallback) | classic Joyetech VTC/Cuboid/Primo family | 1024 B, stride 8 |
/// | 96×16 | Pico M041, Pico Dual M065, Pico Mega M045, Pico RDTA M038, ASTER M037, TC100W/200W/QC200W | 192 B, stride 12 |
/// | 128×32 | Pico 25 M077, Sinuous CB-80 J070 | 512 B, stride 16 |
/// | 64×48 | Wismec RX family, Presa, Predator, Sinuous P80, Invoke M095, ES300 | 384 B |
/// | 64×32 | Pico Squeeze 2 M105, ASTER RT M064 | 256 B |
///
/// Consequences for the flasher:
/// - After ANY flash, the dataflash PID must still read correctly at
///   316..320 or the device boots into the dispatch hang (observed on the
///   Pico Dual: repeated Nuvoton→Joyetech re-enumeration, then silence).
/// - A "Force PID" repair (rewrite df[316..320] with the expected ID and
///   reboot) is the recovery operation when a flash leaves the PID erased —
///   NToolbox exposes the same tool.
pub fn read_product_id() -> Result<String> {
    let data = read_dataflash()?;
    if data.len() < 320 {
        return Err(FirmwareError::Other("dataflash too short".into()));
    }
    let bytes = &data[316..320];
    String::from_utf8(bytes.to_vec()).map_err(|_| FirmwareError::Other("invalid Product ID".into()))
}

/// NFE "Force PID": repair the dataflash product ID (bytes 316..320) when a
/// flash left it erased or wrong, so boot dispatch resolves the right device
/// class and panel geometry instead of hanging on the unknown-PID loop
/// (`bl 0x2FF8`, see the table above `read_product_id`). The boot flag is
/// cleared in the same write so the device restarts into the flashed
/// firmware; the LDROM updater stays reachable via `ensure_ldrom_mode`.
/// No-op when the ID already matches. Verifies the write stuck before
/// returning.
pub fn force_product_id(expected: &str) -> Result<()> {
    let pid_bytes = expected.as_bytes();
    if pid_bytes.len() != 4 {
        return Err(FirmwareError::Other("product ID must be 4 ASCII bytes".into()));
    }
    let data = read_dataflash()?;
    if data.len() < 320 {
        return Err(FirmwareError::Other("dataflash too short".into()));
    }
    if &data[316..320] == pid_bytes {
        return Ok(()); // already correct
    }
    // Preserve everything, patch only the ID slot (absolute 316 = user 312)
    // and clear the boot flag so the restart boots the flashed firmware.
    let mut user_data = [0u8; 2044];
    user_data.copy_from_slice(&data[4..]);
    user_data[312..316].copy_from_slice(pid_bytes);
    user_data[9] = 0;
    write_dataflash(&user_data)?;
    // NToolbox sleeps 100 ms here: give the device time to commit the
    // dataflash sector before the restart command arrives.
    thread::sleep(Duration::from_millis(100));
    restart_device()?;
    // Wait for re-enumeration and confirm the ID stuck.
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(500));
        if let Ok(after) = read_dataflash() {
            if after.len() >= 320 && &after[316..320] == pid_bytes {
                return Ok(());
            }
        }
    }
    Err(FirmwareError::Other(
        "device did not come back with the repaired product ID".into(),
    ))
}

/// Switch the device to LDROM bootloader mode if needed.
pub fn ensure_ldrom_mode() -> Result<()> {
    let data = read_dataflash()?;
    // Data area starts after the 4-byte checksum; BootFlagOffset is 9.
    const BOOT_FLAG: usize = 4 + 9;
    if data.len() <= BOOT_FLAG {
        return Err(FirmwareError::Other("dataflash too short".into()));
    }
    if data[BOOT_FLAG] == 1 {
        return Ok(());
    }

    // Set boot flag and write back.
    let mut user_data = [0u8; 2044];
    user_data.copy_from_slice(&data[4..]);
    user_data[9] = 1; // BootFlagOffset within the 2044-byte data area
    write_dataflash(&user_data)?;
    // NToolbox sleeps 100 ms here: give the device time to commit the
    // dataflash sector before the restart command arrives.
    thread::sleep(Duration::from_millis(100));
    restart_device()?;

    // Wait for re-enumeration and confirm the LDROM boot flag, like NFE does.
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(500));
        if let Ok(mut device) = open_device() {
            if let Ok(data) = read_dataflash_from(&mut device) {
                if data[4 + 9] == 1 {
                    return Ok(());
                }
            }
        }
    }
    Err(FirmwareError::DidNotReenumerate)
}

/// One firmware-write attempt: open the device, send the 0xC3 WriteData
/// command, and stream the whole image. Retried as a unit by
/// [`flash_firmware_guarded`] (see its NToolbox-parity comment).
fn write_firmware_stream(bytes: &[u8]) -> Result<()> {
    let mut device = open_device()?;

    // The stock LDROM updater programs whole flash rows and cannot handle a
    // ragged final partial report: observed on real hardware (Pico Dual,
    // M065), it drops off USB deterministically at the last <64 B chunk,
    // leaving a partially written APROM crash-looping the MCU. Pad the
    // stream with 0xFF (erased flash) to a whole 512-byte flash row and
    // declare the padded length, so every report is a full 64 bytes and
    // every programmed row is complete.
    const FLASH_ROW: usize = 512;
    let padded_len = bytes.len().div_ceil(FLASH_ROW) * FLASH_ROW;
    if padded_len > 128 * 1024 {
        return Err(FirmwareError::Other(format!(
            "firmware image too large after row padding: {padded_len} bytes"
        )));
    }

    send_command(
        &mut device,
        CMD_WRITE_DATA,
        current_family().firmware_start_address(),
        padded_len as i32,
    )?;

    for chunk_start in (0..padded_len).step_by(REPORT_SIZE) {
        // 0xFF fill: any padding past the image end must look like erased
        // flash, and the leading report-ID byte stays 0.
        let mut report = vec![0xFFu8; REPORT_SIZE + 1];
        report[0] = 0;
        let end = (chunk_start + REPORT_SIZE).min(bytes.len());
        if chunk_start < bytes.len() {
            report[1..1 + end - chunk_start].copy_from_slice(&bytes[chunk_start..end]);
        }
        // The LDROM programs flash slower than USB can stream; it NAKs the
        // OUT endpoint while busy, so retry with backoff instead of failing.
        let mut tries = 0;
        loop {
            match device.write(&report) {
                Ok(_) => break,
                Err(e) => {
                    tries += 1;
                    if tries > 50 {
                        return Err(FirmwareError::Other(format!("flash write failed: {e}")));
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
    Ok(())
}

/// Flash a firmware image to the device, restart it, and verify it boots.
///
/// The image is streamed exactly as provided: encrypted (e.g. VandalProof)
/// images are decrypted by the on-device LDROM updater, plaintext images are
/// accepted as-is. If `expected_product_id` is given, the device dataflash
/// Product ID must match before anything is written.
/// Sanity-check the image before touching the device: the Nuvoton APROM
/// is 128K flash minus the LDROM, and the largest known ArcticFox build
/// is ~114K — anything larger would be written past the APROM end.
fn check_image_size(bytes: &[u8]) -> Result<()> {
    const MAX_FIRMWARE_SIZE: usize = 128 * 1024;
    if !(1024..=MAX_FIRMWARE_SIZE).contains(&bytes.len()) {
        return Err(FirmwareError::Other(format!(
            "implausible firmware image size: {} bytes",
            bytes.len()
        )));
    }
    Ok(())
}

pub fn flash_firmware_guarded(
    bytes: &[u8],
    expected_product_id: Option<&str>,
    on_progress: &dyn Fn(&str),
) -> Result<()> {
    check_image_size(bytes)?;

    // Check the Product ID guard before switching to LDROM, so a refused
    // flash leaves the device running its current firmware.
    if let Some(want) = expected_product_id {
        let mut device = open_device()?;
        let df = read_dataflash_from(&mut device)?;
        if df.len() < 320 {
            return Err(FirmwareError::Other("dataflash too short".into()));
        }
        let pid = String::from_utf8_lossy(&df[316..320]).into_owned();
        if pid != want {
            return Err(FirmwareError::Other(format!(
                "refusing to flash: connected device is {pid}, firmware is for {want}"
            )));
        }
    }

    match ensure_ldrom_mode() {
        Ok(()) => {}
        // Some devices (observed: Pico Dual) ignore the soft boot-flag switch
        // and stay in APROM mode. Fall back to the recovery-style protocol:
        // wait for the user to bring the device up in bootloader mode
        // (unplug/replug, holding a button while plugging in if needed), then
        // stream as usual.
        Err(FirmwareError::DidNotReenumerate) => {
            on_progress(
                "device did not enter bootloader mode — unplug it and replug it \
                 (hold a button while plugging in if it doesn't come up); waiting…",
            );
            let mut waited = 0u32;
            loop {
                thread::sleep(Duration::from_millis(500));
                waited += 1;
                if let Ok(mut device) = open_device() {
                    if let Ok(data) = read_dataflash_from(&mut device) {
                        if data.len() > 4 + 9 && data[4 + 9] == 1 {
                            on_progress("device found in bootloader mode — flashing…");
                            break;
                        }
                    }
                }
                if waited % 8 == 0 {
                    on_progress("still waiting for the device in bootloader mode…");
                }
            }
        }
        Err(e) => return Err(e),
    }

    // NToolbox retries the ENTIRE WriteFirmware (0xC3 command + full stream)
    // for 15 s on any failure. Do the same: aborting mid-stream leaves a
    // partially programmed APROM (apparent brick), while the LDROM updater
    // cleanly restarts an interrupted update on the next attempt.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        match write_firmware_stream(bytes) {
            Ok(()) => break,
            Err(e) => {
                if std::time::Instant::now() >= deadline {
                    return Err(e);
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    }

    // Restart and wait for the device to come back in APROM mode. The LDROM
    // may drop USB briefly while finalizing the flash, making a single-shot
    // restart fail transiently — retry before giving up, so a successful
    // flash is never reported as failed.
    let mut last_err = None;
    for _ in 0..10 {
        match restart_device() {
            Ok(()) => {
                last_err = None;
                break;
            }
            Err(e) => {
                last_err = Some(e);
                thread::sleep(Duration::from_millis(300));
            }
        }
    }
    if let Some(e) = last_err {
        return Err(e);
    }
    for _ in 0..20 {
        thread::sleep(Duration::from_millis(500));
        if let Ok(mut device) = open_device() {
            if let Ok(data) = read_dataflash_from(&mut device) {
                if data[4 + 9] == 0 {
                    return Ok(());
                }
            }
        }
    }
    Err(FirmwareError::Other("device did not come back after flashing".into()))
}

/// Flash a decrypted firmware image to the device (no product-id guard).
pub fn flash_firmware(bytes: &[u8]) -> Result<()> {
    flash_firmware_guarded(bytes, None, &|_| {})
}

/// Emergency recovery flash.
///
/// Waits indefinitely for a device to enumerate (a bricked mod in LDROM mode
/// may flap on and off the bus), optionally requiring a matching Product ID,
/// then flashes `bytes` and verifies the device comes back in APROM mode.
/// `on_progress` receives human-readable status lines.
pub fn recovery_flash(
    bytes: &[u8],
    expected_product_id: Option<&str>,
    on_progress: impl Fn(&str),
) -> Result<()> {
    // Deterministic validation up front: the wait→flash loop below exists
    // for transient device flapping; a bad image would otherwise loop forever
    // with the sidecar permanently suspended.
    check_image_size(bytes)?;
    // A bricked device can flap on and off the bus at any point — including
    // between device-found and flash-start. Loop the whole wait→flash cycle:
    // the LDROM updater erases and restarts an interrupted update cleanly on
    // the next attempt (verified in the hardware recovery experiments).
    loop {
        // Phase 1: wait for the device (and the right one).
        on_progress("waiting for device — unplug and replug it, or hold a button while plugging in");
        loop {
            match open_device() {
                Ok(mut device) => match read_dataflash_from(&mut device) {
                    Ok(df) => {
                        if df.len() >= 320 {
                            let pid = String::from_utf8_lossy(&df[316..320]).into_owned();
                            if let Some(want) = expected_product_id {
                                if pid != want {
                                    on_progress(&format!("found {pid}, need {want} — still waiting"));
                                    thread::sleep(Duration::from_secs(2));
                                    continue;
                                }
                            }
                            on_progress(&format!("device {pid} found"));
                            drop(device);
                            break;
                        }
                    }
                    Err(_) => thread::sleep(Duration::from_millis(500)),
                },
                Err(_) => thread::sleep(Duration::from_millis(500)),
            }
        }

        // Phase 2: flash with retries and verify the reboot. On failure
        // (e.g. the device flapped away mid-flash) go back to waiting.
        on_progress("flashing…");
        match flash_firmware_guarded(bytes, expected_product_id, &on_progress) {
            Ok(()) => {
                on_progress("flash complete, device rebooted into flashed firmware");
                return Ok(());
            }
            Err(e) => {
                on_progress(&format!("flash attempt failed: {e} — waiting for the device again"));
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcu_family_signature_and_flash_base() {
        assert_eq!(McuFamily::from_vid(0x0416), McuFamily::Nuvoton);
        assert_eq!(McuFamily::from_vid(0x0483), McuFamily::Stm);
        assert_eq!(McuFamily::Nuvoton.hid_signature(), *b"HIDC");
        assert_eq!(McuFamily::Stm.hid_signature(), [0x5C, 0xCA, 0x37, 0x75]);
        assert_eq!(McuFamily::Nuvoton.firmware_start_address(), 0);
        assert_eq!(McuFamily::Stm.firmware_start_address(), 0x0800C000);

        // The 18-byte command packet carries the family signature at bytes
        // 10..14 and the sum of bytes 0..14 as a LE i32 checksum at 14..18.
        CURRENT_FAMILY.store(McuFamily::Stm as u8, Ordering::SeqCst);
        let pkt = create_command(CMD_WRITE_DATA, 0x0800C000, 114_688);
        assert_eq!(pkt[0], CMD_WRITE_DATA);
        assert_eq!(&pkt[2..6], &0x0800C000i32.to_le_bytes());
        assert_eq!(&pkt[10..14], &[0x5C, 0xCA, 0x37, 0x75]);
        let sum: i32 = pkt[..14].iter().map(|b| *b as i32).sum();
        assert_eq!(&pkt[14..18], &sum.to_le_bytes());

        CURRENT_FAMILY.store(McuFamily::Nuvoton as u8, Ordering::SeqCst);
        let pkt = create_command(CMD_WRITE_DATA, 0, 114_688);
        assert_eq!(&pkt[10..14], b"HIDC");
        assert_eq!(&pkt[2..6], &0i32.to_le_bytes());
    }
}
