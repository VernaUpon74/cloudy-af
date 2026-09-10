#!/usr/bin/env bash
# DEVIATION: Builds a FUSE3-compatible AppImage for Cloudy AF.
# The stock Tauri AppImage bundler uses a runtime that requires libfuse.so.2
# (FUSE2). Many modern distributions (including Fedora 40+) ship only FUSE3,
# so this script repackages the AppDir with the static AppImage runtime from
# https://github.com/AppImage/type2-runtime which works on FUSE3 systems.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
APPDIR="${PROJECT_ROOT}/src-tauri/target/release/bundle/appimage/Cloudy AF.AppDir"
BUNDLE_DIR="${PROJECT_ROOT}/src-tauri/target/release/bundle/appimage"
CACHE_DIR="${HOME}/.cache/cloudy-af-appimage"
NODE_VERSION="${NODE_VERSION:-22.13.1}"
NODE_ARCH="${NODE_ARCH:-linux-x64}"
STATIC_RUNTIME_URL="https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64"
LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
APPIMAGETOOL_URL="https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage"

mkdir -p "${CACHE_DIR}"

echo "==> Bundling Linux Node sidecar"
npm run sidecar:bundle:linux

echo "==> Building release binary inside the GNOME SDK (includes GTK/WebKit SDKs)"
# DEVIATION: The host often lacks GTK/WebKit development pkg-config files, so
# run Tauri's release build inside the same org.gnome.Sdk runtime the Flatpak
# uses. The resulting binary and bundled libraries are built against the GNOME
# runtime and are self-contained in the AppImage.
GNOME_SDK="org.gnome.Sdk//49"
SDK_PATH="/usr/lib/sdk/node22/bin:/usr/lib/sdk/rust-stable/bin:/app/bin:/usr/bin"
if flatpak run --command=bash \
        --filesystem="${PROJECT_ROOT}" \
        --filesystem=/var/home/j/.cargo \
        --share=network \
        --env=PATH="${SDK_PATH}" \
        --env=HOME=/var/home/j \
        --env=CARGO_HOME=/var/home/j/.cargo \
        "${GNOME_SDK}" \
        -c "cd '${PROJECT_ROOT}' && npm run build && npx tauri build" ; then
    echo "Tauri build completed in GNOME SDK"
else
    echo "Warning: Tauri build failed; will try to reuse an existing AppDir" >&2
fi

if [[ ! -d "${APPDIR}" ]]; then
    echo "Error: AppDir not found at ${APPDIR}" >&2
    echo "Did 'tauri build' succeed far enough to create the AppDir?" >&2
    exit 1
fi

VERSION="$(node -p "require('${PROJECT_ROOT}/package.json').version")"

echo "==> Preparing AppImage packaging tools"
APPIMAGETOOL="${CACHE_DIR}/appimagetool-x86_64.AppImage"
if [[ ! -f "${APPIMAGETOOL}" ]]; then
    curl -fsSL -o "${APPIMAGETOOL}" "${APPIMAGETOOL_URL}"
    chmod +x "${APPIMAGETOOL}"
fi
STATIC_RUNTIME="${CACHE_DIR}/runtime-x86_64"
if [[ ! -f "${STATIC_RUNTIME}" ]]; then
    curl -fsSL -o "${STATIC_RUNTIME}" "${STATIC_RUNTIME_URL}"
