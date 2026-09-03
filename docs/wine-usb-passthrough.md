# Running NToolbox under Wine with USB/HID passthrough

> Machine-specific recipe for this dev box. For a portable guide usable on
> any Linux system, see `docs/wine-usb-passthrough-generic.md`.

Recipe for running the Windows NToolbox (NFE Tools) on this machine with the
device attached, verified 2026-09-03 with the Pico (Nuvoton HID,
VID `0x0416` / PID `0x5020`).

## TL;DR

The `org.winehq.Wine` Flatpak (Wine 7.0) does **not** work: its winebus
creates only a raw `USB\VID_0416&PID_5020` node and never builds the HID PDO
that HidSharp-based apps enumerate. Use the newer Bottles runner directly on
the host instead — no Flatpak sandbox, no `/run/udev` mounting needed:

```bash
RUNNER=~/.var/app/com.usebottles.bottles/data/bottles/runners/soda-11.0-5
WINEPREFIX=~/wine-ntoolbox \
  "$RUNNER/bin/wine" \
  "/var/home/j/ArcticFox Stuff/NFE-Tools-v190703-21.46/NToolbox.exe"
```

First run auto-installs the runner's bundled `wine-mono` into the prefix
(`drive_c/windows/mono/`), which NToolbox (.NET 4.0) needs.

## Prerequisites

- Device attached and accessible: `ls -l /dev/hidraw*` should show the
  Nuvoton node with an ACL (`crw-rw----+`, TAGS `uaccess`) — the repo's
  `flatpak/50-arcticfox-config.rules` udev rule covers this. All
  ArcticFox-compatible devices share VID `0x0416` / PID `0x5020`.
- The Bottles Flatpak with the `soda-11.0-5` runner downloaded
  (`~/.var/app/com.usebottles.bottles/data/bottles/runners/soda-11.0-5`).
  Any recent runner (Wine 9+) should do; the feature needed is the direct
  `/dev/hidraw*` enumeration (`hid:build_initial_deviceset_direct`).

## Verifying HID enumeration

```bash
RUNNER=~/.var/app/com.usebottles.bottles/data/bottles/runners/soda-11.0-5
WINEPREFIX=~/wine-probe WINEDEBUG=+plugplay,+winebus,+hid \
  "$RUNNER/bin/wine" wineboot --init 2>&1 | grep -i 0416
```

Working output includes:

```
trace:hid:maybe_add_devnode Considering /dev/hidraw1...
trace:hid:bus_create_hid_device desc {vid 0416, pid 5020, ... is_hidraw 1 ...}
```

On the broken Wine 7.0 Flatpak the same probe shows only a USB-level node
(`plugplay ... USB\VID_0416&PID_5020`) and `bus_create_hid_device` entries
solely for Wine's virtual `VID_845E` devices.

## Notes / pitfalls

- A `WINEPREFIX` under `/tmp` fails ("not owned by you") — keep prefixes in
  `$HOME`.
- Don't reuse the old Flatpak prefix (`~/.var/app/org.winehq.Wine/data/wine`)
  without an update pass; a fresh prefix is cleaner. NToolbox is portable and
  keeps its settings in `NToolboxConfiguration.xml` next to the exe.
- The udev device entry has no `ID_INPUT` property — that is fine for the
  hidraw path; Wine 11 does not filter on it.
- `oleacc`/`uiautomation` fixme spam in the log is harmless WinForms noise.
