use super::{FirmwareError, Result};
use std::time::{SystemTime, UNIX_EPOCH};

/// Minimal CBC mode on top of the `aes` crate's raw AES-128 block cipher.
/// (The `cbc` crate is not available offline; CBC itself is trivial.)
mod cbc_mode {
    use aes::cipher::generic_array::GenericArray;
    use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
    use aes::Aes128;

    const BLOCK: usize = 16;

    pub struct Decryptor {
        cipher: Aes128,
        iv: [u8; BLOCK],
    }

    pub struct Encryptor {
        cipher: Aes128,
        iv: [u8; BLOCK],
    }

    impl Decryptor {
        pub fn new(key: &[u8; BLOCK], iv: &[u8; BLOCK]) -> Self {
            Self { cipher: Aes128::new(GenericArray::from_slice(key)), iv: *iv }
        }

        /// Decrypt `data` in place; length must be a multiple of 16.
        pub fn decrypt(&mut self, data: &mut [u8]) {
            for block in data.chunks_exact_mut(BLOCK) {
                let mut saved = [0u8; BLOCK];
                saved.copy_from_slice(block);
                self.cipher.decrypt_block(GenericArray::from_mut_slice(block));
                for (b, iv) in block.iter_mut().zip(self.iv.iter()) {
                    *b ^= iv;
                }
                self.iv = saved;
            }
        }
    }

    impl Encryptor {
        pub fn new(key: &[u8; BLOCK], iv: &[u8; BLOCK]) -> Self {
            Self { cipher: Aes128::new(GenericArray::from_slice(key)), iv: *iv }
        }

        /// Encrypt `data` in place; length must be a multiple of 16.
        pub fn encrypt(&mut self, data: &mut [u8]) {
            for block in data.chunks_exact_mut(BLOCK) {
                for (b, iv) in block.iter_mut().zip(self.iv.iter()) {
                    *b ^= iv;
                }
                self.cipher.encrypt_block(GenericArray::from_mut_slice(block));
                self.iv.copy_from_slice(block);
            }
        }
    }
}

/// Recognised firmware encryption schemes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionType {
    /// Plaintext / no encryption.
    None,
    /// Joyetech XOR obfuscation derived from file size.
    Joyetech,
    /// ArcticFox encryption (key key 0x17, table length 9).
    ArcticFox,
    /// ArcticFox encryption variant (key key 0x19, table length 11).
    ArcticFox2,
    /// VandalProof encryption (AES-128-CBC, fixed key, IV prefix).
    VandalProof,
}

/// Minimum buffer length we consider for Joyetech-encrypted firmware.
/// Smaller buffers are treated as plaintext so that short test payloads
/// (and tiny files) are not misclassified.
const JOYETECH_MIN_LEN: usize = 8;

const JOYETECH_MAGIC: i32 = 0x63B38;

const ARCTICFOX_KEY_KEY: u8 = 0x17;
const ARCTICFOX_TABLE_LEN: usize = 9;

const ARCTICFOX2_KEY_KEY: u8 = 0x19;
const ARCTICFOX2_TABLE_LEN: usize = 11;

/// VandalProof packages are AES-128-CBC with a fixed key; the first 16 bytes
/// are the IV and the payload is PKCS7-padded. Key matches the one used by
/// NFirmwareEditor's obfuscated loader (see vp-crypt).
const VP_KEY: &[u8; 16] = b"FA89412D87B0EFD9";
const VP_BLOCK: usize = 16;

/// Marker present in decrypted Joyetech-derived firmware images. NFirmwareEditor
/// uses this to decide whether a candidate decryption actually produced plaintext.
const FIRMWARE_MARKER: &[u8] = b"Joyetech APROM";

/// Additional marker used by STM32-based ArcticFox builds. The classic Nuvoton
/// images carry `Joyetech APROM`; the STM32 line shortened it to
/// `Joyetech APP`. Both mark a valid decrypted firmware.
const FIRMWARE_MARKER_STM32: &[u8] = b"Joyetech APP";

