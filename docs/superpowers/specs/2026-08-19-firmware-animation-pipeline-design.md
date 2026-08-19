# Firmware Animation Pipeline Design

> Supersedes the phased outline in `2026-08-15-charging-animations-design.md`
> where they conflict (notably: full firmware read-back IS possible, and stock
> firmware is bundled rather than downloaded at runtime).

## Overview

Add animated effects to the device's charging screen and clock display. The
Firmware Editor acquires a firmware image — either by **reading it back from
the connected device in realtime** or from a **bundled stock library** —
decrypts it, injects a procedural ARM Thumb animation routine, re-encrypts,
and flashes it back over HID. An **Undo Changes** button restores the
untouched firmware. All firmware handling is offline: stock builds ship as
bundled resources and every encryption scheme (None, Joyetech, ArcticFox,
ArcticFox2, VandalProof) is implemented locally and verified against every
build published on nfeteam.org.

## Decisions Log

- **Read-back approach:** gated on LDROM command-set analysis (Phase 1a) —
  true APROM read-back if the bootloader supports it, otherwise
  version-matched stock + instrumented verify-read. Bare-metal USB payload
  rejected: flashing any APROM payload erases the firmware it would preserve.
- **RE tooling is an optional feature:** Ghidra + JDK are fetched on demand
  via `scripts/fetch-ghidra.sh` into the gitignored `DecryptProject/`
  scratch directory and are never packaged with the app. The main project
  carries no RE dependencies.
- **Bundled stock set:** `af_170222.bin`, `af_180913.bin`, `af_190602.bin`
  (Nuvoton), `af_211009.bin` (STM32). `af_190624` is excluded at user request
  (a corrupt copy circulated locally). All four verified decryptable.
- **Animation RE targets:** `af_190602` (Nuvoton) and `af_211009` (STM32).
  Other builds fall back to stock-flash without animation.
- **Visual verification:** on-device results are checked with the HID
  screenshot facility (the NFirmwareEditor "Screen Taker" equivalent already
  implemented as `flasher::screenshot`), capturing frames over time.

## Workflow

1. **Acquire firmware**
   - *Read from Device (preferred when feasible):* in LDROM mode, stream the
     device's current APROM back over HID. Only available if Phase 1a finds a
     read command in the LDROM bootloader (see Read-back Design).
   - *Download Stock (fallback):* read dataflash, parse Product ID
     (offset 316), firmware version, and hardware version; map to a hardware
     line via `resources/firmware/devices.json`; load the matching bundled
     build. If the version matches a bundled build, that image is the
     device's own firmware. Unknown Product IDs ask the user to pick a line
     explicitly; an unmatched build is never flashed automatically.
2. **Decrypt** with the scheme cascade (None → Joyetech → ArcticFox →
   ArcticFox2 → VandalProof), validated by the `Joyetech APROM` /
   `Joyetech APP` markers.
3. **Patch** — generate the animation patch from a per-build descriptor and
   the selected effect, apply via the existing patch engine.
4. **Re-encrypt** with the image's original scheme and **flash** through the
   existing LDROM flow (`flash_firmware_guarded` with Product-ID guard).
5. **Undo Changes** — reflash the cached untouched image (backups already
   land in `~/.config/cloudy-af/firmware-backups/<product-id>-<timestamp>.bin`).

## Read-back Design

The HID protocol has no *documented* APROM read command, and any payload
flashed into APROM erases the firmware it was meant to preserve — so true
read-back of a device's current image is gated on a Phase 1 discovery step:

**Phase 1a — LDROM analysis (feasibility gate).** Complete the LDROM dump
(the trampoline payload technique is proven: `/var/home/j/ldrom_dump_payload.bin`
copies LDROM chunks into dataflash, which is read back over HID), then
reverse the dumped bootloader in Ghidra and enumerate its HID command set.
The LDROM is custom Joyetech code (it contains the VandalProof decryptor) and
may implement more than the four commands NFE uses.

**Outcome A — LDROM has an APROM-read command.** Implement it in
`flasher.rs`; the device's exact current image streams back in LDROM mode
without erasing anything. This is the preferred path.

**Outcome B — no read command.** True read-back of unknown firmware is
impossible on this hardware. The pipeline then works as follows:

- *Version-matched stock:* the dataflash already carries the firmware
  version; if it matches a bundled stock build, that bundled image **is** the
  device's firmware, byte for byte, and becomes the patch base.
- *Custom/unknown firmware:* the app warns that the current image cannot be
  preserved and will be replaced by stock + patch (Undo then restores stock,
  not the unknown custom image).
- *Instrumented verify-read:* a bundled stock image patched with a new HID
  `ReadFlash(offset, len)` command (injected into the firmware's existing HID
  dispatcher, found via the same Ghidra RE used for animation hooks) is
  flashed when verification is wanted; it streams the APROM back in seconds
  to confirm a flash succeeded byte-for-byte and to produce undo backups.
  The instrumentation never persists — the session always ends by flashing
  the final image, and a dataflash marker byte lets the next app launch
  detect an interrupted session and offer immediate restore.

Either way, one Ghidra RE effort (dispatcher + render hooks) feeds both the
read-back and animation features.

## Animation Injection

- **Hook:** a 2-byte Thumb branch (`B`) patch inside the charge-screen render
  function (and separately the clock render function), placed after the frame
  is composed but before it is pushed to the display. Injected code reads the
  animation config byte and either branches back immediately (`Off`) or draws
  the effect into the display buffer and returns.
