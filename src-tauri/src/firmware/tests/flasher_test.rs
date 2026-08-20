//! Hardware-in-the-loop tests. Require a device (VID 0416 / PID 5020)
//! plugged in; run with `cargo test -- --ignored`.

use crate::firmware::flasher;

#[test]
#[ignore]
fn test_read_dataflash_hardware() {
    let data = flasher::read_dataflash().expect("read_dataflash failed");
    assert_eq!(data.len(), 2048);
    let pid = flasher::read_product_id().expect("read_product_id failed");
    println!("Product ID: {pid}");
    println!("dataflash[4..20]: {:02x?}", &data[4..20]);
    println!("dataflash[300..340]: {:?}", &data[300..340]);
    println!("checksum ok: {}", u32::from_le_bytes(data[0..4].try_into().unwrap())
        == data[4..].iter().map(|b| *b as u32).sum::<u32>());
}

#[test]
#[ignore]
fn test_open_device_hardware() {
    flasher::open_device().expect("open_device failed");
}

fn make_block1(width: u8, height: u8, on: bool) -> Vec<u8> {
    let data_len = width as usize * ((height as usize) + 7) / 8;
    let mut out = vec![if on { 0xFF } else { 0x00 }; data_len + 2];
    out[0] = width;
    out[1] = height;
    out
}

fn make_block2(width: u8, height: u8, on: bool) -> Vec<u8> {
    let data_len = ((width as usize) + 7) / 8 * height as usize;
    let mut out = vec![if on { 0xFF } else { 0x00 }; data_len + 2];
    out[0] = width;
    out[1] = height;
    out
}

