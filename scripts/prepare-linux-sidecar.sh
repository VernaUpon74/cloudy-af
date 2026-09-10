#!/usr/bin/env bash
# DEVIATION: Bundles the Node.js runtime and HID sidecar so the Linux AppImage
# does not depend on a system `node` binary. This mirrors the macOS sidecar
# bundling approach and keeps the AppImage self-contained.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUNDLE_DIR="${PROJECT_ROOT}/src-tauri/binaries/linux"
NODE_VERSION="${NODE_VERSION:-22.13.1}"
NODE_ARCH="${NODE_ARCH:-linux-x64}"
NODE_TARBALL="node-v${NODE_VERSION}-${NODE_ARCH}.tar.xz"
NODE_URL="https://nodejs.org/dist/v${NODE_VERSION}/${NODE_TARBALL}"

mkdir -p "${BUNDLE_DIR}"
cd "${BUNDLE_DIR}"

if [[ ! -f "node" ]]; then
    echo "Downloading Node.js ${NODE_VERSION} for ${NODE_ARCH}..."
    curl -fsSL "${NODE_URL}" -o "${NODE_TARBALL}"
    tar -xf "${NODE_TARBALL}" --strip-components=2 "node-v${NODE_VERSION}-${NODE_ARCH}/bin/node"
    # The official Node binary links against libnode.so, so bundle it as well.
    mkdir -p lib
    tar -xf "${NODE_TARBALL}" --strip-components=2 -C lib "node-v${NODE_VERSION}-${NODE_ARCH}/lib/libnode.so" 2>/dev/null || true
    rm -f "${NODE_TARBALL}"
    chmod +x node
    # Strip the Node binary to reduce AppImage size. Node's runtime does not
    # need debug symbols for the HID sidecar to function.
    if command -v strip >/dev/null 2>&1; then
        strip node || true
        strip lib/libnode.so* 2>/dev/null || true
    fi
fi

# Rebuild native modules against the bundled Node binary so the AppImage works
# on systems that may not have a compatible system node-hid build.
SIDECAR_SRC="${PROJECT_ROOT}/sidecar"
SIDECAR_DST="${BUNDLE_DIR}/sidecar"
if [[ -d "${SIDECAR_DST}" ]]; then
    rm -rf "${SIDECAR_DST}"
fi
mkdir -p "${SIDECAR_DST}"
cp -a "${SIDECAR_SRC}/"*.js "${SIDECAR_DST}/"
cp -a "${SIDECAR_SRC}/package.json" "${SIDECAR_DST}/"
cp -a "${SIDECAR_SRC}/package-lock.json" "${SIDECAR_DST}/" 2>/dev/null || true
cp -a "${SIDECAR_SRC}/patches" "${SIDECAR_DST}/" 2>/dev/null || true
cp -a "${SIDECAR_SRC}/put-replacement" "${SIDECAR_DST}/" 2>/dev/null || true

# Copy the already-installed sidecar dependencies. They were built against the
# local Node 22 runtime, which is ABI-compatible with the bundled Node binary.
SIDECAR_NODE_MODULES="${SIDECAR_SRC}/node_modules"
if [[ -d "${SIDECAR_NODE_MODULES}" ]]; then
    cp -a "${SIDECAR_NODE_MODULES}" "${SIDECAR_DST}/"
    # Remove files that are unnecessary at runtime to reduce AppImage size.
    find "${SIDECAR_DST}/node_modules" -type f \( \
        -name '*.d.ts' -o \
        -name '*.map' -o \
        -name 'README*' -o \
        -name 'CHANGELOG*' -o \
        -name 'HISTORY*' -o \
        -name 'LICENSE*' -o \
        -name '*.md' \
    \) -delete 2>/dev/null || true
    find "${SIDECAR_DST}/node_modules" -type d \( \
        -name 'test' -o \
        -name 'tests' -o \
        -name '__tests__' -o \
        -name 'docs' -o \
        -name 'doc' -o \
        -name 'examples' -o \
        -name 'benchmark' -o \
        -name 'benchmarks' -o \
        -name '.github' \
    \) -prune -exec rm -rf {} + 2>/dev/null || true
else
    echo "Warning: sidecar/node_modules not found; run npm install first." >&2
fi

echo "Linux sidecar bundle ready at ${BUNDLE_DIR}"
