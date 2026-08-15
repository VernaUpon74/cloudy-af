use crate::firmware::encryption::{decrypt, encrypt, EncryptionType};

#[test]
fn test_none_roundtrip() {
    let data = vec![0, 1, 2, 3, 255];
    let (plain, enc) = decrypt(&data).unwrap();
    assert_eq!(plain, data);
    assert!(matches!(enc, EncryptionType::None));
    let out = encrypt(&plain, enc).unwrap();
    assert_eq!(out, data);
}

#[test]
fn test_joyetech_roundtrip() {
    let data = vec![0x55; 64];
    let cipher = encrypt(&data, EncryptionType::Joyetech).unwrap();
    let (plain, enc) = decrypt(&cipher).unwrap();
    assert_eq!(plain, data);
    assert!(matches!(enc, EncryptionType::Joyetech));
}

#[test]
fn test_arcticfox_roundtrip() {
    // Include the firmware marker so the heuristic decryptor recognises the
    // result as an ArcticFox-encrypted payload.
    let marker = b"Joyetech APROM";
    let mut data = vec![0u8; 256];
    data[100..100 + marker.len()].copy_from_slice(marker);

    let cipher = encrypt(&data, EncryptionType::ArcticFox).unwrap();
    let (plain, enc) = decrypt(&cipher).unwrap();
    assert_eq!(plain, data);
    assert!(matches!(enc, EncryptionType::ArcticFox));
}

#[test]
fn test_arcticfox2_roundtrip() {
    let marker = b"Joyetech APROM";
    let mut data = vec![0u8; 256];
    data[50..50 + marker.len()].copy_from_slice(marker);

    let cipher = encrypt(&data, EncryptionType::ArcticFox2).unwrap();
    let (plain, enc) = decrypt(&cipher).unwrap();
    assert_eq!(plain, data);
    assert!(matches!(enc, EncryptionType::ArcticFox2));
}

#[test]
fn test_vandalproof_unsupported() {
    let data = vec![0u8; 16];
    let result = encrypt(&data, EncryptionType::VandalProof);
    assert!(result.is_err());
}