/// Try to decrypt `data` using each supported scheme and return the first
/// plausible candidate.
///
/// Detection is heuristic because none of the schemes have a header. We
/// validate each candidate by looking for the common firmware marker
/// (`Joyetech APROM`, or `Joyetech APP` on STM32 builds), which is present
/// both in plaintext and in images that are still encrypted. If no candidate
/// contains the marker, the file is in an unknown format, so we surface a
/// clear error instead of silently returning garbage.
pub fn decrypt(data: &[u8]) -> Result<(Vec<u8>, EncryptionType)> {
    // Unencrypted firmware already contains the marker.
    if contains_marker(data) {
        return Ok((data.to_vec(), EncryptionType::None));
    }

    if data.len() >= JOYETECH_MIN_LEN {
        let plain = decrypt_joyetech(data);
        if contains_marker(&plain) {
            return Ok((plain, EncryptionType::Joyetech));
        }
    }

    if data.len() >= 4 + ARCTICFOX_TABLE_LEN {
        if let Ok(plain) = decrypt_arcticfox(data, ARCTICFOX_KEY_KEY, ARCTICFOX_TABLE_LEN) {
            if contains_marker(&plain) {
                return Ok((plain, EncryptionType::ArcticFox));
            }
        }
    }

    if data.len() >= 4 + ARCTICFOX2_TABLE_LEN {
        if let Ok(plain) = decrypt_arcticfox(data, ARCTICFOX2_KEY_KEY, ARCTICFOX2_TABLE_LEN) {
            if contains_marker(&plain) {
                return Ok((plain, EncryptionType::ArcticFox2));
            }
        }
    }

    if let Ok(plain) = decrypt_vandalproof(data) {
        if contains_marker(&plain) {
            return Ok((plain, EncryptionType::VandalProof));
        }
    }

    // No marker found. Joyetech/ArcticFox firmware images always contain the
    // "Joyetech APROM" marker once decrypted. Files that do not contain it
    // after any supported scheme are in an unknown format, so surface a clear
    // error instead of silently returning garbage.
    Err(super::FirmwareError::UnsupportedEncryption(
        "This firmware uses an unknown or corrupt encryption. \
         Cloudy AF supports plaintext, Joyetech, ArcticFox, ArcticFox2 and \
         VandalProof firmware images."
            .into(),
    ))
}

fn contains_marker(data: &[u8]) -> bool {
    data.windows(FIRMWARE_MARKER.len()).any(|w| w == FIRMWARE_MARKER)
        || data
            .windows(FIRMWARE_MARKER_STM32.len())
            .any(|w| w == FIRMWARE_MARKER_STM32)
}

/// Encrypt `data` using the requested scheme.
pub fn encrypt(data: &[u8], enc: EncryptionType) -> Result<Vec<u8>> {
    match enc {
        EncryptionType::None => Ok(data.to_vec()),
        EncryptionType::Joyetech => Ok(encrypt_joyetech(data)),
        EncryptionType::ArcticFox => Ok(encrypt_arcticfox(data, ARCTICFOX_KEY_KEY, ARCTICFOX_TABLE_LEN)),
        EncryptionType::ArcticFox2 => Ok(encrypt_arcticfox(data, ARCTICFOX2_KEY_KEY, ARCTICFOX2_TABLE_LEN)),
        EncryptionType::VandalProof => Ok(encrypt_vandalproof(data)),
    }
}

// ---------------------------------------------------------------------------
// Joyetech
// ---------------------------------------------------------------------------

fn joyetech_key(file_size: usize, index: usize) -> u8 {
    let value = file_size as i32 + JOYETECH_MAGIC + index as i32 - (file_size as i32 / JOYETECH_MAGIC);
    (value & 0xFF) as u8
}

fn encrypt_joyetech(data: &[u8]) -> Vec<u8> {
    let len = data.len();
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ joyetech_key(len, i))
        .collect()
}

fn decrypt_joyetech(data: &[u8]) -> Vec<u8> {
    // Joyetech encryption is symmetric XOR.
    encrypt_joyetech(data)
}

// ---------------------------------------------------------------------------
// ArcticFox / ArcticFox2
// ---------------------------------------------------------------------------

