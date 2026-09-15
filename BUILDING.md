# Building Cloudy AF

Desktop configurator for ArcticFox/NToolbox-compatible vaping devices
(Tauri v2 + WebKit frontend, Node.js HID sidecar).

> **Distribution delta:** the Firmware Editor's *Patches* tab is disabled in
> this tree (the tab and panel are removed from `firmware.html` and
> `refreshPatches()` early-returns in `src/renderer-firmware.js`) while the
> animation-patch feature is under development. It stays enabled in the main
> development tree. Re-apply this edit after every source refresh.

## Prerequisites

- Node.js 22+ and npm (frontend + sidecar)
- Rust toolchain (src-tauri)
- For flatpak: flatpak-builder, and builds run inside a toolbox/container
  with the distro toolchain (e.g. `toolbox run -c arcticfox-build`)

## Setup

```sh
npm install                      # frontend
cd sidecar && npm install        # sidecar; patch-package applies the node-hid patch
```

The sidecar's `node-hid` carries a local patch
(`sidecar/patches/node-hid+2.2.0.patch`) that mutex-serializes `close()`
against its read thread — without it, closing the hidraw handle with a read
in flight aborts the sidecar (SIGABRT, "free(): invalid pointer"). After any
sidecar `npm install` the native addon MUST be rebuilt with the same Node
major version that ships with the app (22):

```sh
cd sidecar/node_modules/node-hid
node-gyp configure --nodedir=<path-to-node-22-headers>
make -C build HID_hidraw BUILDTYPE=Release   # shipped binary: build/Release/HID_hidraw.node
```

(`HID.node`, the libusb variant, is unused on Linux.)

## Dev build

```sh
npm run dev        # vite dev server on :1420 + cargo tauri dev
```

## Release build (flatpak)

`flatpak/org.cloudy.af-local.yml` installs a prebuilt Tauri binary; the binary
embeds `dist/` at compile time. Exact sequence:

1. `npm run build` — rebuild `dist/`
2. `cargo build --release --features tauri/custom-protocol` (in `src-tauri/`,
   offline OK). The `--features tauri/custom-protocol` flag is MANDATORY:
   without it the binary is a dev-mode build that loads the frontend from
   `http://localhost:1420` and the flatpak fails at startup with
   `GDBus.Error:org.freedesktop.portal.Error.NotAllowed`.
3. `flatpak-builder --disable-rofiles-fuse --repo=repo --force-clean build-dir flatpak/org.cloudy.af-local.yml`
4. `flatpak build-update-repo repo`
5. `flatpak build-bundle repo builds/cloudy-af-<version>.flatpak org.cloudy.af master`
6. Reinstall: `flatpak uninstall --user org.cloudy.af` then
   `flatpak install --user builds/cloudy-af-<version>.flatpak`

Gotcha: if step 2 is skipped or built without the feature flag, step 3
silently repackages the stale binary — the bundle shows a new version with
the old app. Symptom: version looks updated but behavior/UI is outdated.

## AppImage

The stock `tauri build --bundles appimage` output is not usable as-is: it
bundles the system GL/EGL stack (crashes WebKitWebProcess at exit on NVIDIA
hosts), drops a broken `put` symlink into the sidecar node_modules, lacks a
`node` binary for the sidecar, and extract-and-run doesn't put `usr/bin` on
PATH. Use `./build.sh` (AppImage target) or the manual procedure:
manual procedure:

1. Regenerate the AppDir: `APPIMAGE_EXTRACT_AND_RUN=1 npm run tauri:build -- --bundles appimage`
   (tauri's own linuxdeploy step fails — ignore it). AppDir:
   `src-tauri/target/release/bundle/appimage/Cloudy AF.AppDir/`. It is
   single-use — never re-run linuxdeploy on a processed AppDir; regenerate.
2. `cp -L /usr/bin/node "<AppDir>/usr/bin/node"` — the Node 22 x86_64 binary,
   matching the rebuilt node-hid addon ABI. Must be `-L` (follow symlinks).
3. Fix the `put` symlink tauri drops:
   `rm -f "<AppDir>/usr/lib/Cloudy AF/sidecar/node_modules/put" && cp -r sidecar/put-replacement "<AppDir>/usr/lib/Cloudy AF/sidecar/node_modules/put"`.
4. Run linuxdeploy with the GL libs EXCLUDED (forces the AppImage to use the
   host's GL drivers — the NVIDIA WebProcess fix):
   ```
   SQ=~/.cache/tauri/squashfs-root
   PATH=$SQ/usr/bin:$PATH LD_LIBRARY_PATH=$SQ/usr/lib:$LD_LIBRARY_PATH \
     APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 \
     $SQ/usr/bin/linuxdeploy --output appimage --plugin gtk \
     --exclude-library "libGL*" --exclude-library "libEGL*" \
     --exclude-library "libGLES*" --exclude-library "libgbm*" \
     --exclude-library "libdrm*" \
     --appdir "<AppDir>" -e target/release/cloudy-af \
     -d "<AppDir>/Cloudy AF.desktop" -i "<AppDir>/Cloudy AF.png"
   ```
   `ARCH=x86_64` is required because node-hid prebuilds contain foreign-arch
   ELFs. One-time: replace the bundled strip with the system one (it crashes
   on `.relr.dyn`): `cp /usr/bin/strip $SQ/usr/bin/strip`.
5. PATH hook so the runtime finds the bundled node — write
   `<AppDir>/apprun-hooks/zz-cloudy-af.sh` (mode +x):
   ```sh
   APPDIR="${APPDIR:-"$(dirname "$(realpath "$0")")"}"
   export PATH="$APPDIR/usr/bin:$PATH"
   ```
6. Repack:
   `APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 $SQ/plugins/linuxdeploy-plugin-appimage/usr/bin/appimagetool "<AppDir>"`
   → `Cloudy_AF-x86_64.AppImage`; copy to `builds/Cloudy AF_<version>_amd64.AppImage`.

On hosts without FUSE, verify with `APPIMAGE_EXTRACT_AND_RUN=1 ./builds/...AppImage`.
The main process shows in `ps` as plain `cloudy-af`; the sidecar appears as
`node /tmp/appimage_extracted_*/usr/lib/Cloudy AF/sidecar/hid-bridge.js`.
Kill BOTH after a test run — an orphaned sidecar holds the hidraw device open.

## Release artifacts

Every version build leaves all artifacts in `builds/`:

- `builds/cloudy-af-<version>.flatpak` — the bundle
- `builds/cloudy-af-<version>` — the raw release binary (`cp src-tauri/target/release/cloudy-af ...`)
- `builds/Cloudy AF_<version>_amd64.AppImage` — when an AppImage is built
- rpm/deb via `npm run tauri:build -- --bundles rpm,deb`

## Version bumps

When README/CHANGELOG/docs change for a release, bump the app version in the
same commit. All of these move together:

- `package.json`
- `sidecar/package.json` and `sidecar/package-lock.json` (2 spots)
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock`
- `flatpak/org.cloudy.af.appdata.xml` (add a `<release>` entry)
- `CHANGELOG.md` (new `## <version> — <date>` section)

Cadence: patch for fixes, minor for user-facing features.

## Device notes

- Nuvoton-line devices (e.g. iStick Pico) and STM32-line devices (e.g.
  iStick Rim C, VID 0483 / PID 5750) are both supported; the STM32 line uses
  a different HID command signature and flash base.
- Only one process may hold the HID device: quit other copies of the app
  (including AppImage/flatpak instances and orphaned sidecars) before
  connecting.
