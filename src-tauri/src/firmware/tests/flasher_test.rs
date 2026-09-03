//! Hardware-in-the-loop tests. Require a device (VID 0416 / PID 5020)
//! plugged in; run with `cargo test -- --ignored`.

use crate::firmware::flasher;

/// Absolute path to a file under `<repo>/test-fixtures/` (device-specific
/// hardware-test binaries, not committed).
fn fixture(rel: &str) -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../test-fixtures/").to_string() + rel
}

/// Path for test-produced dumps under `<repo>/test-fixtures/out/` (created
/// on demand).
fn out(rel: impl AsRef<str>) -> String {
    let dir = fixture("out");
    std::fs::create_dir_all(&dir).unwrap();
    format!("{dir}/{}", rel.as_ref())
}

#[test]
fn test_parse_fw_version() {
    let mut df = vec![0u8; 2048];
    df[4 + 256..4 + 260].copy_from_slice(&210103i32.to_le_bytes());
    assert_eq!(flasher::parse_fw_version(&df).unwrap(), 210103);
}

#[test]
#[ignore]
fn test_download_stock_hardware() {
    let df = crate::firmware::flasher::read_dataflash().unwrap();
    let pid = String::from_utf8_lossy(&df[316..320]).trim_matches(char::from(0)).trim().to_string();
    let ver = crate::firmware::flasher::parse_fw_version(&df).unwrap();
    println!("pid={pid} ver={ver}");
    assert_eq!(pid, "M041");
}

#[test]
#[ignore]
fn test_backup_dataflash_hardware() {
    //! Saves the raw dataflash (settings) to BACKUP_OUT (default
    //! test-fixtures/rescue/dataflash_backup.bin) before firmware surgery.
    let data = flasher::read_dataflash().expect("read_dataflash failed");
    assert_eq!(data.len(), 2048);
    let out = std::env::var("BACKUP_OUT").unwrap_or_else(|_| {
        fixture("rescue/dataflash_backup.bin")
    });
    std::fs::write(&out, &data).expect("write backup");
    println!("saved {} bytes to {out}", data.len());
    println!("fw version: {}, product: {}",
        flasher::parse_fw_version(&data).map(|v| v.to_string()).unwrap_or_else(|_| "?".into()),
        String::from_utf8_lossy(&data[316..320]));
}

