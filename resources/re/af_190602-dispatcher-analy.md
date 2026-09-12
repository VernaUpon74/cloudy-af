# af_190602 Screen Dispatcher & Power-Off Effect RE — Differential Analysis

**Build:** `af_190602` (ArcticFox, Nuvoton M451/M471-class, VandalProof-decrypted)
**Decrypted image:** `AF_fw/decrypted/af_190602.dec.bin`
**Ghidra project:** `scratch-stash/DecryptProject/ghidra-projects/AFSamples` (10 programs)
**Companion:** `resources/re/af_190602-render.md` (charge-screen render RE, Phase 3 Task 1)

---

## 1. Reproducible Ghidra headless pipeline (this session)

Ghidra 12.1.2 + Temurin JDK 21 live at `scratch-stash/DecryptProject/` (the repo-level
`DecryptProject/` is gitignored and *absent*; use the scratch-stash copy).

```bash
cd /var/home/j/cloudy-af/scratch-stash/DecryptProject
JAVA_HOME="$PWD/tools/jdk-21" ghidra_12.1.2_PUBLIC/support/analyzeHeadless \
  $PWD/ghidra-projects AFSamples -process af_190602.dec.bin -noanalysis \
  -scriptPath $PWD/sN -preScript PO4
```

**Hard-won gotchas (do not repeat):**
- Scripts must be in a *separate* directory per script run (`s2/`, `s3/`, ...). Ghidra binds an
  OSGi bundle to a script dir; editing/multi-scripting the same dir produces a cryptic
  `ClassNotFoundException: Failed to get OSGi bundle containing script`.
- `var` is **not** allowed as a method parameter (ECJ rejects it; hidden behind the OSGi
  message). Use the full type.
- `listing.getInstructionAfter(Address)`, not `getInstructionAfter(Instruction)`. The generic
  OSGi error hides ECJ errors — pre-compile every script with JDK-21 javac against the Ghidra
  jars to surface the real error:
  ```bash
  CP=$(find ghidra_12.1.2_PUBLIC/Ghidra/Framework ghidra_12.1.2_PUBLIC/Ghidra/Features -name '*.jar' | tr '\n' ':')
  ./tools/jdk-21/bin/javac -proc:none -d out -cp "$CP" sN/PO5.java
  ```
- Use a fresh `sN/` dir + delete `~/.config/ghidra/ghidra_12.1.2_PUBLIC/osgi` per run.
- Disassembly text dump `af190602_dis.txt` (repo root) is queryable locally for fast grep
  without Ghidra.

---

## 2. Screen dispatcher `FUN_0000d684` (the UI render orchestrator)

`FUN_0000d684` is the top-level screen render dispatcher. Called by the main UI loop blocks at
`0x183ac/0x183bc/0x1879c/0x1887e`, by `FUN_00014690` (large UI task) at `0x151c0/0x15640`, and
by `FUN_0000db5c` at `0xdbc6/0xdbe8`.

Structure:
- Prologue reads a **screen-mode byte** and a **state counter**; branches on flags in the
  device-status word `[r4]` (`0x20000`, bit-1, `0x200000`, ...).
- **TBH #1** at `0xd738` (`tbh [pc,r3]`, bound `cmp r3,#0x24` = 37 entries) — full 37-entry
  jump table recovered (see §table).
- **TBH #2** at `0xd7d8` (bound `cmp r3,#0x25`) — dispatches screen-render subroutines.
- Charge screen path: state ≤ `0x31` at `0xd79c` → `bl 0x0000b53c` (charge composer).
  Screen 0 `[0]→0xd79c`.
- Most screen states funnel into text/blit helpers: `0x138ec` (text/draw), `0x13bf4`
  (post-render commit / selector), `0x13be0`, `0xd04c` (layout blit wrapper), `0x13c44`.

### Recovered 37-entry jump table @ 0xd73c (state → target)

