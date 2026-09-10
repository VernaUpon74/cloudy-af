use crate::firmware::encryption::{decrypt, encrypt, EncryptionType};

#[test]
fn test_none_roundtrip() {
    // Plaintext firmware is recognised by the embedded `Joyetech APROM` marker.
    let marker = b"Joyetech APROM";
    let mut data = vec![0u8; 32];
    data[..5].copy_from_slice(&[0, 1, 2, 3, 255]);
    data[16..16 + marker.len()].copy_from_slice(marker);
    let (plain, enc) = decrypt(&data).unwrap();
    assert_eq!(plain, data);
    assert!(matches!(enc, EncryptionType::None));
    let out = encrypt(&plain, enc).unwrap();
    assert_eq!(out, data);
}

#[test]
fn test_joyetech_roundtrip() {
    let marker = b"Joyetech APROM";
    let mut data = vec![0x55; 64];
    data[32..32 + marker.len()].copy_from_slice(marker);
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
fn test_vandalproof_roundtrip() {
    let marker = b"Joyetech APROM";
    // Exercise padding edge cases: exact block multiple and ragged length.
    for base_len in [128usize, 133] {
        let mut data = vec![0u8; base_len];
        data[16..16 + marker.len()].copy_from_slice(marker);
        let cipher = encrypt(&data, EncryptionType::VandalProof).unwrap();
        assert_eq!(cipher.len() % 16, 0);
        let (plain, enc) = decrypt(&cipher).unwrap();
        assert_eq!(plain, data);
        assert!(matches!(enc, EncryptionType::VandalProof));
    }
}

#[test]
fn test_vandalproof_real_builds() {
    // 2018+ ArcticFox builds ship as VandalProof (AES-128-CBC) packages.
    for name in ["af_180709.bin", "af_180913.bin", "af_190602.bin"] {
        let path = format!("/var/home/j/cloudy-af/AF_fw/{}", name);
        let data = std::fs::read(&path).expect("read VP sample");
        let (plain, enc) = decrypt(&data).unwrap();
        assert!(matches!(enc, EncryptionType::VandalProof), "{}", name);
        assert!(plain
            .windows(b"Joyetech APROM".len())
            .any(|w| w == b"Joyetech APROM"));
        // Cortex-M vector table sanity: initial SP in SRAM, Thumb reset vector
        // inside the image.
        let sp = u32::from_le_bytes(plain[0..4].try_into().unwrap());
        let reset = u32::from_le_bytes(plain[4..8].try_into().unwrap());
        assert!((0x2000_0000..=0x2001_0000).contains(&sp), "{}", name);
        assert_eq!(reset & 1, 1, "{}", name);
        assert!(((reset & !1) as usize) < plain.len(), "{}", name);
    }
}

#[test]
fn test_full_firmware_corpus_decrypts() {
    // Every af_*/rp_* build published on nfeteam.org (stable + nightly +
    // STM32) plus the local AF_fw samples must decrypt with a recognised
    // scheme and contain the firmware marker.
    let roots = [
        "/var/home/j/cloudy-af/AF_fw",
        "/var/home/j/cloudy-af/AF_fw/nfeteam/stable",
        "/var/home/j/cloudy-af/AF_fw/nfeteam/nightly",
        "/var/home/j/cloudy-af/AF_fw/nfeteam/stm32",
    ];
    let mut total = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for root in roots {
        let mut paths: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "bin").unwrap_or(false))
            .collect();
        paths.sort();
        for p in paths {
            total += 1;
            let data = std::fs::read(&p).unwrap();
            match decrypt(&data) {
                Ok((plain, _)) => {
                    let has_marker = plain
                        .windows(b"Joyetech APROM".len())
                        .any(|w| w == b"Joyetech APROM")
                        || plain
                            .windows(b"Joyetech APP".len())
                            .any(|w| w == b"Joyetech APP");
                    if !has_marker {
                        failures.push(format!("{}: no marker", p.display()));
                    }
                }
                Err(e) => failures.push(format!("{}: {}", p.display(), e)),
            }
        }
    }
    assert!(total >= 190, "corpus incomplete: {} files", total);
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
}

#[test]
fn test_arcticfox2_real_build_marker_correction() {
    // Real ArcticFox 170222 build: encrypted with the AF2 parameters
    // (key key 0x19, table length 11) but the build toolchain's PRNG
    // differed slightly from .NET Random, so the decryptor must correct
    // the table using the known plaintext marker.
    let data = std::fs::read("/var/home/j/Desktop/NFE-Tools-v7.1.1/af_170222.bin")
        .expect("read af_170222.bin");
    let (plain, enc) = decrypt(&data).unwrap();
    assert!(matches!(enc, EncryptionType::ArcticFox2));
    assert!(plain
        .windows(b"Joyetech APROM".len())
        .any(|w| w == b"Joyetech APROM"));
}
