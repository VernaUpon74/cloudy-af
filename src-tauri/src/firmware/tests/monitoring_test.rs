//! Golden-value decode tests for the 0x66 monitoring payload (no hardware).

use crate::firmware::monitoring::{decode_monitoring_data, MONITORING_DATA_SIZE};

/// Build the synthetic 64-byte payload documented in `monitoring.rs`.
fn synthetic_payload() -> Vec<u8> {
    let mut buf = vec![0u8; MONITORING_DATA_SIZE];
    buf[0..4].copy_from_slice(&1_234_567u32.to_le_bytes()); // Timestamp
    buf[4] = 1; // IsFiring
    buf[5] = 0; // IsCharging
    buf[6] = 1; // IsCelcius
    buf[7] = 145; // Battery1 -> (145+275)/100 = 4.20 V
    buf[8] = 150; // Battery2 -> 4.25 V
    buf[9] = 0; // Battery3 absent
    buf[10] = 0; // Battery4 absent
    buf[11..13].copy_from_slice(&750u16.to_le_bytes()); // PowerSet -> 75.0 W
    buf[13..15].copy_from_slice(&220u16.to_le_bytes()); // TemperatureSet
    buf[15..17].copy_from_slice(&210u16.to_le_bytes()); // Temperature
    buf[17..19].copy_from_slice(&420u16.to_le_bytes()); // OutputVoltage -> 4.20 V
    buf[19..21].copy_from_slice(&150u16.to_le_bytes()); // OutputCurrent -> 1.50 A
    buf[21..23].copy_from_slice(&250u16.to_le_bytes()); // Resistance -> 0.250 ohm
    buf[23..25].copy_from_slice(&235u16.to_le_bytes()); // RealResistance -> 0.235 ohm
    buf[25] = 32; // BoardTemperature
    buf
}

#[test]
fn test_decode_golden_values() {
    let data = decode_monitoring_data(&synthetic_payload()).unwrap();

    assert_eq!(data.timestamp, 1_234_567);
    assert!(data.is_firing);
    assert!(!data.is_charging);
    assert!(data.is_celsius);

    assert!((data.battery[0] - 4.20).abs() < 1e-6);
    assert!((data.battery[1] - 4.25).abs() < 1e-6);
    assert_eq!(data.battery[2], 0.0);
    assert_eq!(data.battery[3], 0.0);
    assert!((data.battery_pack - 8.45).abs() < 1e-6);

    assert!((data.power_set - 75.0).abs() < 1e-6);
    assert_eq!(data.temperature_set, 220);
    assert_eq!(data.temperature, 210);
    assert!((data.output_voltage - 4.20).abs() < 1e-6);
    assert!((data.output_current - 1.50).abs() < 1e-6);
    assert!((data.power - 6.30).abs() < 1e-6);
    assert!((data.resistance - 0.250).abs() < 1e-6);
    assert!((data.real_resistance - 0.235).abs() < 1e-6);
    assert_eq!(data.board_temperature, 32);
}

#[test]
fn test_decode_short_buffer_rejected() {
    let buf = vec![0u8; MONITORING_DATA_SIZE - 1];
    assert!(decode_monitoring_data(&buf).is_err());
}
