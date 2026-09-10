# VandalProof firmware encryption

ArcticFox firmware packages distributed since ~2018 (`af_*.bin`) are encrypted
with a scheme NFirmwareEditor calls "VandalProof". This document records the
reverse-engineered format so the images can be decrypted/encrypted without the
original (obfuscated) tooling.

## Format

- **Cipher:** AES-128-CBC (hand-rolled CBC over raw AES blocks).
- **Key:** the 16 ASCII bytes of the string `FA89412D87B0EFD9`
  (hex of the key bytes: `46 41 38 39 34 31 32 44 38 37 42 30 45 46 44 39`).
  Note: the key is the ASCII string itself, **not** a hex-decoded 8-byte value.
- **IV:** the first 16 bytes of the file are the IV (random per encryption).
  There is no header or magic.
- **Ciphertext:** starts at file offset `0x10` and runs to end of file; total
  file length minus 16 must be a multiple of 16.
- **Padding:** PKCS7.
- **Validation after decrypt:** search the plaintext for the
  `Joyetech APROM` / `Joyetech APP` marker.

## Key provenance

NFirmwareEditor's obfuscated loader (a `Dummy` DLL XOR-decoded from `NCode.dll`
resources) derives the key from the .NET assembly **public key token**: each
token byte is XORed with a counter starting at `0x42` and incremented by 3, then
formatted as uppercase hex, producing the 16 ASCII characters
`FA89412D87B0EFD9`. Reference implementation:
`DecryptProject/vp-crypt-Release/src/main.rs` (`decrypt_lib`, `generate_key`).

The matching on-device decryptor lives in the device's LDROM updater, so
encrypted images can also be flashed as-is — the device decrypts them during
the update.

## Implementation

- `src-tauri/src/firmware/encryption.rs` — `decrypt_vandalproof` /
  `encrypt_vandalproof` (key constant `VP_KEY`, CBC helpers in `cbc_mode`).
- Covered by roundtrip and known-ciphertext tests in
  `src-tauri/src/firmware/tests/encryption_test.rs`.
- Standalone reference tool: `DecryptProject/vp-crypt-Release/`.