- **Config byte:** stored at a fixed dataflash offset so it survives reboots
  and is settable from the app without re-flashing. Values: `0=Off`,
  `1=Swirl`, `2=Gradient Fade`, `3=Rippling Wave`.
- **Effects:** Gradient Fade implemented first (phase counter + 16-entry sine
  LUT + vertical threshold), then Swirl (angle + phase vs radial mask), then
  Rippling Wave (sin(distance − phase) rings). All integer math on a shared
  16-entry sine/cosine LUT; ~200–400 bytes of Thumb each, emitted by a small
  Rust bytecode builder with unit-tested instruction encodings (not a
  general-purpose assembler).
- **Pacing:** phase advances per rendered frame using the render loop's
  natural cadence (~8 fps target). No timer hardware in v1; a systick hook is
  the documented follow-up if cadence is wrong on-device.
- **Per-build descriptors:** `resources/animations/<build>.json` (versioned)
  carries hook-site offset, code-cave offset/size, config-byte dataflash
  offset, display-buffer pointer, and dispatcher patch points. Supporting
  another build later is data, not code.
- **Display targets:** primary 64×128 monochrome OLED (Block1 vertical
  packing); secondary 96×16 status bar (Block2) with the same math
  cropped/scaled.

## UI

- **Firmware Editor window:** toolbar gains **Read from Device** and
  **Download Stock** next to Open/Save. New **Animations** tab: effect list
  (Off / Swirl / Gradient Fade / Rippling Wave), charge-screen and clock
  target selectors, **Apply & Flash** button. The generated patch appears in
  the existing Patches tab for inspection before flashing. **Undo Changes**
  (existing) remains the escape hatch.
- **Main window:** **Appearance → Charge Screen** and **Appearance → Clock
  Type** each gain an **Animation** sub-field (same four values), persisted as
  `ChargeScreenAnimation` / `ClockAnimation` in the JSON config. Changing a
  field marks the firmware "patch pending" and prompts opening the Firmware
  Editor — the main window never flashes directly.
- Long operations (read-back, flash) run async with progress bars and
  cancellable dialogs, matching the existing flash progress UI.

## Safety & Error Handling

- Every flash goes through `flash_firmware_guarded`; STM32 builds are never
  offered for Nuvoton devices and vice versa. STM32 read-back/flash is
  verified on hardware before its UI is enabled; until then the feature ships
  Nuvoton-only with STM32 controls greyed out.
- Before any flash (including the instrumented read-back image) the current
  on-device image (if read) or chosen stock is copied to the firmware-backups
  directory. Undo restores it.
- Read-back/flash failures retry per the recovery-flasher pattern; a device
  that drops mid-flash is handled by the existing emergency recovery loop. A
  failed checksum aborts before any erase.

## Testing

- **Unit:** Thumb encodings (golden bytes), descriptor parsing, patch
  generation (known descriptor + effect → golden patch), VP round-trips of
  patched images, config-byte read/write.
- **Hardware-in-the-loop (`#[ignore]`, mirroring existing flasher tests):**
  - Read-back of a device flashed with a known build must byte-match the
    bundled stock image.
  - Full apply → verify → undo cycle on the Pico (M041).
  - **Visual verification via HID screenshots** (the NFirmwareEditor Screen
    Taker equivalent, `flasher::screenshot`): after flashing an effect, put
    the device on the charge screen (or clock), capture frames ~250 ms apart,
    and assert (a) successive frames differ (animation is running), (b) the
    changing region matches the effect's expected pattern (e.g. a moving
    vertical intensity band for Gradient Fade), (c) after Undo, frames are
    static and match the stock screen.
- **Manual end-to-end:** open Firmware Editor → Read from Device → apply
  Gradient Fade → flash → observe charge screen → screenshot-compare → Undo.

## Dependencies (already in tree)

- Firmware decrypt/encrypt incl. VandalProof (`firmware::encryption`).
- Patch engine (`firmware::patch`), firmware loader/definitions.
- HID flashing, recovery loop, dataflash access, screenshots
  (`firmware::flasher`).
- Ghidra headless setup, **optional dev-time tooling only**: RE results ship
  as JSON descriptors, so the app never calls Ghidra at runtime and nothing
  Ghidra-related is packaged. Developers fetch the slimmed ARM-headless
  subset (~125 MB Ghidra + ~210 MB JDK 21) on demand with
  `scripts/fetch-ghidra.sh`, which installs into the gitignored
  `DecryptProject/` directory.
- Verified stock firmware corpus (`AF_fw/nfeteam/`) as the source of bundled
  builds and as ground truth for read-back verification.

## Implementation Phasing

1. **Phase 1 — Read-back:** complete the LDROM dump and reverse its HID
   command set in Ghidra (feasibility gate). Outcome A: implement the LDROM
   APROM-read command in `flasher.rs` + "Read from Device" button. Outcome B:
   instrumented verify-read firmware (dispatcher RE on af_190602) +
   dataflash version matching. Hardware-verify against bundled stock either
   way.
2. **Phase 2 — Stock library & pipeline:** bundle the four builds +
   `devices.json`; "Download Stock" flow; patch → re-encrypt → flash → undo
   wired end-to-end in the Firmware Editor.
3. **Phase 3 — First effect:** renderer RE (charge screen) on af_190602;
   Thumb bytecode builder; Gradient Fade descriptor + injection; screenshot
   verification.
4. **Phase 4 — Remaining effects & clock:** Swirl, Ripple, clock hook,
   Appearance dropdowns + config fields.
5. **Phase 5 — STM32:** repeat read-back + renderer RE for af_211009; enable
   STM32 UI after hardware verification.
