use std::thread;
use std::time::Duration;

use super::{FirmwareError, Result};

const VID: u16 = 0x0416;
const PID: u16 = 0x5020;

const CMD_READ_DATAFLASH: u8 = 0x35;
const CMD_WRITE_DATAFLASH: u8 = 0x53;
const CMD_WRITE_DATA: u8 = 0xC3;
const CMD_RESTART: u8 = 0xB4;

const DATAFLASH_SIZE: usize = 2048;
const REPORT_SIZE: usize = 64;

/// Connect to the bootloader HID interface.
pub fn open_device() -> Result<hidapi::HidDevice> {
    let api = hidapi::HidApi::new().map_err(|e| FirmwareError::Other(e.to_string()))?;
    let device = api
        .open(VID, PID)
        .map_err(|e| FirmwareError::Other(format!("cannot open HID device: {e}")))?;
    Ok(device)
}

fn create_command(cmd: u8, arg1: i32, arg2: i32) -> [u8; 15] {
    let mut packet = [0u8; 15];
    packet[0] = cmd;
    packet[1] = 0x0E;
    packet[2..6].copy_from_slice(&arg1.to_le_bytes());
    packet[6..10].copy_from_slice(&arg2.to_le_bytes());
    packet[10..14].copy_from_slice(b"HIDC");
    let sum: u8 = packet[0..14].iter().fold(0u8, |a, b| a.wrapping_add(*b));
    packet[14] = sum;
    packet
}

fn send_command(device: &mut hidapi::HidDevice, cmd: u8, arg1: i32, arg2: i32) -> Result<()> {
    let packet = create_command(cmd, arg1, arg2);
    let mut report = vec![0u8; REPORT_SIZE + 1];
    report[1..1 + packet.len()].copy_from_slice(&packet);
    device.write(&report).map_err(|e| FirmwareError::Other(e.to_string()))?;
    Ok(())
}

fn read_exact(device: &mut hidapi::HidDevice, len: usize) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(len);
    let mut chunk = [0u8; REPORT_SIZE];
    while buf.len() < len {
        let n = device.read(&mut chunk).map_err(|e| FirmwareError::Other(e.to_string()))?;
        if n == 0 {
            return Err(FirmwareError::Other("HID read returned 0 bytes".into()));
        }
        // First byte is report ID; skip it.
        let data = &chunk[1..n];
        buf.extend_from_slice(data);
    }
    buf.truncate(len);
    Ok(buf)
}

/// Read the device dataflash (2048 bytes: 4-byte checksum + 2044 bytes data).
pub fn read_dataflash() -> Result<Vec<u8>> {
    let mut device = open_device()?;
    send_command(&mut device, CMD_READ_DATAFLASH, 0, 0)?;
    let data = read_exact(&mut device, DATAFLASH_SIZE)?;
    Ok(data)
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

    send_command(&mut device, CMD_WRITE_DATAFLASH, 0, 0)?;

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

/// Read the Product ID string from dataflash (4 ASCII chars at offset 312).
pub fn read_product_id() -> Result<String> {
    let data = read_dataflash()?;
    if data.len() < 316 {
        return Err(FirmwareError::Other("dataflash too short".into()));
    }
    let bytes = &data[312..316];
    String::from_utf8(bytes.to_vec()).map_err(|_| FirmwareError::Other("invalid Product ID".into()))
}

/// Switch the device to LDROM bootloader mode if needed.
pub fn ensure_ldrom_mode() -> Result<()> {
    let mut data = read_dataflash()?;
    if data.len() < 13 {
        return Err(FirmwareError::Other("dataflash too short".into()));
    }
    if data[9] == 1 {
        return Ok(());
    }

    // Set boot flag and write back.
    let mut user_data = [0u8; 2044];
    user_data.copy_from_slice(&data[4..]);
    user_data[9] = 1;
    write_dataflash(&user_data)?;
    restart_device()?;

    // Wait for re-enumeration.
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(500));
        if open_device().is_ok() {
            return Ok(());
        }
    }
    Err(FirmwareError::Other("device did not re-enumerate in bootloader mode".into()))
}

/// Flash a decrypted firmware image to the device.
pub fn flash_firmware(bytes: &[u8]) -> Result<()> {
    ensure_ldrom_mode()?;
    let mut device = open_device()?;

    send_command(&mut device, CMD_WRITE_DATA, 0, bytes.len() as i32)?;

    for chunk in bytes.chunks(REPORT_SIZE) {
        let mut report = vec![0u8; REPORT_SIZE + 1];
        report[1..1 + chunk.len()].copy_from_slice(chunk);
        device.write(&report).map_err(|e| FirmwareError::Other(e.to_string()))?;
    }

    Ok(())
}