#[test]
#[ignore]
fn test_restore_dataflash_hardware() {
    //! Restores a raw 2048-byte dataflash backup (4-byte checksum prefix +
    //! 2044 data, as produced by test_backup_dataflash_hardware) onto the
    //! device. Waits out boot-loop windows: retries open+write until the
    //! device holds still long enough. RESTORE_IN defaults to the stock
    //! v1.00 rescue backup.
    use crate::firmware::flasher as f;
    let path = std::env::var("RESTORE_IN").unwrap_or_else(|_| {
        fixture("rescue/dataflash_stock_v1.00.bin")
    });
    let raw = std::fs::read(&path).expect("read backup");
    assert_eq!(raw.len(), 2048);
    let mut user = [0u8; 2044];
    user.copy_from_slice(&raw[4..]);
    // sanity: the backup's own checksum must match its data
    let cks = u32::from_le_bytes(raw[0..4].try_into().unwrap());
    let sum: u32 = user.iter().map(|b| *b as u32).sum();
    assert_eq!(cks, sum, "backup file checksum mismatch");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        match f::write_dataflash(&user) {
            Ok(()) => { println!("dataflash restored from {path}"); break; }
            Err(e) => {
                if std::time::Instant::now() > deadline {
                    panic!("restore failed for 300s: {e}");
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        }
    }
    // verify: wait for a window and read back
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        if let Ok(df) = f::read_dataflash() {
            let ver = f::parse_fw_version(&df).unwrap_or(-1);
            println!("readback: version={ver} product={}", String::from_utf8_lossy(&df[316..320]));
            break;
        }
        if std::time::Instant::now() > deadline { panic!("no readback window"); }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

#[test]
#[ignore]
fn test_read_dataflash_hardware() {
    let data = flasher::read_dataflash().expect("read_dataflash failed");
    assert_eq!(data.len(), 2048);
    let pid = flasher::read_product_id().expect("read_product_id failed");
    println!("Product ID: {pid}");
    println!("dataflash[4..20]: {:02x?}", &data[4..20]);
    println!("dataflash[300..340]: {:?}", &data[300..340]);
    println!("page2 [0x400..0x420]: {:02x?}", &data[4 + 0x400..4 + 0x420]);
    println!("checksum ok: {}", u32::from_le_bytes(data[0..4].try_into().unwrap())
        == data[4..].iter().map(|b| *b as u32).sum::<u32>());
}

#[test]
#[ignore]
fn test_flash_patched_timed_hardware() {
    //! Full-cycle patched-image experiment: enter LDROM via the normal
    //! ensure_ldrom_mode path, stream PATCHED_IMAGE (or default patched
    //! build), 0xB4 back to APROM, then measure time-to-first-open — the
    //! v0 delay stub shows up as +10 s here, proving the image landed AND
    //! the stub executed.
    use crate::firmware::flasher as f;
    let bytes = std::fs::read(std::env::var("PATCHED_IMAGE")
        .unwrap_or_else(|_| fixture("pico_patched.bin")))
        .expect("read patched image");
    let mut d;
    'flash: for attempt in 1..=5 {
        if std::env::var("SKIP_ENSURE").is_err() {
            if f::ensure_ldrom_mode().is_err() { continue; }
        }
        println!("in LDROM; flashing {} bytes (attempt {attempt})", bytes.len());
        d = f::open_device().expect("open failed");
        if f::send_command_pub(&mut d, 0xC3, 0, bytes.len() as i32).is_err() {
            drop(d);
            continue;
        }
        let mut ok = true;
        for (ci, chunk) in bytes.chunks(64).enumerate() {
            if ci % 100 == 0 { println!("  chunk {ci}/{}", bytes.len() / 64); }
            let mut report = vec![0u8; 65];
            report[1..1 + chunk.len()].copy_from_slice(chunk);
            let mut tries = 0;
            while let Err(e) = d.write(&report) {
                tries += 1;
                if tries > 50 {
                    println!("  write failed at chunk {ci}: {e} — restarting update");
                    ok = false;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if !ok { break; }
        }
        if ok { break 'flash; }
        drop(d);
        if attempt == 5 { panic!("flash failed after 5 attempts"); }
    }
    d = f::open_device().expect("open failed");
    drop(d);
    println!("flashed; restarting to APROM");
    std::thread::sleep(std::time::Duration::from_millis(500));
    // The LDROM may drop USB briefly while finalizing the flash; retry.
    let mut restarted = false;
    for _ in 0..150 {
        if f::restart_device().is_ok() { restarted = true; break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !restarted { panic!("restart failed"); }
    let start = std::time::Instant::now();
    loop {
        if let Ok(mut d) = f::open_device() {
            println!("TIMING APROM open after {:.3?}", start.elapsed());
            if let Ok(df) = f::read_dataflash_from(&mut d) {
                println!("flag data[9]={} page2[0x400..0x420]={:02x?}",
                    df[4 + 9], &df[4 + 0x400..4 + 0x420]);
                println!("product id: {}", String::from_utf8_lossy(&df[316..320]));
            }
            break;
        }
        if start.elapsed() > std::time::Duration::from_secs(120) {
            panic!("device never came back after 0xB4");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
#[ignore]
fn test_ldrom_absread_dump_hardware() {
    //! Requires the absread-patched APROM (pico_absread.bin): 0x35(arg1=addr,
    //! arg2=len) streams len bytes from ABSOLUTE device memory. Dumps the
    //! LDROM at 0x00100000 in 2 KB chunks and writes ldrom_m041.bin.
    use crate::firmware::flasher as f;
    let mut dev = f::open_device().expect("open failed");
    let mut dump = Vec::new();
    for addr in (0x0010_0000u32..0x0010_4000).step_by(0x800) {
        let chunk = f::read_abs_pub(&mut dev, addr, 0x800).expect("abs read failed");
        assert_eq!(chunk.len(), 0x800);
        println!("chunk {:#010x}: {:02x?} ...", addr, &chunk[..16]);
        dump.extend_from_slice(&chunk);
    }
    std::fs::write(fixture("ldrom/ldrom_m041.bin"), &dump).unwrap();
    let sp = u32::from_le_bytes(dump[0..4].try_into().unwrap());
    let rst = u32::from_le_bytes(dump[4..8].try_into().unwrap());
    println!("LDROM vectors: SP={sp:#x} reset={rst:#x}");
    assert!(sp >= 0x2000_0000 && sp < 0x2001_0000, "bad SP — dump corrupt");
    assert!(rst & 1 == 1, "bad reset vector");
    println!("LDROM DUMP OK: {} bytes", dump.len());
}

#[test]
#[ignore]
fn test_read_staged_hardware() {
    //! After a v6 (LDROM-stage) patched boot: read the 4 KB staging area at
    //! raw 0x9000 via the absread 0x35 patch and save to ldrom_staged.bin.
    use crate::firmware::flasher as f;
    let mut dev = f::open_device().expect("open failed");
    let mut dump = Vec::new();
    for addr in (0x9000u32..0xA000).step_by(0x800) {
        let chunk = f::read_abs_pub(&mut dev, addr, 0x800).expect("abs read failed");
        assert_eq!(chunk.len(), 0x800);
        dump.extend_from_slice(&chunk);
    }
    std::fs::write(fixture("ldrom/ldrom_staged.bin"), &dump).unwrap();
    println!("staged[0..64]: {:02x?}", &dump[..64]);
    let sp = u32::from_le_bytes(dump[0..4].try_into().unwrap());
    let rst = u32::from_le_bytes(dump[4..8].try_into().unwrap());
    let magic = u32::from_le_bytes(dump[0xFFC..0x1000].try_into().unwrap());
    println!("staged vectors: SP={sp:#x} reset={rst:#x} magic={magic:#x}");
}

#[test]
#[ignore]
fn test_mode_probe_hardware() {
    //! Prints the first bytes of a 0x35 read to identify device mode:
    //! starts with 90 1f 00 20 (stock vector table) => APROM running the
    //! absread image; otherwise (checksum word etc.) => LDROM.
    use crate::firmware::flasher as f;
    let mut dev = f::open_device().expect("open failed");
    let d = f::read_abs_pub(&mut dev, 0, 64).expect("read failed");
    println!("0x35[0..64]: {:02x?}", &d[..]);
    let is_aprom_code = d.starts_with(&[0x90, 0x1f, 0x00, 0x20]);
    println!("guessed mode: {}", if is_aprom_code { "APROM (absread image)" } else { "LDROM (or cache-based 0x35)" });
}

#[test]
#[ignore]
fn test_bootflag_warm_return_hardware() {
    //! Boot-rule experiment: write boot flag data[9]=1 via the PROVEN host
    //! 0x53 path, restart with 0xB4, and measure how long the device takes to
    //! come back. A fast return (seconds) confirms: (a) warm reset re-enumerates
    //! USB when flag==1, (b) the boot rule is data[9]==1 (not "nonzero") — which
    //! would explain why all payload probes went dark (their flag writes never
    //! land; 0xFF != 1 -> boots the no-USB payload in APROM).
    use crate::firmware::flasher as f;
    let mut dev = loop {
        if let Ok(d) = f::open_device() { break d; }
        std::thread::sleep(std::time::Duration::from_millis(300));
    };
    let df = f::read_dataflash_from(&mut dev).expect("read failed");
    println!("current flag data[9]={}", df[4 + 9]);
    let mut user = [0u8; 2044];
    user.copy_from_slice(&df[4..]);
    user[9] = 1;
    f::write_dataflash(&user).expect("write failed");
    drop(dev);
    let start = std::time::Instant::now();
    f::restart_device().expect("restart failed");
    loop {
        if let Ok(mut d) = f::open_device() {
            println!("TIMING re-enumerated after {:.3?}", start.elapsed());
            if let Ok(df2) = f::read_dataflash_from(&mut d) {
                println!("flag now data[9]={}", df2[4 + 9]);
            }
            break;
        }
        if start.elapsed() > std::time::Duration::from_secs(60) {
            panic!("device did not re-enumerate within 60 s after 0xB4 with flag==1");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
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
    // Captures the screen as PNG (scaled 4x) under test-fixtures/out/.
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
    std::fs::write(out("af_screenshot.png"), &png).unwrap();
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
        std::fs::write(out(format!("af_shot_{:02}.bin", i)), &raw).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
}

#[test]
#[ignore]
fn test_flash_plaintext_probe_hardware() {
    // DANGER: flashes a decrypted stock iStick Pico V1.00 image to the first
    // device, then restarts it. Watch whether the device boots stock.
    let bytes = std::fs::read(fixture("pico_v100_plain.bin")).expect("read probe bin");
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
    let bytes = std::fs::read(fixture("pico_v100_plain.bin")).expect("read probe bin");
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

    let bytes = std::fs::read(fixture("pico_v100_plain.bin")).expect("read probe bin");
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
    // 0xC1 screenshot exists only in ArcticFox builds; stock v1.00 drops it.
    match f::screenshot(&mut dev) {
        Ok(s) => println!("screenshot nonzero: {}", s.iter().filter(|b| **b != 0).count()),
        Err(e) => println!("screenshot unavailable (expected on stock firmware): {e}"),
    }
}

#[test]
#[ignore]
fn test_flash_recovery_loop_hardware() {
    // Recovery flasher v3: waits for the Pico, streams immediately, and on
    // any failure goes back to waiting — the LDROM erases and restarts an
    // interrupted update cleanly on the next attempt.
    use crate::firmware::flasher as f;
    let bytes = std::fs::read(std::env::var("RECOVERY_IMAGE")
        .unwrap_or_else(|_| fixture("pico_v100_plain.bin").into())).expect("read probe bin");
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
    // Fixed payload (reads chunk index before the page erase) preferred;
    // fall back to the original (broken) payload for comparison runs.
    let payload = std::fs::read(fixture("ldrom/ldrom_dump_payload_fixed.bin"))
        .or_else(|_| std::fs::read(fixture("ldrom_dump_payload.bin")))
        .expect("read payload");

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
        std::fs::write(fixture("ldrom/ldrom_m041.bin"), &dump).unwrap();
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
    let dump = std::fs::read(fixture("ldrom/ldrom_m041.bin")).unwrap();
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
fn test_probe_flag_hardware() {
    //! Diagnostic for the LDROM dump stall: flashes a MINIMAL probe payload
    //! (probe_flag.py — unlock, erase page at candidate DFBA, write boot flag
    //! data[9]=1 + marker 0xDEADBEEF at data[12..16], reset; no LDROM copy).
    //! If the device returns in LDROM mode and the dataflash read shows the
    //! marker, the candidate DFBA is the one the bootloader honors.
    //! Payload path comes from PROBE_PAYLOAD env var.
    use crate::firmware::flasher as f;
    let payload = std::fs::read(std::env::var("PROBE_PAYLOAD").expect("set PROBE_PAYLOAD"))
        .expect("read probe payload");

    println!("waiting for the Pico (M041)…");
    // PROBE_NO_IDCHECK=1: only require enumeration (open ok), skip the 0x35
    // M041 check. Safe: open_device filters VID 0x0416/PID 0x5020, so the
    // STM32 (0483:5750) can never be opened here. Needed while 0x35 is flaky.
    let no_idcheck = std::env::var("PROBE_NO_IDCHECK").is_ok();
    let wait_start = std::time::Instant::now();
    loop {
        if let Ok(mut d) = f::open_device() {
            if no_idcheck { break; }
            if let Ok(df) = f::read_dataflash_from(&mut d) {
                if df.len() >= 320 && &df[316..320] == b"M041" { break; }
                println!("  skipping {}", String::from_utf8_lossy(&df[316..320]));
            }
        }
        if wait_start.elapsed() > std::time::Duration::from_secs(300) {
            panic!("Pico never enumerated");
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    println!("flashing probe ({} bytes)", payload.len());
    // PROBE_PRESET_FLAG1=1: set boot flag data[9]=1 via the proven host 0x53
    // path BEFORE flashing. Payload resets then warm-boot back into LDROM
    // (with USB) instead of going dark in the no-USB APROM payload.
    if std::env::var("PROBE_PRESET_FLAG1").is_ok() {
        let mut d0 = f::open_device().expect("open for flag preset failed");
        let df0 = f::read_dataflash_from(&mut d0).expect("read for flag preset failed");
        println!("preset: flag was data[9]={}", df0[4 + 9]);
        let mut user = [0u8; 2044];
        user.copy_from_slice(&df0[4..]);
        user[9] = 1;
        // Preload page 0x1E400 (data 0x400..0x600) with 0x00 so a payload-side
        // erase is observable (0x00 -> 0xFF) via the post-run 0x35 read.
        for b in user[0x400..0x600].iter_mut() { *b = 0x00; }
        f::write_dataflash(&user).expect("flag preset write failed");
        drop(d0);
        println!("preset: data[9]=1 written via 0x53");
    }
    let mut d = f::open_device().expect("open failed");
    f::send_command_pub(&mut d, 0xC3, 0, payload.len() as i32).expect("write cmd failed");
    for chunk in payload.chunks(64) {
        let mut report = vec![0u8; 65];
        report[1..1 + chunk.len()].copy_from_slice(chunk);
        let mut tries = 0;
        while let Err(e) = d.write(&report) {
            tries += 1;
            if tries > 50 { panic!("payload write failed: {e}"); }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    drop(d);
    println!("probe flashed; waiting for the device to return in LDROM mode…");

    let start = std::time::Instant::now();
    // Timing channel for diag probes: the device must DISAPPEAR once (payload
    // ran SYSRESETREQ), then we measure gone->return. If it never disappears
    // within 30 s the payload did not reset (hard hang) — that too is a signal.
    let mut gone_at: Option<std::time::Instant> = None;
    let mut t_return_printed = false;
    loop {
        if start.elapsed() > std::time::Duration::from_secs(300) {
            panic!("device never returned to LDROM — DFBA candidate WRONG (flag not honored)");
        }
        match f::open_device() {
            Ok(mut d) => {
                if let Some(g) = gone_at {
                    if !t_return_printed {
                        println!("TIMING return after {:.3?} gone (flash+{:.3?})",
                            g.elapsed(), start.elapsed());
                        t_return_printed = true;
                    }
                } else if start.elapsed() > std::time::Duration::from_secs(30) {
                    println!("TIMING device still present 30 s after flash — payload did NOT reset");
                    gone_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(30));
                }
                match f::read_dataflash_from(&mut d) {
                Ok(df) => {
                    println!("device is back in LDROM after {:.3?}", start.elapsed());
                    println!("flag data[9]={} idx data[8]={:#04x}", df[4 + 9], df[4 + 8]);
                    println!("marker data[12..16]={:02x?}", &df[4 + 12..4 + 16]);
                    println!("page2 data[0x400..0x420]={:02x?}", &df[4 + 0x400..4 + 0x420]);
                    println!("product id: {}", String::from_utf8_lossy(&df[316..320]));
                    assert_eq!(df[4 + 9], 1, "boot flag not set");
                    assert_eq!(&df[4 + 12..4 + 16], &[0xEF, 0xBE, 0xAD, 0xDE],
                        "marker mismatch — LDROM reads a different page than the payload wrote");
                    println!("PROBE OK: DFBA candidate confirmed");
                    break;
                }
                Err(e) => {
                    println!("read failed: {e} — retrying");
                    drop(d);
                }
                }
            }
            Err(_) => {
                if gone_at.is_none() {
                    gone_at = Some(std::time::Instant::now());
                    println!("TIMING device gone at flash+{:.3?}", start.elapsed());
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
#[ignore]
fn test_df_investigate_hardware() {
    //! Where does 0x53 actually write? Dump the top 16 KB of flash via the
    //! absread 0x35, write a marker block (flag data[9]=1 + 0xDEADBEEF at
    //! data[12..16]) via the stock 0x53 path, dump again, diff. Answers:
    //! (a) does 0x53 land at all, (b) at which page, (c) is the boot flag
    //! there afterwards.
    use crate::firmware::flasher as f;
    let mut dev = f::open_device().expect("open failed");

    let snap = |dev: &mut hidapi::HidDevice, tag: &str| -> Vec<u8> {
        let mut buf = Vec::new();
        for addr in (0x1C000u32..0x20000).step_by(0x800) {
            // the absread 0x35 is stateless per command: retry partial reads
            let mut c = Vec::new();
            for attempt in 1..=4 {
                match f::read_abs_pub(dev, addr, 0x800) {
                    Ok(x) => { c = x; break; }
                    Err(e) => {
                        println!("  read {addr:#x} attempt {attempt}: {e}");
                        if attempt == 4 { panic!("abs read failed at {addr:#x}"); }
                    }
                }
            }
            buf.extend_from_slice(&c);
        }
        // per-page summary
        for (i, pg) in buf.chunks(0x800).enumerate() {
            let base = 0x1C000 + i * 0x800;
            let ff = pg.iter().filter(|&&b| b == 0xFF).count();
            let z = pg.iter().filter(|&&b| b == 0).count();
            println!("{tag} page {base:#x}: 0xFF={ff} 0x00={z} of 2048; [316..320]={:02x?} [0..16]={:02x?}",
                &pg[316..320], &pg[..16]);
        }
        buf
    };

    let before = snap(&mut dev, "BEFORE");
    let mut user = [0u8; 2044];
    user[9] = 1;
    user[12..16].copy_from_slice(&[0xEF, 0xBE, 0xAD, 0xDE]);
    user[316..320].copy_from_slice(b"M041");
    f::write_dataflash(&user).expect("0x53 write failed");
    println!("0x53 marker block written");
    let after = snap(&mut dev, "AFTER ");
    for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
        if b != a {
            println!("  diff at {:#x}: {b:#04x} -> {a:#04x}", 0x1C000 + i);
        }
    }
    println!("diff scan done");
}

#[test]
#[ignore]
fn test_ldrom_entry_and_rate_hardware() {
    //! After the 0x53 journal-append at 0x1EE00 (flag data[9]=1 landed):
    //! 1) dump the raw journal region 0x1E000..0x1F000 for offline analysis
    //! 2) 0xB4 restart, detect resulting mode (LDROM vs APROM)
    //! 3) if LDROM: stream dummy4k_v2 (stock[0..4K] + 0xDEADBEEF marker at
    //!    0x18, boot-safe) with per-chunk timing -> LDROM write rate and
    //!    small-update commit behaviour.
    use crate::firmware::flasher as f;
    let mut dev = f::open_device().expect("open failed");

    // 1) journal dump
    let mut region = Vec::new();
    for addr in (0x1E000u32..0x1F000).step_by(0x800) {
        for attempt in 1..=4 {
            match f::read_abs_pub(&mut dev, addr, 0x800) {
                Ok(c) => { region.extend_from_slice(&c); break; }
                Err(e) => {
                    println!("  read {addr:#x} attempt {attempt}: {e}");
                    if attempt == 4 { panic!("abs read failed"); }
                }
            }
        }
    }
    std::fs::write(fixture("ldrom/df_region.bin"), &region).unwrap();
    for (i, row) in region.chunks(16).enumerate() {
        println!("  {:#06x}: {:02x?}", 0x1E000 + i * 16, row);
    }

    // 2) restart and detect mode
    drop(dev);
    f::restart_device().expect("restart failed");
    println!("restarted; waiting for re-enumeration...");
    let w = std::time::Instant::now();
    let mut mode_ldrom = false;
    let mut dev = loop {
        if let Ok(d) = f::open_device() { break d; }
        if w.elapsed() > std::time::Duration::from_secs(30) { panic!("no re-enumeration"); }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    println!("enumerated after {:.3?}", w.elapsed());
    for attempt in 1..=6 {
        match f::read_abs_pub(&mut dev, 0, 64) {
            Ok(d) => {
                println!("0x35[0..16]: {:02x?}", &d[..16]);
                mode_ldrom = !d.starts_with(&[0x90, 0x1f, 0x00, 0x20]);
                break;
            }
            Err(e) => println!("  mode probe attempt {attempt}: {e}"),
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    println!("mode: {}", if mode_ldrom { "LDROM" } else { "APROM" });

    // 3) if LDROM: timed dummy stream
    if mode_ldrom {
        let payload = std::fs::read(fixture("ldrom/dummy4k_v2.bin"))
            .expect("read dummy");
        f::send_command_pub(&mut dev, 0xC3, 0, payload.len() as i32).expect("write cmd failed");
        let t0 = std::time::Instant::now();
        for (ci, chunk) in payload.chunks(64).enumerate() {
            let mut report = vec![0u8; 65];
            report[1..1 + chunk.len()].copy_from_slice(chunk);
            let t = std::time::Instant::now();
            let mut tries = 0;
            while let Err(e) = dev.write(&report) {
                tries += 1;
                if tries > 50 { panic!("dummy write failed at chunk {ci}: {e}"); }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            println!("  chunk {ci}: {:?} (retries {tries})", t.elapsed());
        }
        println!("streamed {} bytes in {:.3?}", payload.len(), t0.elapsed());
    }
}

#[test]
#[ignore]
fn test_absread2_verify_and_dump_hardware() {
    //! Second half of the absread2 experiment (device already flashed with
    //! pico_absread2.bin by test_absread2_dump_hardware). No re-flash:
    //! verify the bl-cave patch bytes, dump the cave region, then dump the
    //! LDROM via the absread 0x35 whatever the cave state is — the result
    //! discriminates: memcpy cave (bl not landed) => APROM alias returns;
    //! ISP cave => real LDROM or 0xBAD00000|idx markers.
    use crate::firmware::flasher as f;
    let mut dev = f::open_device().expect("open failed");
    let peek = |dev: &mut hidapi::HidDevice, addr: u32, len: u32, tag: &str| -> Vec<u8> {
        for attempt in 1..=4 {
            match f::read_abs_pub(dev, addr, len) {
                Ok(c) => {
                    println!("{tag} {addr:#x}: {:02x?}", &c[..(len as usize).min(32)]);
                    return c;
                }
                Err(e) => {
                    println!("  {tag} {addr:#x} attempt {attempt}: {e}");
                    if attempt == 4 { panic!("peek failed at {addr:#x}"); }
                }
            }
        }
        unreachable!()
    };
    peek(&mut dev, 0x1d5c, 8, "patch");
    peek(&mut dev, 0x1d6e, 4, "blcave");
    let cave = peek(&mut dev, 0x8800, 0x800, "cave-region");
    let more = peek(&mut dev, 0x8980, 0x100, "cave-region2");
    let mut region = cave.clone();
    region.extend_from_slice(&more);
    std::fs::write(fixture("ldrom/cave_region.bin"), &region).unwrap();

    let mut dump = Vec::new();
    // 64-byte reads: one command = one report, self-aligning. Long streams
    // lose reports on this link and misalign (proven by seam analysis).
    for addr in (0x0010_0000u32..0x0010_4000).step_by(0x40) {
        let c = peek(&mut dev, addr, 0x40, "ldrom");
        dump.extend_from_slice(&c);
    }
    std::fs::write(fixture("ldrom/ldrom_real.bin"), &dump).unwrap();
    let bad = dump.chunks(4).filter(|w| {
        (u32::from_le_bytes((*w).try_into().unwrap()) & 0xFFFF_0000) == 0xBAD0_0000
    }).count();
    let sp = u32::from_le_bytes(dump[0..4].try_into().unwrap());
    let rst = u32::from_le_bytes(dump[4..8].try_into().unwrap());
    println!("LDROM vectors: SP={sp:#x} reset={rst:#x}; BAD markers: {bad}");
    println!("(alias signature would be SP=0x20001f90 reset=0x171)");
}

#[test]
#[ignore]
fn test_absread2_dump_hardware() {
    //! THE LDROM dump. Flow: (1) enter LDROM (0x53 flag=1 + 0xB4 if needed),
    //! (2) stream pico_absread2.bin (0x35 -> absolute FMC-ISP read cave),
    //! (3) reboot to APROM, verify the image committed (patch bytes at
    //! 0x1d5c + cave at 0x89a8), (4) dump 0x00100000..0x00104000 via the
    //! absread 0x35 and save ldrom_real.bin. Vector check: reset must land
    //! in 0x00100000..0x00104000 (NOT the 0x171 APROM alias), no BAD markers.
    use crate::firmware::flasher as f;

    // mode helper: Ok(true)=LDROM, Ok(false)=APROM
    let probe_mode = |dev: &mut hidapi::HidDevice| -> Option<bool> {
        for _ in 0..6 {
            if let Ok(d) = f::read_abs_pub(dev, 0, 64) {
                return Some(!d.starts_with(&[0x90, 0x1f, 0x00, 0x20]));
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        None
    };
    let wait_open = |secs: u64| -> hidapi::HidDevice {
        let w = std::time::Instant::now();
        loop {
            if let Ok(d) = f::open_device() { return d; }
            if w.elapsed() > std::time::Duration::from_secs(secs) { panic!("no enumeration"); }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    };

    // (1) ensure LDROM
    let mut dev = wait_open(30);
    match probe_mode(&mut dev) {
        Some(true) => println!("already in LDROM"),
        Some(false) => {
            println!("in APROM; setting boot flag via 0x53 + restart");
            let mut user = [0u8; 2044];
            user[9] = 1;
            user[316..320].copy_from_slice(b"M041");
            f::write_dataflash(&user).expect("0x53 flag write failed");
            drop(dev);
            f::restart_device().expect("restart failed");
            dev = wait_open(30);
            match probe_mode(&mut dev) {
                Some(true) => println!("in LDROM after flag+restart"),
                other => panic!("expected LDROM, got {other:?}"),
            }
        }
        None => panic!("mode probe failed"),
    }

    // (2) stream absread2
    let image = std::fs::read(fixture("ldrom/pico_absread2.bin"))
        .expect("read absread2");
    f::send_command_pub(&mut dev, 0xC3, 0, image.len() as i32).expect("write cmd failed");
    let t0 = std::time::Instant::now();
    for (ci, chunk) in image.chunks(64).enumerate() {
        let mut report = vec![0u8; 65];
        report[1..1 + chunk.len()].copy_from_slice(chunk);
        let mut tries = 0;
        while let Err(e) = dev.write(&report) {
            tries += 1;
            if tries > 100 { panic!("image write failed at chunk {ci}: {e}"); }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    println!("flashed absread2 ({} bytes) in {:.3?}", image.len(), t0.elapsed());
    drop(dev);

    // (3) reboot to APROM, verify commit
    std::thread::sleep(std::time::Duration::from_secs(1));
    let mut dev = wait_open(2);
    if probe_mode(&mut dev) == Some(true) {
        println!("still LDROM; 0xB4 to APROM");
        drop(dev);
        let _ = f::restart_device();
        dev = wait_open(30);
    }
    match probe_mode(&mut dev) {
        Some(false) => println!("in APROM"),
        other => panic!("expected APROM after flash, got {other:?}"),
    }
    let hdr = f::read_abs_pub(&mut dev, 0x1d5c, 4).expect("read patch site");
    println!("patch site 0x1d5c: {:02x?} (expect c0 46 21 46)", hdr);
    let cave = f::read_abs_pub(&mut dev, 0x89a8, 4).expect("read cave");
    println!("cave 0x89a8: {:02x?} (expect f0 b5 .. ..)", cave);
    assert_eq!(hdr, [0xc0, 0x46, 0x21, 0x46], "absread2 image did NOT commit");

    // (4) dump the LDROM
    let mut dump = Vec::new();
    for addr in (0x0010_0000u32..0x0010_4000).step_by(0x800) {
        let mut c = Vec::new();
        for attempt in 1..=4 {
            match f::read_abs_pub(&mut dev, addr, 0x800) {
                Ok(x) => { c = x; break; }
                Err(e) => {
                    println!("  read {addr:#x} attempt {attempt}: {e}");
                    if attempt == 4 { panic!("ldrom read failed"); }
                }
            }
        }
        println!("chunk {addr:#010x}: {:02x?} ...", &c[..16]);
        dump.extend_from_slice(&c);
    }
    std::fs::write(fixture("ldrom/ldrom_real.bin"), &dump).unwrap();
    let bad = dump.chunks(4).filter(|w| {
        (u32::from_le_bytes((*w).try_into().unwrap()) & 0xFFFF_0000) == 0xBAD0_0000
    }).count();
    let sp = u32::from_le_bytes(dump[0..4].try_into().unwrap());
    let rst = u32::from_le_bytes(dump[4..8].try_into().unwrap());
    println!("LDROM vectors: SP={sp:#x} reset={rst:#x}; BAD markers: {bad}");
    println!("(alias signature would be SP=0x20001f90 reset=0x171)");
}

#[test]
#[ignore]
fn test_probe_diag_hardware() {
    //! Runner for probe_diag.py payloads (cal0/calN/read/write). Unlike
    //! test_probe_flag_hardware this handles the APROM-absread state
    //! correctly: raw 0xC3 to APROM is ignored and ensure_ldrom_mode is
    //! fooled (APROM[0x0D]==0x01 reads as a set boot flag through the
    //! absread 0x35), so the LDROM transition is done manually: locate the
    //! real dataflash via absread at 0x1E000/0x1F000, set flag data[9]=1,
    //! 0x53 write, 0xB4 warm reset -> LDROM+USB. Then stream the payload
    //! (0xC3) and measure the dark->return time = mask * UNIT (+overhead).
    //! PROBE_PAYLOAD env: path to payload bin.
    use crate::firmware::flasher as f;
    let payload = std::fs::read(std::env::var("PROBE_PAYLOAD").expect("set PROBE_PAYLOAD"))
        .expect("read probe payload");

    // --- phase 1: ensure LDROM with boot flag = 1 ---
    let wait_open = std::time::Instant::now();
    let mut dev = loop {
        if let Ok(d) = f::open_device() { break d; }
        if wait_open.elapsed() > std::time::Duration::from_secs(180) {
            panic!("Pico never enumerated");
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    };
    let in_ldrom = match f::read_dataflash_from(&mut dev) {
        Ok(d) => d.len() >= 320 && &d[316..320] == b"M041",
        Err(_) => false,
    };
    if in_ldrom {
        println!("already in LDROM (product id readable)");
    } else {
        println!("in APROM; locating real dataflash via absread...");
        let mut found = None;
        for base in [0x1F000u32, 0x1E000] {
            match f::read_abs_pub(&mut dev, base, 2048) {
                Ok(pg) => {
                    let sum_ok = u32::from_le_bytes(pg[0..4].try_into().unwrap())
                        == pg[4..].iter().map(|b| *b as u32).sum::<u32>();
                    let pid_ok = &pg[316..320] == b"M041";
                    println!("  page {base:#x}: checksum_ok={sum_ok} pid_ok={pid_ok} flag={}", pg[4 + 9]);
                    if sum_ok && pid_ok { found = Some(pg); break; }
                }
                Err(e) => println!("  page {base:#x}: read failed: {e}"),
            }
        }
        let pg = found.expect("no valid dataflash page found via absread");
        let mut user = [0u8; 2044];
        user.copy_from_slice(&pg[4..]);
        user[9] = 1;
        f::write_dataflash(&user).expect("flag write failed");
        drop(dev);
        f::restart_device().expect("restart failed");
        println!("flag=1 written, restarted; waiting for LDROM...");
        let w = std::time::Instant::now();
        loop {
            if let Ok(mut d) = f::open_device() {
                if let Ok(dd) = f::read_dataflash_from(&mut d) {
                    if dd.len() >= 320 && &dd[316..320] == b"M041" && dd[4 + 9] == 1 {
                        println!("in LDROM after {:.3?}", w.elapsed());
                        break;
                    }
                }
            }
            if w.elapsed() > std::time::Duration::from_secs(60) {
                panic!("no LDROM after flag+restart");
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    // --- phase 2: stream payload ---
    let mut d = f::open_device().expect("open failed");
    f::send_command_pub(&mut d, 0xC3, 0, payload.len() as i32).expect("write cmd failed");
    for (ci, chunk) in payload.chunks(64).enumerate() {
        let mut report = vec![0u8; 65];
        report[1..1 + chunk.len()].copy_from_slice(chunk);
        let t = std::time::Instant::now();
        let mut tries = 0;
        while let Err(e) = d.write(&report) {
            tries += 1;
            if tries > 50 { panic!("payload write failed at chunk {ci}: {e}"); }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        println!("  chunk {ci}: {:?} (retries {tries})", t.elapsed());
    }
    drop(d);
    println!("payload flashed; watching dark->return...");

    // --- phase 3: dark->return timing = mask * UNIT + overhead ---
    let start = std::time::Instant::now();
    let mut gone_at: Option<std::time::Instant> = None;
    loop {
        if start.elapsed() > std::time::Duration::from_secs(300) {
            panic!("device never returned");
        }
        match f::open_device() {
            Ok(mut d) => {
                if let Some(g) = gone_at {
                    println!("TIMING dark = {:.3?} (flash+{:.3?})", g.elapsed(), start.elapsed());
                    if let Ok(dd) = f::read_dataflash_from(&mut d) {
                        println!("returned: flag={} idx={:#04x} pid={}",
                            dd[4 + 9], dd[4 + 8], String::from_utf8_lossy(&dd[316..320]));
                    }
                    break;
                }
            }
            Err(_) => {
                if gone_at.is_none() {
                    gone_at = Some(std::time::Instant::now());
                    println!("device dark at flash+{:.3?}", start.elapsed());
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
#[ignore]
fn test_df_roundtrip_hardware() {
    //! Host-side dataflash write/read roundtrip. Toggles one tail byte via
    //! the LDROM 0x53 write command, reads back, restores. Answers: does the
    //! LDROM-side dataflash WRITE work at all, and does 0x35 reflect the
    //! latest write immediately (vs wear-leveled slot games)?
    use crate::firmware::flasher as f;
    println!("waiting for the Pico…");
    let w = std::time::Instant::now();
    let mut dev = loop {
        if let Ok(d) = f::open_device() { break d; }
        if w.elapsed() > std::time::Duration::from_secs(180) { panic!("Pico never enumerated"); }
        std::thread::sleep(std::time::Duration::from_millis(300));
    };
    let r1 = f::read_dataflash_from(&mut dev).expect("read1 failed");
    println!("baseline: flag data[9]={} data[8]={:#04x} sum16={:02x?}",
        r1[4 + 9], r1[4 + 8], &r1[4..20]);

    let mut data = [0u8; 2044];
    data.copy_from_slice(&r1[4..]);
    let idx = 2000; // harmless tail byte
    data[idx] ^= 0x5A;
    f::write_dataflash(&data).expect("write failed");
    drop(dev);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let mut dev = f::open_device().expect("re-open failed");
    let r2 = f::read_dataflash_from(&mut dev).expect("read2 failed");
    let toggle_visible = r2[4 + idx] == data[idx];
    let same_elsewhere = r2[4..4 + idx] == r1[4..4 + idx];
    println!("toggle visible: {toggle_visible}, rest identical: {same_elsewhere}");
    println!("r2 flag data[9]={} idx data[8]={:#04x}", r2[4 + 9], r2[4 + 8]);
    if !toggle_visible {
        println!("r2[1996..2044]: {:02x?}", &r2[4 + 1996..]);
    }
    // restore original content regardless
    let mut orig = [0u8; 2044];
    orig.copy_from_slice(&r1[4..]);
    f::write_dataflash(&orig).expect("restore write failed");
    drop(dev);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let r3 = f::read_dataflash().expect("read3 failed");
    println!("restored: {}", r3[4..] == r1[4..]);
}

#[test]
#[ignore]
fn test_df_map_hardware() {
    //! Maps the 0x35 read window: arg1 = 0..0x1000 in 0x100 steps (arg1=0x800
    //! worked, 0x1000 died mid-stream — find the exact boundary). Saves every
    //! response to /var/home/j/dfmap/ for offline analysis. Stops at first
    //! failure (device crash => replug needed).
    use crate::firmware::flasher as f;
    std::fs::create_dir_all(fixture("out/dfmap")).unwrap();
    println!("waiting for the Pico…");
    let w = std::time::Instant::now();
    let mut dev = loop {
        if let Ok(d) = f::open_device() { break d; }
        if w.elapsed() > std::time::Duration::from_secs(180) { panic!("Pico never enumerated"); }
        std::thread::sleep(std::time::Duration::from_millis(300));
    };
    for arg1 in (0..=0x1000i32).step_by(0x100) {
        std::thread::sleep(std::time::Duration::from_millis(200));
        match f::send_command_pub(&mut dev, 0x35, arg1, 2048)
            .and_then(|_| f::read_exact_pub(&mut dev, 2048))
        {
            Ok(buf) => {
                let cks = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                let sum: u32 = buf[4..].iter().map(|b| *b as u32).sum();
                println!("arg1={arg1:#06x} cks={cks:#010x} sum={sum:#010x} {} data8={:02x?}",
                    if cks == sum { "OK " } else { "BAD" }, &buf[4..12]);
                std::fs::write(out(format!("dfmap/{arg1:06x}.bin")), &buf).unwrap();
            }
            Err(e) => { println!("arg1={arg1:#06x} FAILED: {e} — stopping"); break; }
        }
    }
    println!("map done");
}

#[test]
#[ignore]
fn test_ldrom_direct_read_hardware() {
    //! Gate-A probe: 0x35 honors arg1 as a dataflash-relative offset
    //! (arg1=0x800 returned distinct content). If the base is DFBA 0x1E000,
    //! arg1 = 0x100000 - 0x1E000 = 0xE2000 reads the LDROM directly.
    //! Reads 9 x 2044-byte chunks (4-byte prefix stripped), assembles 16 KB.
    use crate::firmware::flasher as f;
    println!("waiting for the Pico…");
    let w = std::time::Instant::now();
    let mut dev = loop {
        if let Ok(d) = f::open_device() { break d; }
        if w.elapsed() > std::time::Duration::from_secs(180) { panic!("Pico never enumerated"); }
        std::thread::sleep(std::time::Duration::from_millis(300));
    };
    let mut dump = Vec::new();
    for i in 0..9i32 {
        let arg1 = 0xE2000 + i * 2044;
        let mut ok = false;
        for _ in 0..3 {
            match f::send_command_pub(&mut dev, 0x35, arg1, 2048)
                .and_then(|_| f::read_exact_pub(&mut dev, 2048))
            {
                Ok(buf) => {
                    println!("chunk {i} @ arg1={arg1:#x}: data16={:02x?}", &buf[4..20]);
                    dump.extend_from_slice(&buf[4..]);
                    ok = true;
                    break;
                }
                Err(e) => {
                    println!("chunk {i} err: {e} — retry");
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
            }
        }
        if !ok { println!("chunk {i} FAILED — stopping"); break; }
    }
    println!("got {} bytes", dump.len());
    if dump.len() >= 16 {
        let sp = u32::from_le_bytes(dump[0..4].try_into().unwrap());
        let rst = u32::from_le_bytes(dump[4..8].try_into().unwrap());
        println!("LDROM vector table: SP={sp:#010x} reset={rst:#06x}");
    }
    if dump.len() >= 16384 {
        dump.truncate(16384);
        std::fs::write(fixture("ldrom/ldrom_m041.bin"), &dump).unwrap();
        println!("SAVED 16 KB ldrom_m041.bin");
        assert!(dump.windows(4).any(|w| w == b"HIDC"), "no HIDC sig — wrong region?");
    }
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
            std::fs::write(out("stm32_shot.bin"), &shot).unwrap();
        }
        Err(e) => println!("screenshot failed: {e}"),
    }
}

#[test]
#[ignore]
fn test_restore_stock_hardware() {
    //! Restore stock v1.00 over whatever RE image is on the device, using the
    //! proven LDROM path (0x53 flag=1 + 0xB4 -> LDROM; 0xC3 stream; auto-boot
    //! APROM). If the current APROM is the absread2 image, also recover the
    //! LDROM's last 64 bytes (0x103FC0) first — reads are one-command-behind,
    //! so each address is read twice and the second result is used.
    use crate::firmware::flasher as f;
    let wait_open = |secs: u64| -> hidapi::HidDevice {
        let w = std::time::Instant::now();
        loop {
            if let Ok(d) = f::open_device() { return d; }
            if w.elapsed() > std::time::Duration::from_secs(secs) { panic!("no enumeration"); }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    };
    let mut dev = wait_open(60);

    // opportunistic: LDROM tail recovery while absread2 is still running
    if let Ok(d) = f::read_abs_pub(&mut dev, 0, 64) {
        if d.starts_with(&[0x90, 0x1f, 0x00, 0x20]) {
            let _ = f::read_abs_pub(&mut dev, 0x103FC0, 64); // prime (one-behind)
            if let Ok(tail) = f::read_abs_pub(&mut dev, 0x103FC0, 64) {
                println!("LDROM tail 0x103FC0: {:02x?}", &tail[..32]);
                std::fs::write(fixture("ldrom/ldrom_tail.bin"), &tail).unwrap();
            }
        } else {
            println!("not absread APROM (stock or LDROM) — skipping tail read");
        }
    }

    // enter LDROM (if not already there)
    let in_ldrom = match f::read_abs_pub(&mut dev, 0, 64) {
        Ok(d) => !d.starts_with(&[0x90, 0x1f, 0x00, 0x20]),
        Err(_) => false,
    };
    if !in_ldrom {
        let mut user = [0u8; 2044];
        user[9] = 1;
        user[316..320].copy_from_slice(b"M041");
        f::write_dataflash(&user).expect("0x53 flag write failed");
        drop(dev);
        f::restart_device().expect("restart failed");
        dev = wait_open(30);
        println!("flag=1 written, restarted");
    } else {
        println!("already in LDROM");
    }

    // flash stock
    let image = std::fs::read(fixture("pico_v100_plain.bin")).expect("read stock");
    f::send_command_pub(&mut dev, 0xC3, 0, image.len() as i32).expect("write cmd failed");
    let t0 = std::time::Instant::now();
    for (ci, chunk) in image.chunks(64).enumerate() {
        let mut report = vec![0u8; 65];
        report[1..1 + chunk.len()].copy_from_slice(chunk);
        let mut tries = 0;
        while let Err(e) = dev.write(&report) {
            tries += 1;
            if tries > 100 { panic!("stock write failed at chunk {ci}: {e}"); }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    println!("flashed stock ({} bytes) in {:.3?}", image.len(), t0.elapsed());
    drop(dev);

    // verify: device returns and 0x35 serves the dataflash cache (stock semantics)
    dev = wait_open(30);
    for attempt in 1..=10 {
        match f::read_dataflash_from(&mut dev) {
            Ok(df) => {
                println!("0x35[0..16]: {:02x?}", &df[..16]);
                println!("product id: {}", String::from_utf8_lossy(&df[316..320]));
                assert!(!df.starts_with(&[0x90, 0x1f, 0x00, 0x20]), "still absread image?!");
                println!("STOCK RESTORED");
                return;
            }
            Err(e) => println!("verify attempt {attempt}: {e}"),
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    panic!("stock restore verification failed");
}


#[test]
#[ignore]
fn test_stock_cycle_hardware() {
    //! Full stock-library cycle on the Pico (M041) running ArcticFox:
    //! 1. read dataflash -> product id + fw version word (assert M041);
    //! 2. flash the bundled `resources/firmware/af_190602.bin` AS SHIPPED
    //!    (encrypted package) through `flash_firmware_guarded`, the same
    //!    path the app uses;
    //! 3. re-open, read dataflash, screenshot (0xC1 is AF-only: must
    //!    respond) and print the nonzero count;
    //! 4. undo: restore `AF_fw/decrypted/af_190602.dec.bin` via the
    //!    recovery flash path, verify version word 110 again.
    //!
    //! The undo ALWAYS runs, even when the step-2 flash fails or the
    //! flashed image does not boot: the device must end on af_190602.
    //! Step 5 then asserts the step-2 result, so a non-booting encrypted
    //! build fails the test only after the device has been restored.
    use crate::firmware::flasher as f;

    // 1. identify. The Pico drops off the bus when idle and a previous
    // interrupted flash leaves it flapping in LDROM — wait for it, and
    // retry the dataflash read.
    let df = loop {
        if let Ok(mut dev) = f::open_device() {
            if let Ok(df) = f::read_dataflash_from(&mut dev) { break df; }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    };
    let pid = String::from_utf8_lossy(&df[316..320])
        .trim_matches(char::from(0)).trim().to_string();
    let ver = f::parse_fw_version(&df).expect("parse_fw_version failed");
    println!("before: pid={pid} fw_version={ver} boot_flag={}", df[4 + 9]);
    assert_eq!(pid, "M041");

    // 2. flash the bundled build as shipped (encrypted package). A USB drop
    // mid-stream is recoverable: the LDROM restarts an interrupted update
    // cleanly, so retry the whole guarded flash. Note flash_firmware_guarded
    // only returns Ok once the device is back in APROM (boot flag 0), so an
    // image that does not boot shows up here as an error, not a pass.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().join("resources/firmware/af_190602.bin");
    let bytes = std::fs::read(&path).expect("read bundled build");
    println!("flashing {} ({} bytes, as shipped/encrypted)", path.display(), bytes.len());
    let t = std::time::Instant::now();
    let mut flash_err = None;
    for attempt in 1..=3 {
        // Wait for the Pico to be on the bus before each attempt; it drops
        // off when idle or mid-update and re-enumerates on its own.
        let w = std::time::Instant::now();
        while f::open_device().is_err() {
            if w.elapsed() > std::time::Duration::from_secs(180) {
                panic!("Pico never re-enumerated (attempt {attempt})");
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        match f::flash_firmware_guarded(&bytes, Some("M041")) {
            Ok(()) => { flash_err = None; break; }
            Err(e) => {
                println!("flash attempt {attempt} failed: {e}");
                flash_err = Some(e.to_string());
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }
    match &flash_err {
        None => println!("flashed in {:.3?}", t.elapsed()),
        Some(e) => println!("ENCRYPTED FLASH DID NOT COMPLETE: {e}"),
    }

    // 3. post-flash state (bounded wait; the device may be flapping in
    // LDROM if the image did not boot — that is a RESULT, not a test error)
    let mut post: Option<(i32, u8, Option<usize>)> = None;
    let w = std::time::Instant::now();
    while w.elapsed() < std::time::Duration::from_secs(90) {
        if let Ok(mut dev) = f::open_device() {
            if let Ok(df) = f::read_dataflash_from(&mut dev) {
                let ver2 = f::parse_fw_version(&df).expect("parse_fw_version failed");
                let flag = df[4 + 9];
                println!("after flash: pid={} fw_version={ver2} boot_flag={flag}",
                    String::from_utf8_lossy(&df[316..320]));
                let shot = f::screenshot(&mut dev).ok().map(|s| {
                    let n = s.iter().filter(|b| **b != 0).count();
                    println!("screenshot nonzero bytes: {n}/{}", s.len());
                    n
                });
                if shot.is_none() {
                    println!("screenshot: no response (not booted into AF)");
                }
                post = Some((ver2, flag, shot));
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if post.is_none() {
        println!("after flash: device never readable within 90 s");
    }

    // 4. undo: restore the decrypted af_190602 via the recovery path.
    // ALWAYS runs — the device must end on af_190602 regardless of how the
    // encrypted flash went.
    let undo = std::fs::read("/var/home/j/cloudy-af/AF_fw/decrypted/af_190602.dec.bin")
        .expect("read undo image");
    println!("undo: restoring af_190602.dec.bin ({} bytes) via recovery path", undo.len());
    let t = std::time::Instant::now();
    let mut restored = false;
    for attempt in 1..=10 {
        match f::recovery_flash(&undo, Some("M041"), |s| println!("recovery: {s}")) {
            Ok(()) => { restored = true; break; }
            Err(e) => {
                println!("undo attempt {attempt} failed: {e} — retrying");
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }
    assert!(restored, "undo flash failed after 10 attempts");
    println!("undo flashed in {:.3?}", t.elapsed());
    let df = loop {
        if let Ok(mut dev) = f::open_device() {
            if let Ok(df) = f::read_dataflash_from(&mut dev) { break df; }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    };
    let ver3 = f::parse_fw_version(&df).expect("parse_fw_version failed");
    println!("after undo: pid={} fw_version={ver3}",
        String::from_utf8_lossy(&df[316..320]));
    assert_eq!(ver3, 110, "device not back on af_190602");

    // 5. now that the device is safely back on af_190602, assert the
    // step-2/3 outcome.
    let (ver2, flag2, shot2) = post.expect("device never readable after encrypted flash");
    assert!(flash_err.is_none(), "encrypted flash failed: {:?}", flash_err);
    assert_eq!((ver2, flag2), (110, 0), "encrypted build did not boot into AF");
    assert!(shot2.is_some(), "AF screenshot did not respond after encrypted flash");
}
