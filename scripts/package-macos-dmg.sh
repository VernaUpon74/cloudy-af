#!/usr/bin/env bash
# DEVIATION: Post-build step for the macOS bundle. tauri --bundles app produces
# an unsigned Cloudy AF.app with no Node runtime; the app resolves
# Contents/Resources/sidecar-runtime/node before falling back to PATH
# (find_node_binary in src-tauri/src/lib.rs). This script injects the Node
# binary fetched by prepare-macos-sidecar.sh, ad-hoc signs it (arm64 Mach-O
# binaries must carry at least an ad-hoc signature), re-signs the app, and
# wraps it in a .dmg with hdiutil (no create-dmg dependency).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
NODE_BIN="${PROJECT_ROOT}/src-tauri/binaries/darwin/node"
APP_DIR="${PROJECT_ROOT}/src-tauri/target/release/bundle/macos"
APP="${APP_DIR}/Cloudy AF.app"
VERSION="$(node -p "require('${PROJECT_ROOT}/package.json').version")"
DMG_DIR="${PROJECT_ROOT}/src-tauri/target/release/bundle/dmg"
DMG="${DMG_DIR}/cloudy-af_${VERSION}_aarch64.dmg"

[[ -d "${APP}" ]] || { echo "App bundle not found: ${APP}" >&2; exit 1; }
[[ -f "${NODE_BIN}" ]] || { echo "Node binary not found: ${NODE_BIN} (run prepare-macos-sidecar.sh)" >&2; exit 1; }

echo "Injecting Node runtime into ${APP}..."
mkdir -p "${APP}/Contents/Resources/sidecar-runtime"
cp "${NODE_BIN}" "${APP}/Contents/Resources/sidecar-runtime/node"
chmod +x "${APP}/Contents/Resources/sidecar-runtime/node"

# Ad-hoc sign: injected binary first, then the whole app so arm64 launches.
codesign --force --sign - "${APP}/Contents/Resources/sidecar-runtime/node"
codesign --force --deep --sign - "${APP}"

mkdir -p "${DMG_DIR}"
rm -f "${DMG}"
hdiutil create -volname "Cloudy AF" -srcfolder "${APP}" -ov -format UDZO \
    -fs HFS+ "${DMG}"

echo "DMG ready: ${DMG}"
