#!/usr/bin/env bash
# DEVIATION: Darwin twin of prepare-linux-sidecar.sh. Fetches the official
# Node.js runtime for the .app sidecar (injected into the bundle by
# package-macos-dmg.sh) and forces a from-source node-hid rebuild so the
# patched addon (sidecar/patches/node-hid+2.2.0.patch) compiles for darwin.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUNDLE_DIR="${PROJECT_ROOT}/src-tauri/binaries/darwin"
NODE_VERSION="${NODE_VERSION:-22.13.1}"
NODE_ARCH="${NODE_ARCH:-darwin-arm64}"
NODE_TARBALL="node-v${NODE_VERSION}-${NODE_ARCH}.tar.gz"
NODE_URL="https://nodejs.org/dist/v${NODE_VERSION}/${NODE_TARBALL}"

mkdir -p "${BUNDLE_DIR}"
cd "${BUNDLE_DIR}"

if [[ ! -f "node" ]]; then
    echo "Downloading Node.js ${NODE_VERSION} for ${NODE_ARCH}..."
    curl -fsSL "${NODE_URL}" -o "${NODE_TARBALL}"
    # The official darwin node is a single self-contained Mach-O binary.
    tar -xzf "${NODE_TARBALL}" --strip-components=2 "node-v${NODE_VERSION}-${NODE_ARCH}/bin/node"
    rm -f "${NODE_TARBALL}"
    chmod +x node
fi

# Install sidecar deps and rebuild the native addon from source against the
# darwin Node ABI. patch-package applies the hidraw close() mutex patch; the
# Linux-only hunks warn on darwin and are skipped — harmless (see AGENTS.md).
SIDECAR_DIR="${PROJECT_ROOT}/sidecar"
cd "${SIDECAR_DIR}"
npm ci
npm_config_build_from_source=true npm rebuild node-hid

echo "macOS sidecar bundle ready at ${BUNDLE_DIR}"
