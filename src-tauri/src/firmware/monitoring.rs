//! Decode of the 0x66 "ReadMonitoringData" HID report (NToolbox
//! `MonitoringData.cs`, verified against hobbyquaker/arcticfox's parser).
//! Pure functions only — unit-testable without hardware.

use serde::Serialize;

use super::{FirmwareError, Result};

/// Monitoring telemetry payload size in bytes.
pub const MONITORING_DATA_SIZE: usize = 64;

/// One decoded monitoring sample, SI units where the firmware scales.
///
/// Layout (packed little-endian, 64 bytes):
/// u32 @0 Timestamp (seconds x 100), u8 @4 IsFiring, u8 @5 IsCharging,
/// u8 @6 IsCelcius, u8 @7..10 Battery1..4Voltage (0 = absent, V = (raw+275)/100),
/// u16 @11 PowerSet (W x 10), u16 @13 TemperatureSet (raw),
/// u16 @15 Temperature (raw), u16 @17 OutputVoltage (V x 100),
/// u16 @19 OutputCurrent (A x 100), u16 @21 Resistance (ohm x 1000),
/// u16 @23 RealResistance (ohm x 1000), u8 @25 BoardTemperature (deg C).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MonitoringData {
    /// Device uptime in centiseconds.
    pub timestamp: u32,
    pub is_firing: bool,
    pub is_charging: bool,
    /// true = temperatures reported in Celsius, false = Fahrenheit.
    pub is_celsius: bool,
    /// Per-cell battery voltage in volts; 0.0 when the cell is absent (raw 0).
    pub battery: [f32; 4],
    /// Sum of the present cells, in volts.
    pub battery_pack: f32,
    /// Power setpoint in watts.
    pub power_set: f32,
    /// Temperature setpoint as reported by the firmware (raw).
    pub temperature_set: u16,
    /// Coil/board temperature as reported by the firmware (raw).
    pub temperature: u16,
    /// Output voltage in volts.
    pub output_voltage: f32,
    /// Output current in amps.
    pub output_current: f32,
    /// Output power in watts (OutputVoltage x OutputCurrent).
    pub power: f32,
    /// Cold resistance in ohms.
    pub resistance: f32,
    /// Live resistance in ohms.
    pub real_resistance: f32,
    /// Board temperature in degrees Celsius.
    pub board_temperature: u8,
}

/// Decode a 64-byte 0x66 payload into SI-unit fields.
pub fn decode_monitoring_data(buf: &[u8]) -> Result<MonitoringData> {
    if buf.len() < MONITORING_DATA_SIZE {
        return Err(FirmwareError::Other(format!(
            "monitoring payload too short: {} bytes",
            buf.len()
        )));
    }
    let u16le = |o: usize| u16::from_le_bytes([buf[o], buf[o + 1]]);

    let mut battery = [0f32; 4];
    let mut pack = 0f32;
    for (i, cell) in battery.iter_mut().enumerate() {
        let raw = buf[7 + i];
        if raw != 0 {
            *cell = (raw as f32 + 275.0) / 100.0;
            pack += *cell;
        }
    }

    let output_voltage = u16le(17) as f32 / 100.0;
    let output_current = u16le(19) as f32 / 100.0;

    Ok(MonitoringData {
        timestamp: u32::from_le_bytes(buf[0..4].try_into().unwrap()),
        is_firing: buf[4] != 0,
        is_charging: buf[5] != 0,
        is_celsius: buf[6] != 0,
        battery,
        battery_pack: pack,
        power_set: u16le(11) as f32 / 10.0,
        temperature_set: u16le(13),
        temperature: u16le(15),
        output_voltage,
        output_current,
        power: output_voltage * output_current,
        resistance: u16le(21) as f32 / 1000.0,
        real_resistance: u16le(23) as f32 / 1000.0,
        board_temperature: buf[25],
    })
}
