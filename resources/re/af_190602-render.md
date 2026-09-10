# af_190602 Charge-Screen Render Reverse-Engineering Notes

**Build:** `af_190602` (ArcticFox, Nuvoton M451/M471-class, VandalProof-encrypted APROM)  
**Decrypted image:** `AF_fw/decrypted/af_190602.dec.bin` (116,716 bytes, gitignored RE artifact)  
**Ghidra project:** `DecryptProject/ghidra-projects/AFSamples`  
**Date:** 2026-09-05

## Findings Summary

| Item | Address | Evidence |
|------|---------|----------|
| Display buffer | `0x20002758` – `0x20002b58` | 1024 bytes = 64×128/8; referenced by inner render loop at `0x8cd0` |
| Render function | `FUN_0000b53c` @ `0x0000b53c` | Charge-screen composer; calls inner body at `0x8cd0` |
| Render inner body | `0x00008cd0` | Nested loops over 0..0x7f (128) and 0..0x3f (64); writes to display buffer |
| Emulator entry | `0x00008cd1` | Thumb-mode address of inner body (used by `test_af_190602_render_gate`) |
| HID screenshot cmd | `0xC1` | From `resources/re/ldrom-commands.json` |

## Discovery Procedure

1. Imported `af_190602.dec.bin` into the existing `AFSamples` Ghidra project with ARM Cortex-M4F/Thumb-2 processor spec.
2. Ran `FindMagicConstants.java` to surface RAM pointers and display-sized constants. Notable hits included `0x20002c10`, `0x20003e20`, `0x20004540`.
3. Ran `FindRenderFunction.java` / `DumpRenderCandidates.java` to enumerate functions referencing display-sized constants (64, 128, 1024) and RAM.
4. Identified `FUN_0000b53c` as the charge-screen render composer. Its inner body at `0x8cd0` contains the actual 128×64 framebuffer fill loops.
5. Verified loop bounds and buffer writes in the `DumpRenderCandidates.java` output for `FUN_0000b53c`.

## Descriptor

See `resources/animations/af_190602.json`:

```json
{
  "build": "af_190602",
  "render_entry": "0x00008cd1",
  "display_buffer": { "start": "0x20002758", "end": "0x20002b58", "width": 64, "height": 128 },
  "ram_globals": [
    { "start": "0x20000000", "end": "0x20000e00" },
    { "start": "0x20002c00", "end": "0x20002d00" }
  ],
  "args": [0, 0, 0, 0]
}
```

## Open Items

- **Hook site:** not yet chosen. Candidate is the first instruction of `FUN_0000b53c` (`0x0000b53c`), where a 4-byte detour can branch to a code cave and then resume.
- **Code cave:** not yet located. Need a ≥512-byte unused/padding region in APROM, 4-byte aligned, with no xrefs.
- **Config byte:** not yet chosen. Dataflash base is `0x1F000` (cache 0x800 bytes); need a spare byte outside the settings struct.
- **Phase global:** not yet chosen. Will be a free 4-byte word inside an allowed `ram_globals` range.
- **Thumb-2 emulator support:** the layer-4 gate currently fails on the first instruction (`push.w {r4-r11, lr}` at `0x8cd0`, encoding `0xe92d 0x4ff0`). Task 2 extends the emulator incrementally.

## Next Step

Implement 32-bit Thumb-2 decoding in `src-tauri/src/firmware/emu/thumb.rs` / `cpu.rs`, driven by repeated `test_af_190602_render_gate` failures, until the layer-4 gate runs clean.
