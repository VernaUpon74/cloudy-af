#!/usr/bin/env bash
# Watch public/i18n/ for translation changes; when they go quiet, rebuild the
# Rust binary (in the arcticfox-build toolbox) and optionally the Flatpak
# (with --disable-rofiles-fuse) plus bundle/byproducts into builds/.
# Usage: scripts/translations-watch-rebuild.sh [--flatpak]
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
WITH_FLATPAK=0
[ "${1:-}" = "--flatpak" ] && WITH_FLATPAK=1
VERSION=$(python3 -c "import json;print(json.load(open('package.json'))['version'])")
LOG=/tmp/translations-rebuild.log

sig() { cat public/i18n/*.json | sha256sum | cut -d' ' -f1; }

rebuild() {
  echo "=== $(date) translation change detected — rebuilding $VERSION ==="
  python3 scripts/check-i18n.py || { echo "FAIL: i18n check"; return 1; }
  toolbox run -c arcticfox-build sh -c \
    "cd '$ROOT/src-tauri' && PKG_CONFIG_PATH=/usr/lib64/pkgconfig:/usr/share/pkgconfig cargo build --release --features tauri/custom-protocol" \
    || { echo "FAIL: cargo build"; return 1; }
  cp -f src-tauri/target/release/cloudy-af "builds/cloudy-af-$VERSION"
  echo "OK: binary -> builds/cloudy-af-$VERSION"
  if [ "$WITH_FLATPAK" = 1 ]; then
    rm -rf build-dir .flatpak-builder
    flatpak-builder --force-clean --disable-rofiles-fuse --repo=repo build-dir \
      flatpak/org.cloudy.af-local.yml || { echo "FAIL: flatpak-builder"; return 1; }
    flatpak build-bundle --runtime-repo=repo repo "builds/cloudy-af-$VERSION.flatpak" org.cloudy.af \
      || { echo "FAIL: build-bundle"; return 1; }
    echo "OK: bundle -> builds/cloudy-af-$VERSION.flatpak"
  fi
  echo "=== $(date) rebuild complete ==="
}

echo "watching public/i18n (version $VERSION, flatpak=$WITH_FLATPAK) — Ctrl-C to stop"
LAST=$(sig); QUIET=0
while :; do
  sleep 15
  NOW=$(sig)
  if [ "$NOW" != "$LAST" ]; then QUIET=0; LAST=$NOW; continue; fi
  if [ "$NOW" != "${SETTLED:-}" ]; then
    QUIET=$((QUIET + 15))
    if [ "$QUIET" -ge 60 ]; then
      SETTLED=$NOW
      rebuild >> "$LOG" 2>&1 && echo "$(date) rebuild done (see $LOG)" || echo "$(date) rebuild FAILED (see $LOG)"
    fi
  fi
done
