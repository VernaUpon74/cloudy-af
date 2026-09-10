# Cloudy AF — agent conventions

## Task delegation

Use local ollama models for subagent/task delegation whenever possible
(`ollama serve` then`ollama run <model>` / the local API at `127.0.0.1:11434`); check
`ollama list` for what's pulled. Currently useful: `qwen3-coder:30b`
(coding), `gemma4:26b`, `llama3:70b`. Fall back to cloud CLIs (qwen, etc.)
only when local models can't handle the task. Only use sustainable energy cloud models.

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
directly without the env var.