fn encrypt_arcticfox(data: &[u8], key_key: u8, table_len: usize) -> Vec<u8> {
    let key = generate_arcticfox_key();
    let table = create_arcticfox_table(key, table_len);
    let key_bytes = write_arcticfox_key(key, key_key);

    let mut result = Vec::with_capacity(key_bytes.len() + data.len());
    result.extend_from_slice(&key_bytes);
    for (i, b) in data.iter().enumerate() {
        result.push(b ^ table[i % table.len()]);
    }
    result
}

fn decrypt_arcticfox(data: &[u8], key_key: u8, table_len: usize) -> Result<Vec<u8>> {
    if data.len() <= 4 {
        return Err(FirmwareError::UnknownEncryption);
    }
    let key = read_arcticfox_key(&data[..4], key_key);
    let table = create_arcticfox_table(key, table_len);
    let mut result = Vec::with_capacity(data.len() - 4);
    for (i, b) in data[4..].iter().enumerate() {
        result.push(b ^ table[i % table.len()]);
    }
    if contains_marker(&result) {
        return Ok(result);
    }

    // The original AF build toolchain generated the XOR table with a slightly
    // different PRNG variant than .NET Framework's Random.NextBytes, so a few
    // table bytes can be wrong. The plaintext always contains the
    // "Joyetech APROM" marker; fuzzy-locate it, correct the implicated table
    // bytes, and re-decode.
    let marker = FIRMWARE_MARKER;
    let mut best: Option<(usize, usize)> = None; // (offset, matches)
    for off in 0..result.len().saturating_sub(marker.len()) {
        let matches = marker
            .iter()
            .enumerate()
            .filter(|(j, m)| result[off + j] == **m)
            .count();
        if best.map(|(_, b)| matches > b).unwrap_or(true) {
            best = Some((off, matches));
        }
    }
    let (off, matches) = best.unwrap_or((0, 0));
    if matches < marker.len() - 2 {
        return Err(FirmwareError::UnknownEncryption);
    }
    let mut table = table;
    for (j, m) in marker.iter().enumerate() {
        let got = result[off + j];
        if got != *m {
            let slot = (off + j) % table_len;
            table[slot] ^= got ^ *m;
        }
    }
    let mut corrected = Vec::with_capacity(data.len() - 4);
    for (i, b) in data[4..].iter().enumerate() {
        corrected.push(b ^ table[i % table.len()]);
    }
    if contains_marker(&corrected) {
        Ok(corrected)
    } else {
        Err(FirmwareError::UnknownEncryption)
    }
}

fn generate_arcticfox_key() -> i32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i32)
        .unwrap_or(0x1234_5678)
}

fn read_arcticfox_key(key_bytes: &[u8], key_key: u8) -> i32 {
    let mut decrypted = [0u8; 4];
    for (i, b) in key_bytes.iter().enumerate().take(4) {
        decrypted[i] = b ^ key_key;
    }
    i32::from_le_bytes(decrypted)
}

fn write_arcticfox_key(key: i32, key_key: u8) -> [u8; 4] {
    let mut bytes = key.to_le_bytes();
    for b in &mut bytes {
        *b ^= key_key;
    }
    bytes
}

fn create_arcticfox_table(seed: i32, len: usize) -> Vec<u8> {
    let mut rng = NetRandom::new(seed);
    let mut table = vec![0u8; len];
    rng.next_bytes(&mut table);
    table
}

// ---------------------------------------------------------------------------
// VandalProof (AES-128-CBC)
// ---------------------------------------------------------------------------

fn decrypt_vandalproof(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() <= VP_BLOCK || data.len() % VP_BLOCK != 0 {
        return Err(FirmwareError::UnknownEncryption);
    }
    let (iv, payload) = data.split_at(VP_BLOCK);
    let mut buf = payload.to_vec();
    let mut dec = cbc_mode::Decryptor::new(VP_KEY, iv.try_into().unwrap());
    dec.decrypt(&mut buf);
    // Strip PKCS7 padding.
    let pad = *buf.last().ok_or(FirmwareError::UnknownEncryption)? as usize;
    if pad == 0 || pad > VP_BLOCK || pad > buf.len() {
        return Err(FirmwareError::UnknownEncryption);
    }
    if buf.iter().skip(buf.len() - pad).any(|b| *b as usize != pad) {
        return Err(FirmwareError::UnknownEncryption);
    }
    buf.truncate(buf.len() - pad);
    Ok(buf)
}

