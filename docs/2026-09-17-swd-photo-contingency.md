# SWD pad identification from teardown photos — contingency (2026-09-17)

**Division of labor:** you take and submit the photos per §1; **I do the
analysis and hand back the clip-point guidance per §2** — MCU part
identification, ranked candidate pads, the Nu-Link clip table, the
verification gate, and the attach procedure. You never clip a wire based
on your own guess; every clip point comes from my guidance package, and
each one traces to a datasheet pin number plus a measured continuity
check. If my guidance is unclear, ask before clipping.

Use when bench section B of `docs/2026-09-16-nulink-bench-probe-list.md`
(continuity buzzing) stalls: pads unclear, silkscreen absent, or the MCU
pinout is unverified. The user photographs the open device; the agent
returns clip-point guidance for the **plugged-in Nu-Link** (5-pin header:
VCC, SWDIO, SWCLK, GND, nRST — verify against the ZORZA silkscreen first).
First photo session AND first clip go on the **sacrificial mule Pico (M041,
75W)** — never the daily-driver Pico 25 (M077) before the mule survives
attach (SWD plan Global Constraints).

## 1. Photo request checklist (what the user submits)

Drop into `captures/teardown/` (untracked — personal hardware photos are
never committed). Name files `<device>-<shot>.jpg`, e.g.
`pico75w-mcu.jpg`. All shots: diffuse light, NO flash (glare kills
silkscreen), in focus, ruler or coin in frame for scale on at least one.

| Shot | Must show | Name |
|---|---|---|
| 1 | PCB front, whole board | `<dev>-front.jpg` |
| 2 | PCB back, whole board | `<dev>-back.jpg` |
| 3 | MCU close-up — top marking legible (part number + package suffix + date code) | `<dev>-mcu.jpg` |
| 4 | USB connector area + any pads/vias around it | `<dev>-usb.jpg` |
| 5 | Battery contacts + wiring side | `<dev>-batt.jpg` |
| 6 | Every candidate pad row / unpopulated footprint / via cluster, macro | `<dev>-pads.jpg` |

Also state in the submission message: which device this is (Pico 75W M041
vs Pico 25 M077) and that the battery is OUT while shooting.

## 2. What the agent returns (clip-point guidance package)

1. **MCU identification** from shot 3: part + package → SWD pin numbers
   (ICE_DAT/SWDIO, ICE_CLK/SWCLK, nRST) taken ONLY from the M451 datasheet
   for that package (SWD plan Task 1 Step 5). Pin numbers are never guessed
   from photos alone.
2. **Candidate pad map** from shots 4/6, ranked (labelled pads first, then
   unlabelled pads near the MCU, then via clusters). ArcticFox-family boards
   typically expose SWD on a short pad row near the USB connector; the USB
   shield is the GND reference.
3. **Clip table** for the plugged Nu-Link:

   | Nu-Link pin | Connect to | Notes |
   |---|---|---|
   | VCC | **LEAVE UNCONNECTED** | target self-powers from its battery (probe list A.1) |
   | SWDIO | identified pad | datasheet pin + measured pad |
   | SWCLK | identified pad | datasheet pin + measured pad |
   | GND | USB shield or battery − | first connection, every time |
   | nRST | identified pad | optional; skip if not found |

4. **Verification gate** (multimeter, battery OUT — must pass before any
   clip touches a pad):
   - continuity < few ohms from each candidate pad to its MCU pin;
   - every non-GND candidate to GND: open;
   - SWDIO to SWCLK: open (swapped-wire trap caught here, not at the probe).
5. **Attach procedure + fallback ladder** (probe list C): generic OpenOCD
   attach (`-expected-id 0`, SWD plan Task 2 config) → DPIDR nonzero =
   wiring proven; DPIDR 0x00000000/0xFFFFFFFF → swap SWDIO/SWCLK once →
   still dead → Nu-Link ICP path (SWD plan Task 3). NEVER "retry with VCC".
   Session stays READ-ONLY (probe list A.3).

## 3. Agent-side analysis procedure

- Read the photos directly (image read), zoom candidate regions before
  naming pads.
- Cross-check silkscreen labels (D/CLK, SWD/SWC, 4P/5P footprints) against
  the datasheet pinout — a label only counts when its trace reaches the
  expected MCU pin (or is explicitly requested as a re-shot).
- If a pin cannot be traced visually, request exactly the re-shots needed
  (region, angle, lighting) instead of guessing.

## 4. Safety rails (verbatim, probe list A)

1. 3.3 V logic only — NEVER connect the programmer's VCC to the target.
2. Battery stays in for live attach; battery OUT for continuity work.
3. Read-only session: never program anything during pad identification.

## 5. If attach fails after verified wiring

- Photograph the MCU underside and neighbouring joints (bridge/crack) and
  the pad surfaces: matte/pitted pads suggest conformal coating — light
  scrape with a fiberglass pen, re-verify continuity, retry once.
- If SWD is config-disabled, only the Nu-Link ICP path connects (SWD plan
  Task 2 Step 6). Do NOT experiment with config bits on any device.
