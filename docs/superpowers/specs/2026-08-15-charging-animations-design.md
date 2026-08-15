# Charging Screen / Clock Animations Design

## Overview

Add optional animated effects to the device’s charging screen and clock display. The user selects an animation under **Appearance → Charge Screen** and **Appearance → Clock Type**. Cloudy AF then patches the firmware and flashes it to the connected device over USB/HID. An **Undo Changes** button restores the original/stock firmware.

## Important Constraint: No Full Firmware Read-Back

The ArcticFox HID protocol supports reading **dataflash**, **configuration**, **monitoring data**, and **screenshots**, but it does **not** support reading the full APROM firmware back from the device. Therefore the workflow cannot "pull firmware from device" for patching. Instead:

1. The user provides a stock/known firmware `.bin` file for their device (or Cloudy AF ships a small library of stock firmwares keyed by Product ID).
2. Cloudy AF reads the device **dataflash** to confirm the Product ID and device model.
3. Cloudy AF decrypts the stock firmware, applies the animation patch, re-encrypts if needed.
4. Cloudy AF switches the device to LDROM bootloader mode and flashes the patched firmware.
5. **Undo Changes** flashes the original stock firmware again.

## UI Placement

- **Appearance → Charge Screen** dropdown gains an **Animation** sub-field:
  - `Off`
  - `Swirl`
  - `Gradient Fade`
  - `Rippling Wave`
- **Appearance → Clock Type** dropdown gains the same **Animation** sub-field.
- New JSON config fields: `ChargeScreenAnimation` and `ClockAnimation`.
- New global toolbar button: **Undo Changes** (enabled only after a patched firmware has been flashed).

## On-Device Rendering

The animation is rendered procedurally by injected ARM Thumb code:

- A small routine is injected into an unused or overwriteable region of flash.
- The routine hooks the charge-screen or clock render function.
- It draws the selected effect pixel-by-pixel into the display buffer at 8 fps.
- A configuration byte selects the active effect (`Off`, `Swirl`, `Fade`, `Ripple`).

### Effect Details

All effects use a shared 16-entry sine/cosine lookup table to avoid floating-point math on the MCU.

1. **Swirl**
   - A radial highlight rotates around the screen center.
   - Phase advances by 22.5° per frame (16 frames = full rotation).
   - Rendered by sampling angle = `atan2(y, x) + phase` and thresholding against a radial mask.

2. **Gradient Fade**
   - A vertical intensity bar moves up and down the screen.
   - Intensity at pixel `(x, y)` is driven by `sin(phase + y * scale)`.
   - Pixels are on where intensity exceeds a threshold.

3. **Rippling Wave**
   - Concentric rings expand from the screen center.
   - Distance from center drives a `sin(distance - phase)` value.
   - Rings wrap around when they reach the screen edge.

### Display Targets

- **Primary:** 64×128 monochrome OLED (Block1 vertical packing).
- **Secondary:** 96×16 status bar (Block2 horizontal packing). The same math is cropped/scaled to fit.

## Firmware Patch & Flash Workflow

1. **Identify device**
   - Read dataflash (`0x35`) over HID.
   - Parse Product ID at offset 312 and hardware/firmware versions.
   - Match against known device definitions.

2. **Select stock firmware**
   - If Cloudy AF has a bundled stock firmware for the Product ID, use it.
   - Otherwise prompt the user to choose a `.bin` file.

3. **Build animation patch**
   - Decrypt the stock firmware.
   - Reverse-engineer and hook the charge-screen / clock renderer.
   - Assemble the ARM Thumb effect routine.
   - Inject routine + configuration byte.
   - Re-encrypt if necessary.

4. **Flash**
   - If not in LDROM mode, set dataflash boot flag, write dataflash, send restart.
   - Wait for re-enumeration (up to 15 s).
   - Send `WriteData(0, firmware.Length)` and stream the patched firmware.
   - Cache the original stock firmware path/hash for Undo.

5. **Undo Changes**
   - Flash the cached original stock firmware (no patch).
   - Clear the animation config fields or set them to `Off`.

## Reversibility

- The animation patch is treated as a reversible change because Undo simply reflashes the original stock firmware.
- A local backup of the original firmware is kept in the app data directory (`~/.config/cloudy-af/firmware-backups/<product-id>-<timestamp>.bin`).

## Dependencies

- Firmware editor / patch infrastructure (Patches tab).
- HID flashing commands (`ReadDataflash`, `WriteDataflash`, `WriteData`, `Restart`).
- Device identification via dataflash.
- ARM Thumb assembler or a small bytecode generator for the injected routine.

## Phased Implementation Plan

1. **Phase 0 — Foundation:** Build the Patches tab and HID flashing commands.
2. **Phase 1 — Static fallback:** Allow frame-based animations using 16 image slots as a proof of concept.
3. **Phase 2 — Reverse engineering:** Disassemble the charge-screen and clock renderers of a reference firmware.
4. **Phase 3 — Code injection:** Implement one procedural effect (Gradient Fade is simplest) and flash it.
5. **Phase 4 — More effects:** Add Swirl and Rippling Wave.
6. **Phase 5 — UI integration:** Add Appearance dropdowns and Undo Changes button.