```
0→0xd79c   1→0xd828  2→0xd830  3→0xd7a6  4→0xd864  5→0xd9b0  6→0xd970
7→0xd9a4   8→0xd840  9→0xd894 10→0xd87e 11→0xd86e 12→0xd9c4 13→0xd878
14→0xd998 15→0xd836 16→0xd9aa 17→0xd89e 18→0xd89e 19→0xd986 20→0xd928
21→0xd944 22→0xd944 23→0xd95a 24→0xd840 25→0xd992 26→0xd99e 27→0xd98c
28→0xd9ba 29→0xd9cc 30→0xd9d6 31→0xd9e2 32→0xd9dc 33→0xd9e8 34→0xd9ee
35→0xd9f4 36→0xd99e
### Screen-render subroutine targets (from the 0xd7d8 TBH branches)

| fn address | role (deduced) | fn address | role |
|---|---|---|---|
| `0xd438` | screen A layout | `0xc194` | screen anim/menu A |
| `0xc904` | screen B | `0xcb20` | screen C |
| `0xca00` | screen D | `0xbd74` | screen E |
| `0xbe34` | screen F | `0xbc70` | screen G |
| `0xc120` | screen H | `0xb6f8` | screen I |
| `0xf518` | screen J | `0xb848` | screen K |
| `0xc098` | screen L | `0xcd98` | screen M |

These are the individual screen render routines the effect-injection work must sit beside.

---

## 3. Differential analysis for the animation effect design (Gradient Fade, Phase 3 Task 5)

The point of this RE is to let the new injected effect **match the firmware's real render
cadence and buffer semantics**.

### 3.1 Buffer & render-framing facts the effect must respect
- Display buffer: `0x20002758–0x20002b58`, 1024 bytes = 64×128 @ 1bpp (pixel
  `x+(y/8)*width`, bit `y%8`).
- Render entry for the harness: `0x00008cd1` (Thumb addr of inner fill body); composer
  `FUN_0000b53c` @ `0xb53c`.
- The dispatcher calls the composer once per frame; the composer's multi-path branches
  (`0x8e5c/0xa880/0x9a10/0xa47c/0xb458`) route on device-status bits — the same bit-flag
  pattern Task 5's effect must not clobber.

### 3.2 How the *existing* screens animate — the pattern to extend
- No timer-based pacing: the dispatcher runs per main-loop tick, advancing a **state counter**
  (`[r6]`) and a **16-bit timer/phase word** (`[0x2000...]` at `0xd8cc`), decremented at
  `0xd9fa`/`0xda00`. Pseudocode:
  ```
  d7bc: if (state <= 9) -> dAD0 (advance/proceed)
  d7c8: phase16 = *(u16*)P
  d7ca: if (phase16 != 0) -> d9fa (phase16--; if !=0 -> d7d0 re-dispatch)
  d7d0: screen = *(u8*)M; tbh [pc,screen]
  ```
  This is exactly the "advance state, gate on a countdown, re-dispatch" loop that Task 5's
  `gradient_fade_apply` phase stepping must mirror (phase bumps once per render call; no
  wall-clock timer).
- `0x13bf4` (post-render commit) branches on a **screen-mode byte**
  `0x30/0x01/0x20/0x64-0x65/0xc8-0xc9` — a bank of screen IDs an injected effect can reuse
  without touching the dispatcher's jump tables.

### 3.3 Where the power-off / CRT-collapse plug-in point is
- The user reports a **"CRT Power Off"** effect at power-off. The firmware's power-off path
  sets the screen state and/or a device-status **shutdown bit**, then the main-loop renderers
  fade/clear the display. In the dispatcher, the **device-status word `[r4]`** (`0x100`,
  `0x20000`, bit1, `0x20000000`, ...) gates several branches — one of these masks is the
  "shutting down" flag that should trigger a collapse/blank sequence.
- Design implication: the injected Gradient Fade (and a future CRT-collapse) should **bit-and
  into the final committed frame** at the `0x13bf4` commit seam, keyed on the same
  device-status **shutdown bit**, so it layers naturally over the stock charge screen without
  touching the 37-entry dispatcher.
- Recommended verification hook: set the shutdown bit + a screen state that still routes to
  `FUN_0000b53c` (charge) so the effect runs over the reflashed charge screen exactly as the
  hardware power-off does.

---

## 4. Open items / next steps
- [x] Identify the exact device-status **shutdown bit** (see **§5** — the effect is gated on the
      device-status word bitfields `0x100`/`bit1`/`0x20000000` and the **screen-mode byte**).
- [x] Map the 14 screen-render functions (`0xd438`…`0xcd98`) — all are **static** status/menu
      screens (text via `0x138ec`/`0x13730`), not effects. The **transition/effects** are a
      separate subsystem via the screen-mode byte (`0x30/0x01/0x20/0x64-0x65/0xc8-0xc9`) — §5.
- [x] Confirm no VFP in the charge-screen frame path (Thumb-2 integer only) so the emulator
      gate stays float-free — **confirmed 2026-09-09**: the layer-4 gate runs 7394 steps over
      4 frames with zero undefined-instruction faults and no VFP hit. The effect primitives
      in §5.3 are all integer ops (no VFP), as suspected.
- [ ] Feed the recovered dispatcher → composer topology into
      `resources/animations/af_190602.json` as a `screens` extension so Task 5/8 can pick the
      correct hook site by screen state.

---

## 5. The screen-transition / "CRT Power-Off" effect subsystem (RE found it)

The 14 screen-render functions (§table) are **static** screens. The power-off (and power-on)
**animation** is a separate subsystem selected by a **screen-mode byte** and driven through the
post-render funnel `0x13bf4` → per-mode renderers.

### 5.1 Screen-mode → effect routing (`0x13bf4`, the post-render commit selector)
```
mode <= 0x30                                -> 0x12970   (+ 0x01, 0x20)
mode 0x64, 0x65  OR  mode 0xc8, 0xc9        -> 0x1311c
everything else                             -> return (commit)
```
So mode IDs **`0x01/0x20/0x30`**, **`0x64/0x65`** and **`0xc8/0xc9`** are the animation/effect
screen IDs. `0x64/0x65` and `0xc8/0xc9` are the dual-end (on/off) of transition pairs — the
power-off "CRT Power Off" effect is one of these screen-mode IDs.

### 5.2 Per-frame animation loop (`0x12970`, the effect driver)
```
r5 = iterations (4 / 6 / 16 depending on mode); r4 = 0
loop:
  bl 0x1333c      ; per-frame effect draw (writes a frame-param byte, routes by mode)
  ... update frame pointer  r8 = r9 + r4*0x40   ; 64-byte stride per frame step
  bl 0x13388      ; effect blit (0x40 = 64-byte row transfer)
  r4++
  if r4 != r5 -> loop