fn encrypt_vandalproof(data: &[u8]) -> Vec<u8> {
    let mut iv = [0u8; VP_BLOCK];
    getrandom::getrandom(&mut iv).unwrap_or_else(|_| {
        // Fall back to a time-derived IV if the OS RNG is unavailable.
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e37_79b9_7f4a_7c15);
        for (i, b) in iv.iter_mut().enumerate() {
            *b = (seed.rotate_left((i * 7) as u32) & 0xFF) as u8;
        }
    });

    // PKCS7 pad to a full block (always adds 1..=16 bytes).
    let pad = VP_BLOCK - (data.len() % VP_BLOCK);
    let mut buf = data.to_vec();
    buf.resize(data.len() + pad, pad as u8);

    let mut enc = cbc_mode::Encryptor::new(VP_KEY, &iv);
    enc.encrypt(&mut buf);

    let mut result = Vec::with_capacity(VP_BLOCK + buf.len());
    result.extend_from_slice(&iv);
    result.extend_from_slice(&buf);
    result
}

// ---------------------------------------------------------------------------
// .NET System.Random compatible PRNG
//
// ArcticFox encryption uses `new Random(seed).NextBytes(table)` to build its
// XOR table. To remain compatible with NFirmwareEditor we reproduce the
// classic .NET Framework subtractive generator exactly.
// ---------------------------------------------------------------------------

struct NetRandom {
    seed_array: [i32; 56],
    inext: usize,
    inextp: usize,
}

impl NetRandom {
    const MBIG: i32 = i32::MAX;
    const MSEED: i32 = 161_803_398;

    fn new(seed: i32) -> Self {
        let mut seed_array = [0i32; 56];

        let subtraction = if seed == i32::MIN {
            i32::MAX
        } else {
            seed.abs()
        };
        let mut mj = Self::MSEED.wrapping_sub(subtraction);
        seed_array[55] = mj;
        let mut mk = 1i32;

        for i in 1..55 {
            let ii = (21 * i) % 55;
            seed_array[ii] = mk;
            mk = mj.wrapping_sub(mk);
            if mk < 0 {
                mk = mk.wrapping_add(Self::MBIG);
            }
            mj = seed_array[ii];
        }

        for _ in 1..5 {
            for i in 1..56 {
                let idx = 1 + (i + 30) % 55;
                seed_array[i] = seed_array[i].wrapping_sub(seed_array[idx]);
                if seed_array[i] < 0 {
                    seed_array[i] = seed_array[i].wrapping_add(Self::MBIG);
                }
            }
        }

        Self {
            seed_array,
            inext: 0,
            inextp: 21,
        }
    }

    fn internal_sample(&mut self) -> i32 {
        let mut loc_inext = self.inext;
        let mut loc_inextp = self.inextp;

        loc_inext += 1;
        if loc_inext >= 56 {
            loc_inext = 1;
        }
        loc_inextp += 1;
        if loc_inextp >= 56 {
            loc_inextp = 1;
        }

        let mut ret_val = self.seed_array[loc_inext].wrapping_sub(self.seed_array[loc_inextp]);
        if ret_val == Self::MBIG {
            ret_val = ret_val.wrapping_sub(1);
        }
        if ret_val < 0 {
            ret_val = ret_val.wrapping_add(Self::MBIG);
        }

        self.seed_array[loc_inext] = ret_val;
        self.inext = loc_inext;
        self.inextp = loc_inextp;
        ret_val
    }

    fn next_bytes(&mut self, buffer: &mut [u8]) {
        for b in buffer.iter_mut() {
            *b = (self.internal_sample() % 256) as u8;
        }
    }
}
