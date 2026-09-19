# T3 — AppImage device-detection flicker: root-cause analysis

> Reconstructed 2026-09-15 in-repo (the original delegated report at
> `/tmp/cline-tasks/T3-report.md` was lost when /tmp was cleared).
> Analysis from source only; on-device verification still pending.

## Symptom
In the packaged AppImage the device-detection / connection status flickers
(connected ↔ disconnected churn), unlike a dev run. Tracked in
`docs/goals.md` ("AppImage device-detection flicker", delegate follow-up).

## Architecture (facts from source)
- Rust backend spawns the Node sidecar `sidecar/hid-bridge.js`
  (`spawn_sidecar`, `src-tauri/src/lib.rs:498`); sidecar talks over
  stdin/stdout JSON lines.
- Sidecar uses node-hid **hidraw** backend on Linux so hotplugged devices
  are visible (`setDriverType('hidraw')`, `hid-bridge.js:23`). The node-hid
  addon carries the local mutex patch (`sidecar/patches/node-hid+2.2.0.patch`)
  that serializes `close()` against the AsyncWorker read thread.
- Bridge-side reconnect loop: `RECONNECT_INTERVAL_MS = 2000`
  (`hid-bridge.js:51`). Every 2 s, while not connected, it re-attempts
  `fox.connect()`.
- Bridge-side "unresponsive probe": a device that opens but does not answer
  the ArcticFox protocol (stock firmware) keeps its handle open and is
  re-probed on the same 2 s timer (`scheduleUnresponsiveProbe`,
  `hid-bridge.js:148`), each cycle re-emitting `connect(false)`.
- Status plumbing: `emit('connect', status)` → Rust `ipc-event` →
  renderer `src/renderer.js:14` rewrites `#connection-status` on **every**
  event, with no edge-detection or debounce.
- Startup: the Rust side previously issued the connect command after a
  **fixed 1500 ms sleep** instead of waiting for the sidecar's `ready`
  event (`lib.rs` setup closure) — a race against slow sidecar startup.

## Root causes (ranked)
1. **Blind startup race (fixed this session).** The 1500 ms sleep could
   issue `connect` while the sidecar was still loading node-hid (cold page
   cache on first AppImage run = slow start). The command then landed
   mid-probe-cycle: connect attempts and the first probe timer overlapped,
   producing repeated connect/close cycles before settling — every one
   repainting the status bar. Replaced with event-driven autoconnect on the
   bridge's existing `ready` announcement (`lib.rs`, sidecar reader loop).
2. **Timer-vs-timer duplication (contributor).** `scheduleReconnect()` and
   `scheduleUnresponsiveProbe()` both (re)arm a 2 s timer on every event;
   connect errors, `close`, and probe timeouts can re-arm each other's
   timers. `clearReconnectTimer()` guards a single slot, so a probe and a
   reconnect could interleave in the same window (probe timeout →
   `scheduleUnresponsiveProbe` → `downloadConfig` retry → `Error: timeout`
   → `emit connect(false)` again), each emitting a status event.
3. **Unresponsive devices re-emit `connect(false)` forever (by design, but
   UI-visible).** For a stock-firmware or flaky device, every 2 s cycle
   writes a fresh "Disconnected" status. If the device then half-answers
   (marginal USB contact), the sequence `probe → timeout → connect(false) →
   success → connect(true) → timeout → …` is exactly observed flicker.
4. **Every event repaints the status bar (amplifier).** `renderer.js:14`
   has no last-state check, so redundant events still cause DOM writes and
   visible churn.

## Fix applied (this session)
- `src-tauri/src/lib.rs`: autoconnect now sent on sidecar `ready` event;
  removed the fixed 1500 ms sleep (comment explains the race).

## Follow-ups applied (2026-09-18)
1. **Renderer edge-detect** (`src/renderer.js:14`): added `lastConnectStatus`
   guard — DOM is only rewritten when the status actually changes, eliminating
   UI churn from redundant events.
2. **Bridge first-failure guard** (`sidecar/hid-bridge.js:228`): the
   unresponsive-probe path now only emits `connect(false)` on the *first*
   timeout failure (`firstFailure` flag). Subsequent probe failures keep
   the existing "Disconnected" status without re-emitting, preventing the
   `probe → timeout → emit(false) → probe → …` flicker loop.
3. **ENOENT/ENODEV**: already handled — `fox.connect()` failures in the
   reconnect loop don't emit `connect(false)`; only `onClose()` (device was
   opened then closed) and `downloadConfig()` timeout (device opened but
   unresponsive) emit status, both of which are genuine state changes.

## Recommended follow-ups (not yet applied)
1. Renderer: edge-detect the connect event — only update
   `#connection-status` when `status` actually changed (1-line guard in
   `src/renderer.js:14`), eliminating UI-level flicker from redundant
   events.
2. Bridge: only emit `connect(false)` on the *first* failure of an
   unresponsive device (the `firstFailure` flag already exists in
   `downloadConfig` — extend it to the periodic probe path), instead of
   every 2 s cycle.
3. Bridge: when opening the handle for a fresh connect attempt fails with
   ENOENT/ENODEV (device absent), skip emitting status entirely — absence
   is not a state change.

## Verification (pending hardware)
- `APPIMAGE_EXTRACT_AND_RUN=1 ./builds/…AppImage` with the Pico plugged in:
  status must settle to "Connected" within ~1 s of sidecar `ready` and not
  churn while idle.
- Unplug/replug: exactly one Disconnected → Connected transition pair.
- Stock-firmware device (rescue kit in `test-fixtures/rescue/`): a single
  "Disconnected", not a periodic repaint.