```
This is the **progressive multi-frame transition**: each main-loop render advances `r4`,
redrawing the display with a stepped geometry. **This is the "advance one step per render call,
no wall-clock timer" pattern the injected Gradient Fade must mirror** — identical to the
dispatcher pacing sauce in §3.2.

- `0x1333c` (per-frame draw) routes on the same mode byte (`0x30→0x12c88`, `0x01→0x12f64`,
  `0x20/0x64/0xc8→0x13094`).
- `0x13388` (blit) row-transfers 0x40 bytes (`0x30→0x12c88 0x40`, else `0x13094/0x12f64`).

### 5.3 The collapse/expansion primitive (`0x12f64`, screen-mode 0x01 — CRT-geometry core)
```
for each source byte r6 = [r5++], over r2 bytes:
  movs r4,#0x7 ; r4=7          ; bit plane 7 (MSB)
  lsl.w lr, r12(r=1), r4        ; lr = bit mask (0x80)
  tst.w lr, r6                 ; source bit set?
   - yes: r7 = 0x0F ;  no: r7 = 0x00
  ands.w lr, r6, lr lsr #1     ; peek next-lower bit
   - if set: r7 |= 0xF0
```
Each **1bpp source bit maps to a 4-bit nibble** (`0x0F`/`0xF0`, or combined `0xFF`) — a
per-pixel horizontal **expansion/scale**, i.e. the frame is re-laid out per animation step.
Paired with the step geometry in `0x12c88`/`0x13094`, this is the **CRT-collapse** sweep:
each step redraws the frame at a different (typically shrinking row-band / expanding line)
mapping until the display goes black at power-off.

- `0x12c88` (mode `0x30`) and `0x13094` (modes `0x20/0x64/0xc8`) write the **display-controller
  command word** (`0x200000` → `[base+0x84]`) and step a row/scroll counter per frame — the
  hardware address/show-window being updated each step.

### 5.4 Differential-analysis takeaways for the injected effect(s)
1. **Reuse the mode-byte funnel** — pick an unused screen-mode ID and hook `0x13bf4`
   (and `0x1333c`/`0x13388`) rather than the 37-entry dispatcher. The firmware already has a
   clean "effect" plumbing seam.
2. **Match the geometry language** — the CRT effect is bit-(un)packing + row-band stepping,
   all **integer** (shift/and/or per byte, `0x40` byte rows). The Gradient Fade should use the
   same vocabulary (integer sine LUT + per-`0x40`-byte-row threshold sweep).
3. **Pace by render-call, not timer** — the effect driver advances one iteration per render
   call with the phase in a global; the injected effect's `gradient_fade_apply(buf, w, h,
   phase)` should be invoked exactly once per render pass, matching the per-frame `r4++`.
4. **Buffer is 1bpp packed** — the `0x12f64` nibble expansion implies the source state is
   bit-packed; the effect must operate on the 1024-byte `0x20002758` framebuffer in that same
   1bpp format (pixel `x+(y/8)*width`, bit `y%8`).

---

## 6. Concrete facts lock-in (for Task 5/8 hook-site selection)

| item | value |
|---|---|
| Effect funnel (commit seam) | `0x13bf4` (branch on screen-mode byte) |
| Effect driver (per-frame loop) | `0x12970` (r4 = 0..5, stride `0x40`) |
| Per-frame draw / blit | `0x1333c` / `0x13388` |
| CRT expansion core | `0x12f64` (1bpp bit→nibble, screen-mode `0x01`) |
| Scroll/row-step core | `0x12c88`, `0x13094` (display-cmd `0x200000` → `[disp+0x84]`) |
| Transition screen-mode IDs | `0x01`, `0x20`, `0x30`, `0x64`, `0x65`, `0xc8`, `0xc9` |
| Row size | `0x40` = 64 bytes (8 rows of bytes per step) |

---

## 7. Show-clock-on-timeout pathway (the injection route for a CRT shutoff patch)

**User idea:** enact the CRT shutoff animation on *screen timeout* by riding the firmware's
existing show-clock-on-timeout pathway. RE confirms this is the right seam — the firmware
already has a complete "on timeout, switch what the render loop draws" mechanism.

### 7.1 The timeout lifecycle (fully traced)

1. **Idle counter** — dispatcher `FUN_0000d684` at `0xd6b6–0xd6c4`: every main-loop tick
   decrements a 16-bit timeout counter (pointer at literal `0xd8d0`). When it reaches 0
   (`beq 0xd6ac`) the timeout fires.
2. **Timeout fires** — `0xd6ac–0xd6b4`:
   ```
   d6a0: strh r3,[r2]          ; save the new timeout value (re-arm)
   d6a2: [0xd8cc] = 1          ; phase16 gate = 1  (forces a re-dispatch next tick)
   d6aa: [r6] = 0              ; state/phase counter reset to 0
   d6ae: [r4] |= 0x20000       ; device-status bit 17 = "screen timed out"
   ```
3. **Next render pass** — the charge composer `FUN_0000b53c` routes to the clock renderer
   **`FUN_00009a10`**, which at `0x9a16` tests bit `0x20000`:
   - **clear** (`beq 0x9a32`): draws the **clock** (big time widget via `0x136b4`,
     `0x90`, `0xb230`, `0x13760`, `0x13798`).
   - **set** (`0x9a20 → 0x9b48`): takes the dim/idle branch (screen-off path).
4. **Wake** — a button press clears `0x20000` (consumed at `0x15538` in `FUN_00014690` and
   re-tested at `0x9a16`), returning to normal rendering.

So the "show clock on timeout" behaviour is: **timeout → `0x20000` raised → the render loop
keeps calling `FUN_00009a10`, which now takes the timeout branch.** The bit persists across
frames until a wake event — giving a multi-frame window with `[r6]` (state counter) and
`phase16` (`0xd8cc`) available as free-running animation phase inputs.

### 7.2 Why this is the right seam for a CRT-shutoff-on-timeout patch

- **No dispatcher surgery needed.** The 37-entry jump tables stay untouched; the patch
  detours a single decision point.
- **The firmware already paces it.** The dispatcher resets `[r6]=0` and arms `phase16` on
  timeout, so the cave body gets a per-frame phase counter for free — exactly the cadence the
  stock transition effects (`0x12970`, §5.2) use.
- **The collapse primitive exists in-ROM.** `0x12f64` (§5.3) is the CRT bit-expansion core;
  the patch can either re-implement a small integer collapse in the cave or `bl` into the
  stock routine with the right mode byte.

### 7.3 Candidate hook sites (ranked)

| site | where | semantics | notes |
|---|---|---|---|
| **A** `0x9a16` | `FUN_00009a10` `tst r2,#0x20000` | "timeout? then draw X" | Cleanest: 4-byte `B.W` detour over `tst`+`beq` into the cave; cave runs the collapse for `phase=[r6]` frames, then resumes at `0x9a20` (timeout branch) or `0x9a32` (clock). |
| **B** `0xd6ae` | dispatcher `orr r3,r3,#0x20000` | "timeout just fired" | Fires once per timeout (edge), not per frame — good for *starting* a collapse sequence, but the per-frame drawing still needs site A or the phase16 gate. |
| **C** `0x13bf4` | post-render commit selector | "every frame, by mode byte" | Most general (§5.1) but touches the shared funnel — higher regression risk than A. |

