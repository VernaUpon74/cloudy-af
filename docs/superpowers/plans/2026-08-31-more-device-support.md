Add to 99-arcticfox.rules more ArcticFox compatible device vendor IDs. Probably a reference in NToolbox somewhere. Current version only supports Pico 25.

## Findings (2026-09-03) — DONE

The premise was wrong: no extra vendor IDs are needed. All ArcticFox-compatible
devices share the same Nuvoton HID bootloader interface, VID `0x0416` / PID
`0x5020` — confirmed in the official NFE source
(`DecryptProject/nfe-source/src/NCore/USB/HidConnector.cs` uses a single
hardcoded VID/PID for every device). The existing
`flatpak/50-arcticfox-config.rules` therefore already covers every device.

The real gate was `resources/firmware/devices.json`: its `nuvoton` line only
listed 19 Eleaf M-series product IDs, so Joyetech (E-series) and
Wismec/Vaporflask/etc. (W-series) devices hit the "unknown product id" path.
Fixed by extending `product_ids` to the full 47-device union of the NFE source
list (`NCore/USB/HidDeviceInfo.cs`) and the pre-existing entries. Flasher,
loader, and udev rules needed no changes; stock tests pass.

dockur/windows was considered and rejected: NToolbox under Windows emulation
would only re-extract device data already available in the local NFE source.
