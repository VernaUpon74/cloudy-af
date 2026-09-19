# Cloudy AF — agent conventions

## Delegation hardware profile

The workstation: 12 CPU cores, 62 GB RAM, RTX 4050 Laptop with only 6 GB
VRAM. There are NO cgroup limits on ollama (CPUQuota/MemoryMax = infinity) —
delegates already get full hardware access, but any model larger than ~4.5 GB
quantized spills to CPU (the 20 GB qwen3-coder runs 81/19 CPU/GPU and is
slow). Prefer `qwen2.5-coder:7b` (fits VRAM, ~5-10× faster) for mechanical
tasks; reserve the 30B for genuinely open-ended analysis. Cloud models are
not to be used (see energy policy above). RAM cannot substitute for VRAM on
this hardware (CUDA has no unified-memory mode; Intel iGPU would need the
IPEX-LLM fork).

## Task delegation

**Convention: run local-model delegates ONE AT A TIME.** The box only has
headroom for a single ollama inference (18 GB models); parallel cline runs
thrash and stall. Queue tasks sequentially through a runner script
(e.g. `/tmp/cline-queue.sh`: a `for` loop invoking `cline` per task, each
appending to its own log, `setsid nohup` so it survives the agent session).
Launch exactly one queue, never parallel cline instances.

Use local models for subagent/task delegation whenever possible —
`cline` CLI (free models, `/var/home/j/.npm-global/bin/cline`, e.g.
`cline -c <repo> "task"` with auto-approve) for simple, well-specified
tasks (UI markup, mechanical ports from a complete spec, drafting
tests), and ollama (`ollama serve` then`ollama run <model>` / the local
API at `127.0.0.1:11434`) for drafting and review work; check
`ollama list` for what's pulled. Currently useful: `qwen3-coder:30b`
(coding), `gemma4:26b-chat`, `llama3:70b`. Fall back to cloud CLIs
(qwen, etc.) only when local models can't handle the task. Only use
sustainable energy cloud models. Reserve full agent dispatches for
multi-step work that needs tool use across the repo.

## node-hid native addon