**Recommendation: site A** (`0x9a16`), with the cave body:
1. read `[r6]` (phase) → run one step of the CRT collapse over the framebuffer
   (`0x20002758`, 1024 bytes, 1bpp) using the §5.3 integer bit-geometry;
2. increment `[r6]`;
3. when the collapse completes (all rows collapsed), fall through to the stock timeout branch
   (`0x9a20`) so the device still dims/sleeps normally afterwards.

### 7.4 Patch-mechanics notes (for the Task 4/5 builder)
- **Exact bytes at the hook site** (verified against `af_190602.dec.bin`):
  ```
  0x9a16: F412 3F00   tst.w r2, #0x20000    ; 32-bit Thumb-2, 4 bytes
  0x9a1a: B085        sub  sp, #0x14
  0x9a1c: 461D        mov  r5, r3
  0x9a1e: D008        beq  0x9a32           ; 16-bit, +8 -> clock branch
  ```
  The decision point is **6 bytes** (`tst.w` + `beq`). A 4-byte `B.W` detour can overwrite
  just the `tst.w` at `0x9a16` (the cave replays `tst.w r2,#0x20000` then branches back to
  `0x9a1a`), or overwrite `0x9a16–0x9a1f` with `B.W` + `NOP` and replay both instructions.
  Use `bw_pair(0x9a16, cave)` from the Task 4 builder.