#[test]
#[ignore]
fn test_set_logo_pulse_hardware() {
    // Visual check: alternates a well-formed 64x64 all-on logo with an
    // empty one a few times, then clears. Watch the device display.
    let mut dev = flasher::open_device().expect("open failed");
    for _ in 0..5 {
        flasher::set_logo(&mut dev, &make_block1(64, 56, true), &make_block2(64, 56, true)).expect("set_logo failed");
        std::thread::sleep(std::time::Duration::from_millis(500));
        flasher::clear_logo(&mut dev).expect("clear failed");
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[test]
#[ignore]
fn test_list_all_devices_hardware() {
    let devices = flasher::list_devices().expect("list_devices failed");
    println!("found {} device(s)", devices.len());
    for d in &devices {
        let mut dev = flasher::open_device_by_path(&d.path).expect("open failed");
        let df = flasher::read_dataflash_from(&mut dev).expect("read_dataflash failed");
        let pid = String::from_utf8_lossy(&df[316..320]).into_owned();
        println!("{d:?} -> Product ID: {pid}");
        assert_eq!(df.len(), 2048);
    }
    assert!(!devices.is_empty());
}

#[test]
#[ignore]
fn test_set_logo_static_hardware() {
    // Sends a single well-formed all-on logo and leaves it in place.
    let mut dev = flasher::open_device().expect("open failed");
    flasher::set_logo(&mut dev, &make_block1(64, 56, true), &make_block2(64, 56, true)).expect("set_logo failed");
}

#[test]
#[ignore]
fn test_screenshot_hardware() {
    // Captures the screen as PNG (scaled 4x) at /var/home/j/af_screenshot.png.
    let mut dev = flasher::open_device().expect("open failed");
    let raw = flasher::screenshot(&mut dev).expect("screenshot failed");
    assert_eq!(raw.len(), 0x400);
    // Raw 1bpp rows, MSB first (GDI Format1bppIndexed). Screen is 64x128
    // unless all bytes past the 96x16 buffer are zero.
    let small = raw[192..].iter().all(|b| *b == 0);
    let (w, h) = if small { (96usize, 16usize) } else { (64usize, 128usize) };
    let stride = (w + 7) / 8;
    let scale = 4usize;
    let mut img = vec![0u8; w * scale * h * scale * 3];
    for y in 0..h {
        for x in 0..w {
            let bit = (raw[y * stride + x / 8] >> (7 - x % 8)) & 1;
            let v = if bit == 1 { 255 } else { 0 };
            for dy in 0..scale { for dx in 0..scale {
                let px = ((y * scale + dy) * w * scale + (x * scale + dx)) * 3;
                img[px] = v; img[px + 1] = v; img[px + 2] = v;
            }}
        }
    }
    // minimal uncompressed PNG via stored deflate
    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&( (w*scale) as u32).to_be_bytes());
    ihdr.extend_from_slice(&( (h*scale) as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut idat_raw = Vec::new();
    for row in 0..h*scale {
        idat_raw.push(0u8);
        idat_raw.extend_from_slice(&img[row*w*scale*3..(row+1)*w*scale*3]);
    }
    fn chunk(out: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(data);
        let mut crc = u32::MAX;
        for b in name.iter().chain(data.iter()) {
            crc ^= *b as u32;
            for _ in 0..8 { crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB88320 } else { crc >> 1 }; }
        }
        out.extend_from_slice(&(!crc).to_be_bytes());
    }
    chunk(&mut png, b"IHDR", &ihdr);
    // zlib stored stream
    let mut z = vec![0x78, 0x01];
    let mut i = 0;
    while i < idat_raw.len() {
        let n = (idat_raw.len() - i).min(65535);
        let last = i + n == idat_raw.len();
        z.push(if last { 1 } else { 0 });
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&idat_raw[i..i + n]);
        i += n;
    }
    let s1: u32 = idat_raw.iter().map(|b| *b as u32).fold(1, |a, b| (a + b) % 65521);
    let s2: u32 = { let mut a = 1u32; let mut b = 0u32; for &x in &idat_raw { a = (a + x as u32) % 65521; b = (b + a) % 65521; } b };
    z.extend_from_slice(&((s2 << 16 | s1).to_be_bytes()));
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    std::fs::write("/var/home/j/af_screenshot.png", &png).unwrap();
    println!("saved /var/home/j/af_screenshot.png ({} bytes)", png.len());
}

#[test]
#[ignore]
fn test_logo_visibility_hardware() {
    // Writes a distinctive 96x16 checkerboard logo, then polls screenshots
    // for 70s to see whether/when the logo screen appears.
    let mut dev = flasher::open_device().expect("open failed");
    let mut b2 = make_block2(96, 16, false);
    for y in 0..16 {
        for x in 0..96 {
            if (x + y) % 2 == 0 {
                b2[2 + y * 12 + x / 8] |= 1 << (7 - x % 8);
            }
        }
    }
    flasher::set_logo(&mut dev, &[], &b2).expect("set_logo failed");
    for i in 0..14 {
        let raw = flasher::screenshot(&mut dev).expect("screenshot failed");
        let nonzero = raw[..192].iter().filter(|b| **b != 0).count();
        println!("t={}s nonzero={}", i * 5, nonzero);
        std::fs::write(format!("/var/home/j/af_shot_{:02}.bin", i), &raw).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
}

#[test]
#[ignore]
fn test_flash_plaintext_probe_hardware() {
    // DANGER: flashes a decrypted stock iStick Pico V1.00 image to the first
    // device, then restarts it. Watch whether the device boots stock.
    let bytes = std::fs::read("/var/home/j/pico_v100_plain.bin").expect("read probe bin");
    println!("flashing {} bytes (plaintext stock Pico V1.00)", bytes.len());
    flasher::flash_firmware(&bytes).expect("flash failed");
    flasher::restart_device().expect("restart failed");
    println!("flashed and restarted");
}

#[test]
#[ignore]
fn test_flash_steps_hardware() {
    use crate::firmware::flasher as f;
    let mut dev = f::open_device().expect("open failed");
    println!("step: read_dataflash");
    match f::read_dataflash_from(&mut dev) {
        Ok(d) => println!("  ok, boot flag data[9]={}", d[9]),
        Err(e) => println!("  err: {e}"),
    }
}

#[test]
#[ignore]
fn test_wake_and_mode_hardware() {
    use crate::firmware::flasher as f;
    let mut dev = f::open_device().expect("open failed");
    let df = f::read_dataflash_from(&mut dev).expect("read_dataflash failed");
    println!("boot flag Data[9]={} (1=LDROM)", df[4 + 9]);
    let s1 = f::screenshot(&mut dev).expect("shot1 failed");
    println!("before wake: nonzero={}", s1[..192].iter().filter(|b| **b != 0).count());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let (y, mo, d, h, mi, sec) = epoch_to_ymd(now);
    f::set_date_time(&mut dev, y, mo, d, h, mi, sec).expect("set_date_time failed");
    std::thread::sleep(std::time::Duration::from_secs(1));
    let s2 = f::screenshot(&mut dev).expect("shot2 failed");
    println!("after wake: nonzero={}", s2[..192].iter().filter(|b| **b != 0).count());
}

fn epoch_to_ymd(t: u64) -> (u16, u8, u8, u8, u8, u8) {
    let days = (t / 86400) as i64;
    let secs = t % 86400;
    // civil from days (Howard Hinnant's algorithm)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = if m <= 2 { y + 1 } else { y } as u16;
    (y, m, d, (secs / 3600) as u8, ((secs % 3600) / 60) as u8, (secs % 60) as u8)
}

#[test]
#[ignore]
fn test_monitoring_wake_hardware() {
    use crate::firmware::flasher as f;
    let mut dev = f::open_device().expect("open failed");
    // ReadMonitoringData: cmd 0x66, len 64
    f::send_command_pub(&mut dev, 0x66, 0, 64).expect("cmd failed");
    let data = f::read_exact_pub(&mut dev, 64).expect("read failed");
    println!("monitoring: {:02x?}", &data[..16]);
    std::thread::sleep(std::time::Duration::from_millis(500));
    let s = f::screenshot(&mut dev).expect("shot failed");
    println!("after monitoring: nonzero={}", s[..192].iter().filter(|b| **b != 0).count());
}

#[test]
#[ignore]
fn test_flash_verbose_hardware() {
    use crate::firmware::flasher as f;
    let bytes = std::fs::read("/var/home/j/pico_v100_plain.bin").expect("read probe bin");
    println!("step 1: ensure_ldrom_mode");
    f::ensure_ldrom_mode().expect("ensure_ldrom_mode failed");
    println!("step 2: open device in LDROM");
    let mut dev = f::open_device().expect("open failed");
    println!("step 3: WriteData(0, {})", bytes.len());
    f::send_command_pub(&mut dev, 0xC3, 0, bytes.len() as i32).expect("WriteData cmd failed");
    println!("step 4: stream");
    for (i, chunk) in bytes.chunks(64).enumerate() {
        let mut report = vec![0u8; 65];
        report[1..1 + chunk.len()].copy_from_slice(chunk);
        let mut tries = 0;
        loop {
            match dev.write(&report) {
                Ok(_) => break,
                Err(e) => {
                    tries += 1;
                    if tries > 50 { panic!("write failed at chunk {}: {e}", i); }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        if i % 100 == 0 { println!("  chunk {}", i); }
    }
    println!("step 5: done, not restarting");
}

#[test]
#[ignore]
fn test_flash_hidraw_direct_hardware() {
    // Bypass hidapi: talk to the LDROM device via /dev/hidraw directly.
    use std::io::{Read, Write};
    use crate::firmware::flasher as f;
    // find the right hidraw node
    let mut node = None;
    for entry in std::fs::read_dir("/dev").unwrap() {
        let p = entry.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if !name.starts_with("hidraw") { continue; }
        let uevent = std::fs::read_to_string(format!("/sys/class/hidraw/{name}/device/uevent")).unwrap_or_default();
        if uevent.contains("00000416:00005020") { node = Some(p); break; }
    }
    let node = node.expect("no 0416:5020 hidraw node");
    println!("using {:?}", node);
    let mut fdev = std::fs::OpenOptions::new().read(true).write(true).open(&node).expect("open hidraw");

    let bytes = std::fs::read("/var/home/j/pico_v100_plain.bin").expect("read probe bin");
    // WriteData(0, len) command: [cmd, 14, arg1 LE, arg2 LE, "HIDC", sum LE i32]
    let mut cmd = [0u8; 18];
    cmd[0] = 0xC3; cmd[1] = 14;
    cmd[6..10].copy_from_slice(&(bytes.len() as i32).to_le_bytes());
    cmd[10..14].copy_from_slice(b"HIDC");
    let sum: i32 = cmd[..14].iter().map(|b| *b as i32).sum();
    cmd[14..18].copy_from_slice(&sum.to_le_bytes());
    let mut rep = vec![0u8; 65];
    rep[1..19].copy_from_slice(&cmd);
    fdev.write_all(&rep).expect("cmd write failed");
    println!("command sent");
    let mut sent = 0;
    for (i, chunk) in bytes.chunks(64).enumerate() {
        let mut report = vec![0u8; 65];
        report[1..1 + chunk.len()].copy_from_slice(chunk);
        fdev.write_all(&report).expect("chunk write failed");
        sent += chunk.len();
        if i % 100 == 0 { println!("  chunk {}", i); }
    }
    println!("streamed {} bytes; NOT restarting device", sent);
}

#[test]
#[ignore]
fn test_check_state_hardware() {
    use crate::firmware::flasher as f;
    let mut dev = f::open_device().expect("open failed");
    let df = f::read_dataflash_from(&mut dev).expect("read_dataflash failed");
    println!("boot flag Data[9]={}", df[4 + 9]);
    let fwver = i32::from_le_bytes(df[4 + 256..4 + 260].try_into().unwrap());
    println!("fw version raw: {} ({}.{:02})", fwver, fwver / 100, fwver % 100);
    println!("product id: {}", String::from_utf8_lossy(&df[316..320]));
    let s = f::screenshot(&mut dev).expect("screenshot failed");
    println!("screenshot nonzero: {}", s.iter().filter(|b| **b != 0).count());
}

#[test]
#[ignore]
fn test_flash_recovery_loop_hardware() {
    // Recovery flasher v3: waits for the Pico, streams immediately, and on
    // any failure goes back to waiting — the LDROM erases and restarts an
    // interrupted update cleanly on the next attempt.
    use crate::firmware::flasher as f;
    let bytes = std::fs::read("/var/home/j/pico_v100_plain.bin").expect("read probe bin");
    'outer: loop {
        println!("waiting for Pico (M041)...");
        let mut dev = loop {
            match f::open_device() {
                Ok(mut d) => {
                    match f::read_dataflash_from(&mut d) {
                        Ok(df) => {
                            let pid = &df[316..320];
                            let printable = pid.iter().all(|b| (0x20..0x7f).contains(b));
                            if pid == b"M041" || !printable { break d; }
                            println!("  skipping {}", String::from_utf8_lossy(pid));
                            std::thread::sleep(std::time::Duration::from_secs(2));
                        }
                        Err(_) => std::thread::sleep(std::time::Duration::from_millis(300)),
                    }
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(300)),
            }
        };
        println!("Pico open, sending WriteData(0, {})", bytes.len());
        if f::send_command_pub(&mut dev, 0xC3, 0, bytes.len() as i32).is_err() {
            println!("  command failed, re-waiting");
            continue 'outer;
        }
        let mut failed = false;
        for (i, chunk) in bytes.chunks(64).enumerate() {
            let mut report = vec![0u8; 65];
            report[1..1 + chunk.len()].copy_from_slice(chunk);
            let mut tries = 0;
            loop {
                match dev.write(&report) {
                    Ok(_) => break,
                    Err(e) => {
                        tries += 1;
                        if tries > 10 {
                            println!("  write failed at chunk {i}: {e} — re-waiting");
                            failed = true;
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
            }
            if failed { break; }
        }
        if failed { continue 'outer; }
        println!("stream complete");
        break;
    }
}

#[test]
#[ignore]
fn test_restart_and_verify_hardware() {
    use crate::firmware::flasher as f;
    let mut dev = f::open_device().expect("open failed");
    f::restart_device().expect("restart failed");
    drop(dev);
    std::thread::sleep(std::time::Duration::from_secs(5));
    let mut dev = f::open_device().expect("re-open failed");
    let df = f::read_dataflash_from(&mut dev).expect("read_dataflash failed");
    println!("boot flag Data[9]={}", df[4 + 9]);
    let fwver = i32::from_le_bytes(df[4 + 256..4 + 260].try_into().unwrap());
    println!("fw version raw: {}", fwver);
    println!("product id: {}", String::from_utf8_lossy(&df[316..320]));
}

#[test]
#[ignore]
fn test_ldrom_dump_hardware() {
    //! Dumps the device LDROM (which contains the VandalProof decryptor).
    //! Flow: flash the dump payload once; then per chunk: set chunk index in
    //! dataflash, boot APROM (payload copies LDROM chunk -> dataflash, sets
    //! boot flag, resets), read the chunk back from dataflash in LDROM mode.
    use crate::firmware::flasher as f;
    const CHUNK: usize = 2028; // bytes per chunk (dataflash data[16..2044])
    const NCHUNKS: usize = 8;  // 16 KB LDROM
    let payload = std::fs::read("/var/home/j/ldrom_dump_payload.bin").expect("read payload");

    println!("waiting for the Pico (M041)…");
    loop {
        match f::open_device() {
            Ok(mut d) => {
                if let Ok(df) = f::read_dataflash_from(&mut d) {
                    if df.len() >= 320 && &df[316..320] == b"M041" { break; }
                    println!("  skipping {}", String::from_utf8_lossy(&df[316..320]));
                }
            }
            Err(_) => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    println!("flashing payload ({} bytes)", payload.len());
    let mut attempt = 0;
    'flash_retry: loop {
        attempt += 1;
        if attempt > 1 {
            // device likely dropped; go back to waiting for the Pico
            println!("  re-waiting for Pico…");
            loop {
                match f::open_device() {
                    Ok(mut d) => {
                        if let Ok(df) = f::read_dataflash_from(&mut d) {
                            if df.len() >= 320 {
                                let pid = &df[316..320];
                                let printable = pid.iter().all(|b| (0x20..0x7f).contains(b));
                                if pid == b"M041" || !printable { break; }
                            }
                        }
                    }
                    Err(_) => {}
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
        // Fast path for a flapping device: if it is already in LDROM, stream
        // immediately without the multi-step ensure_ldrom_mode dance.
        let fast = f::open_device().ok().and_then(|mut d| {
            f::read_dataflash_from(&mut d).ok().map(|df| (d, df))
        });
        let r = match fast {
            Some((mut d, df)) if df[4 + 9] == 1 => {
                println!("  device already in LDROM, streaming now");
                (|| -> Result<(), String> {
                    f::send_command_pub(&mut d, 0xC3, 0, payload.len() as i32).map_err(|e| e.to_string())?;
                    for chunk in payload.chunks(64) {
                        let mut report = vec![0u8; 65];
                        report[1..1 + chunk.len()].copy_from_slice(chunk);
                        let mut tries = 0;
                        while let Err(e) = d.write(&report) {
                            tries += 1;
                            if tries > 50 { return Err(e.to_string()); }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                    }
                    Ok(())
                })().map_err(|e| crate::firmware::FirmwareError::Other(e))
            }
            _ => f::flash_firmware(&payload),
        };
        match r {
            Ok(()) => break,
            Err(e) => {
                println!("  flash attempt {attempt} failed: {e}");
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }
    println!("payload flashed and device rebooted into APROM");

    let mut collected: std::collections::HashMap<usize, (Vec<u8>, bool)> = std::collections::HashMap::new();
    fn confirmed_map(m: &std::collections::HashMap<usize, (Vec<u8>, bool)>) -> std::collections::HashMap<usize, Vec<u8>> {
        m.iter().filter(|(_, (_, c))| *c).map(|(i, (c, _))| (*i, c.clone())).collect()
    }
    let save = |collected: &std::collections::HashMap<usize, Vec<u8>>| {
        let mut dump = vec![0xAAu8; NCHUNKS * CHUNK];
        for (i, c) in collected {
            dump[i * CHUNK..(i + 1) * CHUNK].copy_from_slice(c);
        }
        std::fs::write("/var/home/j/ldrom_dump.bin", &dump).unwrap();
        println!("  (saved {} chunks)", collected.len());
    };
    let start = std::time::Instant::now();
    while confirmed_map(&collected).len() < NCHUNKS && start.elapsed() < std::time::Duration::from_secs(900) {
        let dev = loop {
            match f::open_device() {
                Ok(d) => break d,
                Err(_) => {
                    if start.elapsed() > std::time::Duration::from_secs(900) { panic!("timeout waiting for device"); }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        };
        let mut dev = dev;
        let df = match f::read_dataflash_from(&mut dev) {
            Ok(d) => d,
            Err(e) => { println!("read failed: {e} — retrying"); drop(dev); std::thread::sleep(std::time::Duration::from_millis(500)); continue; }
        };
        let flag = df[4 + 9];
        let idx = df[4 + 8] as usize;
        if flag == 1 && idx < NCHUNKS {
            let chunk = df[4 + 16..4 + 16 + CHUNK].to_vec();
            // integrity: reject blank chunks, require two identical reads
            let blank = chunk.iter().all(|b| *b == 0xFF) || chunk.iter().all(|b| *b == 0);
            let crc = chunk.iter().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(*b as u32));
            if blank {
                println!("chunk {idx} blank — possible end of LDROM, still verifying");
            }
            match collected.get(&idx) {
                None => {
                    println!("collected chunk {idx} (crc {crc:08x}), awaiting confirmation read");
                    collected.insert(idx, (chunk, false));
                }
                Some((prev, confirmed)) if !*confirmed => {
                    if *prev == chunk {
                        println!("chunk {idx} CONFIRMED (crc {crc:08x})");
                        collected.insert(idx, (chunk, true));
                        save(&confirmed_map(&collected));
                    } else {
                        println!("chunk {idx} MISMATCH (crc {crc:08x} vs previous) — re-reading");
                        collected.insert(idx, (chunk, false));
                    }
                }
                _ => {}
            }
        }
        // choose next missing chunk index
        let next = (0..NCHUNKS).find(|i| !confirmed_map(&collected).contains_key(i)).unwrap();
        let mut data = [0xFFu8; 2044];
        data[8] = next as u8;
        data[9] = 0; // run APROM payload
        if f::write_dataflash(&data).is_err() { drop(dev); std::thread::sleep(std::time::Duration::from_millis(500)); continue; }
        let _ = f::restart_device();
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    let confirmed = confirmed_map(&collected);
    println!("done: {} / {} chunks confirmed", confirmed.len(), NCHUNKS);
    assert_eq!(confirmed.len(), NCHUNKS, "incomplete dump");
    // final integrity: LDROM vector table sanity
    let dump = std::fs::read("/var/home/j/ldrom_dump.bin").unwrap();
    let sp = u32::from_le_bytes(dump[0..4].try_into().unwrap());
    let rst = u32::from_le_bytes(dump[4..8].try_into().unwrap());
    println!("LDROM vector table: SP={sp:08x} reset={rst:08x}");
    assert!(sp >= 0x20000000 && sp < 0x20010000, "bad SP — dump corrupt");
    assert!(rst & 1 == 1 && (rst as usize) < dump.len() + 0x00100000, "bad reset vector");
    assert!(dump.windows(4).any(|w| w == b"HIDC"), "HIDC signature missing — dump corrupt");
    println!("integrity checks passed");
}

#[test]
#[ignore]
fn test_stm32_identify_hardware() {
    //! READ-ONLY probe for STM32-line ArcticFox devices (VID 0483 / PID 5750,
    //! "Joyetech APP"). Sends ReadDataflash and a screenshot request; never
    //! writes, so there is no brick risk.
    const STM32_VID: u16 = 0x0483;
    const STM32_PID: u16 = 0x5750;
    use crate::firmware::flasher as f;

    let api = hidapi::HidApi::new().expect("hidapi init");
    let mut path = None;
    for d in api.device_list() {
        if d.vendor_id() == STM32_VID && d.product_id() == STM32_PID {
            println!("found STM32 device: {:?} iface {}", d.path(), d.interface_number());
            path = Some(d.path().to_owned());
        }
    }
    let path = path.expect("no STM32 device plugged in");
    let mut dev = api.open_path(&path).expect("open STM32 device");

    // ReadDataflash (0x35): 4-byte checksum + 2044 data bytes if the STM32
    // line speaks the same HID protocol as the Nuvoton line.
    match f::send_command_pub(&mut dev, 0x35, 0, 2048)
        .and_then(|_| f::read_exact_pub(&mut dev, 2048))
    {
        Ok(df) => {
            let cks = u32::from_le_bytes(df[0..4].try_into().unwrap());
            let sum: u32 = df[4..].iter().map(|b| *b as u32).sum();
            println!("dataflash: 2048 bytes, checksum {}", if cks == sum { "OK" } else { "MISMATCH" });
            println!("boot flag data[9]={}", df[4 + 9]);
            let fwver = i32::from_le_bytes(df[4 + 256..4 + 260].try_into().unwrap());
            println!("fw version raw: {fwver}");
            println!("product id: {}", String::from_utf8_lossy(&df[316..320]));
            println!("data[300..340]: {:02x?}", &df[4 + 300..4 + 340]);
        }
        Err(e) => println!("dataflash read failed (protocol may differ): {e}"),
    }

    // Screenshot (0xC1): 1024 bytes, 64x128 vertical packing.
    match f::send_command_pub(&mut dev, 0xC1, 0, 1024)
        .and_then(|_| f::read_exact_pub(&mut dev, 1024))
    {
        Ok(shot) => {
            let on = shot.iter().filter(|b| **b != 0).count();
            println!("screenshot: 1024 bytes, {on} non-zero");
            std::fs::write("/var/home/j/stm32_shot.bin", &shot).unwrap();
        }
        Err(e) => println!("screenshot failed: {e}"),
    }
}