The sidecar's `node-hid` carries a local patch
(`sidecar/patches/node-hid+2.2.0.patch`, applied by patch-package on
`npm install`) that mutex-serializes `close()` against the AsyncWorker read
thread — without it, closing the hidraw handle with a read in flight aborts
(SIGABRT in HID_hidraw.node, "free(): invalid pointer"). The patch changes
C++, so after any sidecar `npm install` the addon MUST be rebuilt with the
toolbox node (v22, same ABI as the flatpak's `/app/bin/node`):

```
toolbox run -c arcticfox-build sh -c \
  'export PATH=/usr/bin:$PATH LIBRARY_PATH=/tmp/fakelib:$LIBRARY_PATH && \
   cd sidecar/node_modules/node-hid && \
   node-gyp configure --nodedir=/tmp/node22-headers && \
   make -C build HID_hidraw BUILDTYPE=Release'
```

(/tmp/node22-headers is the unpacked node-v22 headers tarball;
/tmp/fakelib holds a `libusb-1.0.so` symlink to the runtime `.so.0` — the
toolbox lacks the -devel package. The shipped binary is `build/Release/
HID_hidraw.node`; the `HID.node` libusb variant is unused on Linux.)

## Version bumps

Whenever README, CHANGELOG, or docs are updated for a change, bump the app
version in the same commit. All of these must move together:

- `package.json`
- `sidecar/package.json` and `sidecar/package-lock.json` (2 spots)
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock`
- `flatpak/org.cloudy.af.appdata.xml` (add a `<release>` entry)
- `CHANGELOG.md` (retitle `## Unreleased` to `<version> — <date>`)

Cadence: patch for fixes (1.15.x), minor for user-facing features (e.g.
1.14.0, 1.16.0).

## Flatpak release builds

`flatpak/org.cloudy.af-local.yml` does NOT compile the app — it installs the
Tauri binary prebuilt at `src-tauri/target/release/cloudy-af`. The binary
embeds the frontend `dist/` at compile time, so the exact release sequence is:

1. `npm run build` (host — rebuilds `dist/`)
2. `toolbox run -c arcticfox-build sh -c 'cargo build --release --offline --features tauri/custom-protocol'`
   from `src-tauri/`. The `--features tauri/custom-protocol` flag is
   MANDATORY (ADR-007): without it the binary is a dev-mode build that tries
   to load the frontend from `http://localhost:1420` and the flatpak shows
   `GDBus.Error:org.freedesktop.portal.Error.NotAllowed` at startup.
3. `flatpak-builder --disable-rofiles-fuse --repo=repo --force-clean build-dir flatpak/org.cloudy.af-local.yml`
   (`--disable-rofiles-fuse` is required on this system; plain runs fail at
   cleanup with a rofiles-fuse Permission denied error)
4. `flatpak build-update-repo repo`
5. `flatpak build-bundle repo builds/cloudy-af-<version>.flatpak org.cloudy.af master`
6. Reinstall: `flatpak uninstall --user org.cloudy.af` then
   `flatpak install --user builds/cloudy-af-<version>.flatpak`
   (the `af2-origin` bundle remote stays disabled; system installs need sudo
   we don't have).

Gotcha: if step 2 is skipped or run without the feature flag, step 3 silently
repackages the stale binary — the bundle gets new metadata with the OLD app.
Symptom: installed version looks updated but behavior/UI is outdated, plus the
portal error above.

## Release artifacts

Every version build must leave ALL artifacts in `builds/` (gitignored):

- `builds/cloudy-af-<version>.flatpak` — the bundle (step 5 above)
- `builds/cloudy-af-<version>` — the raw release binary:
  `cp src-tauri/target/release/cloudy-af builds/cloudy-af-<version>`
- `builds/Cloudy AF_<version>_amd64.AppImage` — when an AppImage is built
  (see below)

## AppImage builds

The stock `tauri build --bundles appimage` output is NOT usable as-is: it
bundles the system GL/EGL stack, which crashes WebKitWebProcess at exit on
NVIDIA hosts (SEGV in libnvidia-gpucomp), it drops a broken `put` symlink
into the sidecar node_modules, it lacks a `node` binary for the sidecar,
and extract-and-run doesn't put `usr/bin` on PATH. The working procedure
(all steps inside `toolbox run -c arcticfox-build`, from the repo root):

1. Regenerate the AppDir (tauri's own linuxdeploy step fails — ignore it):
   `APPIMAGE_EXTRACT_AND_RUN=1 npm run tauri:build -- --bundles appimage`.
   The AppDir is single-use — NEVER re-run linuxdeploy on an already
   processed AppDir; regenerate instead. The AppDir is at
   `src-tauri/target/release/bundle/appimage/Cloudy AF.AppDir/`.
2. `cp -L /usr/bin/node "<AppDir>/usr/bin/node"` — must be `-L` (the RPM
   x86_64 binary). Do NOT use `~/.local/bin/node` (symlink into ~/.hermes;
   its foreign-arch siblings break appimagetool arch detection).
3. Fix the `put` symlink tauri drops:
   `rm -f "<AppDir>/usr/lib/Cloudy AF/sidecar/node_modules/put" && cp -r sidecar/put-replacement "<AppDir>/usr/lib/Cloudy AF/sidecar/node_modules/put"`.
4. Run the extracted, patched linuxdeploy with the GL libs EXCLUDED (this is
   the NVIDIA-webprocess fix — the AppImage must use the host's GL drivers):
   ```
   SQ=/var/home/j/.cache/tauri/squashfs-root
   PATH=$SQ/usr/bin:$PATH LD_LIBRARY_PATH=$SQ/usr/lib:$LD_LIBRARY_PATH \
     APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 \
     $SQ/usr/bin/linuxdeploy --output appimage --plugin gtk \
     --exclude-library "libGL*" --exclude-library "libEGL*" \
     --exclude-library "libGLES*" --exclude-library "libgbm*" \
     --exclude-library "libdrm*" \
     --appdir "<AppDir>" -e target/release/cloudy-af \
     -d "<AppDir>/Cloudy AF.desktop" -i "<AppDir>/Cloudy AF.png"
   ```
   `ARCH=x86_64` is required because node-hid prebuilds contain
   foreign-arch ELFs. One-time prerequisite: replace the bundled strip with
   the system one (it crashes on `.relr.dyn`):
   `cp /usr/bin/strip $SQ/usr/bin/strip` (backup the bundled one first).
5. Add a PATH hook so the runtime finds the bundled node: write
   `<AppDir>/apprun-hooks/zz-cloudy-af.sh` (mode +x):
   ```sh
   APPDIR="${APPDIR:-"$(dirname "$(realpath "$0")")"}"
   export PATH="$APPDIR/usr/bin:$PATH"
   ```
6. Repack:
   `APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 $SQ/plugins/linuxdeploy-plugin-appimage/usr/bin/appimagetool "<AppDir>"`
   → produces `Cloudy_AF-x86_64.AppImage`; copy it to
   `builds/Cloudy AF_<version>_amd64.AppImage`.

Verification on this host (no FUSE): run with
`APPIMAGE_EXTRACT_AND_RUN=1 ./builds/...AppImage`. The main process shows in
`ps` as plain `cloudy-af` (the runtime re-execs it); the sidecar appears as
`node /tmp/appimage_extracted_*/usr/lib/Cloudy AF/sidecar/hid-bridge.js`.
Both alive + empty stderr = good. On FUSE-equipped systems the AppImage runs


## i18n: tooltips and translations are mandatory for new UI

Whenever you add text, a button, a field, a tab, or any user-visible feature:

1. Give every new element a `data-lang` key; give interactive elements
   (buttons, selects, tabs, rows) a `data-lang-title` accessibility tooltip
   too. Keep tooltips user-meaningful sentences, not key names.
2. Add the new keys to **all 17** files in `public/i18n/` (`en`, `cn`, `cs`,
   `cz`, `de`, `es`, `fr`, `hu`, `it`, `ja`, `nl`, `pl`, `ru`, `sk`, `sr`,
   `tr`, `ua`) — never `en` alone, never leave keys missing: a missing key
   renders as **empty text** at runtime (`if (phrase)` skips the element).
3. Locale files are two-letter codes; `getLocale()` returns full BCP-47 tags
   (`de-DE`) — always normalize with `.substr(0, 2)` before building the
   `i18n/<locale>.json` path (monitor page comment documents this; the
   firmware/tfr pages were fixed after shipping an entire release that could
   not switch languages).
4. Before committing, verify: no `data-lang*` key missing from `en.json`, and
   all 17 locale files have identical key counts. A one-liner check lives in
   `scripts/check-i18n.py`.

## Rebuild after translations

`scripts/translations-watch-rebuild.sh` polls `public/i18n/`; as soon as
translation changes go 60 s quiet it rebuilds the Rust binary (toolbox),
the Flatpak (`--disable-rofiles-fuse` — see above), and exports the bundle +
binary byproducts into `builds/`. Run it in a terminal while translating:

```
scripts/translations-watch-rebuild.sh [--flatpak]
```

Without `--flatpak` it rebuilds only the host binary. With it, the full
Flatpak + bundle pipeline runs. Stop with Ctrl-C; every stage logs a clear
FAIL line if it breaks.

## Flatpak local builds

Always pass `--disable-rofiles-fuse` to `flatpak-builder` on this host —
`rofiles-fuse` fails with `fusermount3: failed to access mountpoint ...
Permission denied` on the btrfs/homed mount, killing the final commit stage
(exit_status: 1024) after minutes of otherwise-good work:

```
flatpak-builder --force-clean --disable-rofiles-fuse --repo=repo build-dir flatpak/org.cloudy.af-local.yml
```

Also: if a build dies with `Error: opendir(refs/heads): No such file or
directory`, the previous `build-dir/` is stale/partial (flatpak-builder is
treating it as an OSTree repo) — delete `build-dir/` (and `.flatpak-builder/`
if suspicious) and rebuild. Export a single-file bundle with:

```
flatpak build-bundle --runtime-repo=repo repo builds/cloudy-af-<version>.flatpak org.cloudy.af
cp src-tauri/target/release/cloudy-af builds/cloudy-af-<version>
```

Build the Rust binary inside the `arcticfox-build` toolbox (host cargo cannot
find the gtk pkg-config files):
`toolbox run -c arcticfox-build sh -c 'cd src-tauri && PKG_CONFIG_PATH=/usr/lib64/pkgconfig:/usr/share/pkgconfig cargo build --release --features tauri/custom-protocol'`

directly without the env var.