fi
APPIMAGETOOL_DIR="${CACHE_DIR}/appimagetool-extracted"
if [[ ! -d "${APPIMAGETOOL_DIR}/AppRun" ]]; then
    rm -rf "${APPIMAGETOOL_DIR}"
    mkdir -p "${APPIMAGETOOL_DIR}"
    cd "${APPIMAGETOOL_DIR}"
    "${APPIMAGETOOL}" --appimage-extract >/dev/null 2>&1
    if [[ -d "squashfs-root" ]]; then
        mv squashfs-root/* . 2>/dev/null || true
        rmdir squashfs-root 2>/dev/null || true
    fi
fi

echo "==> Bundling Node.js sidecar into AppDir"
# Tauri already created the AppDir with GTK/WebKit libraries and the desktop
# entry; we just need to add the Node runtime and HID sidecar so the Rust
# backend can spawn it at runtime. Prefer the freshly built Flatpak artifacts
# when they exist: they are built against the same GNOME runtime libraries that
# are bundled in the AppDir and are already stripped smaller than the official
# Node tarball.
strip "${APPDIR}/usr/bin/cloudy-af" 2>/dev/null || true
mkdir -p "${APPDIR}/usr/lib/cloudy-af/resources"
FLATPAK_BIN="${PROJECT_ROOT}/flatpak/build-dir/files/bin/cloudy-af"
FLATPAK_SIDECAR="${PROJECT_ROOT}/flatpak/build-dir/files/lib/cloudy-af/resources/sidecar"
FLATPAK_NODE="${PROJECT_ROOT}/flatpak/build-dir/files/bin/node"
LOCAL_SIDECAR="${PROJECT_ROOT}/src-tauri/binaries/linux/sidecar"
LOCAL_NODE="${PROJECT_ROOT}/src-tauri/binaries/linux/node"
if [[ -f "${FLATPAK_BIN}" ]]; then
    echo "Using Flatpak-built cloudy-af binary"
    cp "${FLATPAK_BIN}" "${APPDIR}/usr/bin/cloudy-af"
    chmod +x "${APPDIR}/usr/bin/cloudy-af"
fi
if [[ -d "${FLATPAK_SIDECAR}" ]]; then
    echo "Using Flatpak-built sidecar"
    rm -rf "${APPDIR}/usr/lib/cloudy-af/resources/sidecar"
    cp -a "${FLATPAK_SIDECAR}" "${APPDIR}/usr/lib/cloudy-af/resources/"
else
    rm -rf "${APPDIR}/usr/lib/cloudy-af/resources/sidecar"
    cp -a "${LOCAL_SIDECAR}" "${APPDIR}/usr/lib/cloudy-af/resources/"
fi
if [[ -f "${FLATPAK_NODE}" ]]; then
    echo "Using Flatpak Node runtime (smaller)"
    cp -a "${FLATPAK_NODE}" "${APPDIR}/usr/bin/node"
else
    cp -a "${LOCAL_NODE}" "${APPDIR}/usr/bin/node"
fi
# The official Node tarball's bin/node links against lib/libnode.so; bring it
# into the AppDir so the bundled binary does not pick up a system libnode.
LOCAL_NODE_LIB="${PROJECT_ROOT}/src-tauri/binaries/linux/lib"
if [[ -d "${LOCAL_NODE_LIB}" ]]; then
    cp -a "${LOCAL_NODE_LIB}"/libnode.so* "${APPDIR}/usr/lib/" 2>/dev/null || true
fi

# Tauri's AppImage bundler builds against the GNOME SDK but does not copy the
# WebKit helper processes or injected bundle into the AppDir. Bundle them here
# so the AppImage is self-contained on systems that do not have webkit2gtk-4.1
# installed natively.
echo "==> Bundling WebKit helper processes from GNOME SDK"
GNOME_SDK_PATH="$(flatpak info --show-location org.gnome.Sdk//49 2>/dev/null || true)"
if [[ -d "${GNOME_SDK_PATH}/files/libexec/webkit2gtk-4.1" ]]; then
    mkdir -p "${APPDIR}/usr/libexec/webkit2gtk-4.1"
    cp -a "${GNOME_SDK_PATH}/files/libexec/webkit2gtk-4.1"/* "${APPDIR}/usr/libexec/webkit2gtk-4.1/"
fi
if [[ -f "${GNOME_SDK_PATH}/files/lib/x86_64-linux-gnu/webkit2gtk-4.1/injected-bundle/libwebkit2gtkinjectedbundle.so" ]]; then
    mkdir -p "${APPDIR}/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/injected-bundle"
    cp -a "${GNOME_SDK_PATH}/files/lib/x86_64-linux-gnu/webkit2gtk-4.1/injected-bundle/libwebkit2gtkinjectedbundle.so" \
        "${APPDIR}/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/injected-bundle/"
fi

# Trim sidecar dev files that are unnecessary at runtime.
find "${APPDIR}/usr/lib/cloudy-af/resources/sidecar/node_modules" -type f \( \
    -name '*.d.ts' -o -name '*.map' -o -name 'README*' -o -name 'CHANGELOG*' -o \
    -name 'HISTORY*' -o -name 'LICENSE*' -o -name '*.md' \
\) -delete 2>/dev/null || true
find "${APPDIR}/usr/lib/cloudy-af/resources/sidecar/node_modules" -type d \( \
    -name 'test' -o -name 'tests' -o -name '__tests__' -o -name 'docs' -o \
    -name 'doc' -o -name 'examples' -o -name 'example' -o -name 'benchmark' -o \
    -name 'benchmarks' -o -name '.github' \
\) -prune -exec rm -rf {} + 2>/dev/null || true

echo "==> Creating FUSE3-compatible AppImage"
cd "${BUNDLE_DIR}"
rm -f *.AppImage
# Package the existing AppDir with the static type2 runtime. This produces an
# AppImage that works on FUSE3-only systems and uses zstd compression.
ARCH=x86_64 "${APPIMAGETOOL_DIR}/AppRun" --runtime-file="${STATIC_RUNTIME}" --comp xz \
    "Cloudy AF.AppDir" "Cloudy_AF-${VERSION}-x86_64.AppImage"

# Keep copies in builds/ so completed releases are easy to find.
mkdir -p "${PROJECT_ROOT}/builds"
rm -f "${PROJECT_ROOT}/builds/Cloudy_AF-"*.AppImage
if [[ -f "Cloudy_AF-${VERSION}-x86_64.AppImage" ]]; then
    cp "Cloudy_AF-${VERSION}-x86_64.AppImage" "${PROJECT_ROOT}/builds/"
    # Tauri also produces .deb and .rpm bundles; copy those too.
    DEB_SRC="${PROJECT_ROOT}/src-tauri/target/release/bundle/deb/Cloudy AF_${VERSION}_amd64.deb"
    RPM_SRC="${PROJECT_ROOT}/src-tauri/target/release/bundle/rpm/Cloudy AF-${VERSION}-1.x86_64.rpm"
    [[ -f "${DEB_SRC}" ]] && cp "${DEB_SRC}" "${PROJECT_ROOT}/builds/"
    [[ -f "${RPM_SRC}" ]] && cp "${RPM_SRC}" "${PROJECT_ROOT}/builds/"
    echo "==> AppImage ready: ${BUNDLE_DIR}/Cloudy_AF-${VERSION}-x86_64.AppImage"
    echo "==> Builds copied to: ${PROJECT_ROOT}/builds/"
else
    echo "Error: AppImage was not produced" >&2
    exit 1
fi
