# NToolbox under Wine on Linux — USB/HID passthrough setup

Generic setup guide for running the Windows NFE Tools (NToolbox /
NFirmwareEditor) on Linux with direct access to the device. Works for any
ArcticFox-compatible mod — they all share the same Nuvoton HID bootloader
interface (USB VID `0x0416`, PID `0x5020`). Verified end-to-end 2026-09-03
(device detected, config read/written).

## Why a recent Wine is required

NToolbox talks to the device through HidSharp, which enumerates Windows HID
devices. Wine exposes Linux `/dev/hidraw*` nodes as HID devices through
`winebus.sys` — but **older Wine builds never create the HID device node for
these gadgets**. Symptom: the app opens fine but shows no device, even though
Linux itself sees it.

- **Wine ≤ 7 (e.g. the old `org.winehq.Wine` Flatpak): broken.** winebus
  registers only a raw `USB\VID_0416&PID_5020` node; no HID PDO is created.
- **Wine 9+ (current distros, Bottles/Lutris runners such as
  soda/caffe/GE-Proton builds): works.** These enumerate `/dev/hidraw*`
  directly (look for `hid:build_initial_deviceset_direct` in a debug log).

No Flatpak-sandboxed Wine build is known to work — even with
`--device=all` and `/run/udev` mounted, enumeration depends on the Wine
version, not the sandbox permissions. Prefer a **host-native** Wine or a
runner extracted on the host.

## 1. Make the device accessible to your user

Create a udev rule, e.g. `/etc/udev/rules.d/50-arcticfox-config.rules`:

```
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="0416", ATTRS{idProduct}=="5020", MODE="0666", TAG+="uaccess"
```

Then:

```bash
sudo udevadm control --reload
sudo udevadm trigger
```

Plug the device in and confirm:

```bash
ls -l /dev/hidraw*    # the Nuvoton node should be group/user writable (crw-rw---- +)
udevadm info /dev/hidrawN | grep -E 'ID_VENDOR_ID|ID_MODEL_ID'
# expect ID_VENDOR_ID=0416, ID_MODEL_ID=5020
```

## 2. Pick a Wine runner

Any of these work:

- Your distro's Wine 9+ (`wine --version` to check).
- A Bottles runner, used standalone — no Bottles sandbox involved:

```bash
RUNNER=~/.var/app/com.usebottles.bottles/data/bottles/runners/<runner-name>
"$RUNNER/bin/wine" --version
```

## 3. Create a prefix and run

NToolbox is a portable .NET 4.0 app; wine-mono (auto-installed into a fresh
prefix by most runners) is sufficient — no .NET Framework install needed.

```bash
WINEPREFIX=~/wine-ntoolbox \
  "$RUNNER/bin/wine" /path/to/NFE-Tools/NToolbox.exe
```

Notes:

- Keep the prefix inside your home directory — Wine refuses prefixes in
  locations it considers unsafe (e.g. a root-owned `/tmp`).
- NToolbox stores its settings next to the exe (`NToolboxConfiguration.xml`),
  so run it from a writable directory.

## 4. Verifying HID enumeration (troubleshooting)

```bash
WINEPREFIX=~/wine-probe WINEDEBUG=+plugplay,+winebus,+hid \
  "$RUNNER/bin/wine" wineboot --init 2>&1 | grep -i 0416
```

Working output includes lines like:

```
trace:hid:maybe_add_devnode Considering /dev/hidraw1...
trace:hid:bus_create_hid_device desc {vid 0416, pid 5020, ... is_hidraw 1 ...}
```

If you only see `USB\VID_0416&PID_5020` (no `bus_create_hid_device` with
`is_hidraw 1`), your Wine is too old — switch runners. If nothing appears at
all, the udev rule/permissions (step 1) are the problem.

Harmless noise you can ignore: `oleacc`, `uiautomation`, and `mscoree
parse_supported_runtime` fixme spam — normal for WinForms apps on wine-mono.

## Known-good reference setup

Verified on Fedora (Wayland), device: Eleaf iStick Pico (Nuvoton HID Transfer,
`0416:5020`) running ArcticFox af_190602:

- Runner: `soda-11.0-5` (Wine 11, TkG) from Bottles, run directly on the host
- Prefix: fresh, `~/sodaprobe`, wine-mono 10.4.1 auto-installed
- Result: NToolbox v190703 detected the device and successfully wrote
  configuration (puff cut-off 60 s confirmed in the device's config blob)