- Dispatcher timeout-set site bytes (site B): `0xd6ae: F443 3300` = `orr.w r3,r3,#0x20000`,
  `0xd6b2: 6023` = `str r3,[r4,#0]`.
- The cave body is pure Thumb-1 integer code (shift/and/or per byte over `0x40`-byte rows) —
  within the existing `Asm` builder's instruction set (`lsls/lsrs/ands/movs/ldr/str/b/bcond`).
- Framebuffer format: 1bpp packed, pixel `x+(y/8)*width`, bit `y%8` (§3.1) — same as the
  stock collapse primitive expects.
- Config byte: reuse the planned animation config byte (dataflash) to select
  `0=off / 1=clock (stock) / 2=CRT shutoff` so the patch is toggleable without reflashing.
- **Code-cave scan (2026-09-09, byte-verified):** there is NO in-image `0xFF` run of
  ≥256 bytes (the longest is 58 bytes at `0x1a51e`), so the cave cannot live inside the
  stock image. The image ends at `0x1c7ec` and dataflash starts at `0x1F000`, leaving
  the APROM tail `0x1c7ec..0x1F000` (~9.9 KB, all `0xFF`) as cave space — but the
  patched image must GROW to cover it. Open verification: confirm the flasher/LDROM
  uploader tolerates an image larger than the stock 0x1c7ec bytes (or pad to a fixed
  size the bootloader already accepts).
```