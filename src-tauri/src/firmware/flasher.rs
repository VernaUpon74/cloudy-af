use std::thread;
use std::time::Duration;

use super::{FirmwareError, Result};

const VID: u16 = 0x0416;
const PID: u16 = 0x5020;

const CMD_READ_DATAFLASH: u8 = 0x35;
const CMD_WRITE_DATAFLASH: u8 = 0x53;
const CMD_WRITE_DATA: u8 = 0xC3;
const CMD_RESTART: u8 = 0xB4;
const CMD_SET_LOGO: u8 = 0xA5;

const DATAFLASH_SIZE: usize = 2048;
const REPORT_SIZE: usize = 64;

/// APROM offset of the logo blocks (from NFirmwareEditor HidConnector).
const LOGO_OFFSET: i32 = 102400;
/// Two 512-byte image blocks.
const LOGO_LENGTH: usize = 1024;

/// Connect to the first bootloader HID interface found.
pub fn open_device() -> Result<hidapi::HidDevice> {
    let api = hidapi::HidApi::new().map_err(|e| FirmwareError::Other(e.to_string()))?;
    let device = api
        .open(VID, PID)
        .map_err(|e| FirmwareError::Other(format!("cannot open HID device: {e}")))?;
    Ok(device)
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
        if d.vendor_id() == VID && d.product_id() == PID {
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
    packet[10..14].copy_from_slice(b"HIDC");
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

/// Read the Product ID string from dataflash (4 ASCII chars at offset 312 of
/// the data area, i.e. raw offset 316 including the checksum prefix).
pub fn read_product_id() -> Result<String> {
    let data = read_dataflash()?;
    if data.len() < 320 {
        return Err(FirmwareError::Other("dataflash too short".into()));
    }
    let bytes = &data[316..320];
    String::from_utf8(bytes.to_vec()).map_err(|_| FirmwareError::Other("invalid Product ID".into()))
}

/// Switch the device to LDROM bootloader mode if needed.
pub fn ensure_ldrom_mode() -> Result<()> {
    let mut data = read_dataflash()?;
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
    Err(FirmwareError::Other("device did not re-enumerate in bootloader mode".into()))
}

/// Flash a firmware image to the device, restart it, and verify it boots.
///
/// The image is streamed exactly as provided: encrypted (e.g. VandalProof)
/// images are decrypted by the on-device LDROM updater, plaintext images are
/// accepted as-is. If `expected_product_id` is given, the device dataflash
/// Product ID must match before anything is written.
pub fn flash_firmware_guarded(bytes: &[u8], expected_product_id: Option<&str>) -> Result<()> {
    ensure_ldrom_mode()?;
    let mut device = open_device()?;

    if let Some(want) = expected_product_id {
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

    send_command(&mut device, CMD_WRITE_DATA, 0, bytes.len() as i32)?;

    for chunk in bytes.chunks(REPORT_SIZE) {
        let mut report = vec![0u8; REPORT_SIZE + 1];
        report[1..1 + chunk.len()].copy_from_slice(chunk);
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
    drop(device);

    // Restart and wait for the device to come back in APROM mode.
    restart_device()?;
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
    flash_firmware_guarded(bytes, None)
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

    // Phase 2: flash with retries and verify the reboot.
    on_progress("flashing…");
    flash_firmware_guarded(bytes, expected_product_id)?;
    on_progress("flash complete, device rebooted into flashed firmware");
    Ok(())
}
