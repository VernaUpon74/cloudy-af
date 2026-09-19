#!/bin/bash

# Build script for Cloudy-af: Docker-based Tauri build + Flatpak or AppImage.
#
# Usage: ./build.sh [appimage|flatpak] [--patches] [-y|--yes]
#   With no arguments the original interactive prompts are used.
#   Env: CLOUDY_TOOLBOX — toolbox container used for the AppImage deploy/pack
#        stage (default: arcticfox-build when it exists, else host mode).

set -e  # Exit on any error

BUILD_TARGET=""
ENABLE_PATCHES=""
ASSUME_YES=false
for arg in "$@"; do
    case "$arg" in
        appimage|flatpak) BUILD_TARGET="$arg" ;;
        --appimage)       BUILD_TARGET="appimage" ;;
        --flatpak)        BUILD_TARGET="flatpak" ;;
        --patches)        ENABLE_PATCHES=true ;;
        --no-patches)     ENABLE_PATCHES=false ;;
        -y|--yes|--non-interactive) ASSUME_YES=true ;;
        -h|--help)
            sed -n '3,9p' "$0" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *) echo "Unknown argument: $arg (try --help)" >&2; exit 2 ;;
    esac
done

# Get the directory where this script is located
echo "================================================"
echo "BUILDING CLOUDY-AF APPLICATION"
echo "================================================"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
MAIN_REPO_DIR="$SCRIPT_DIR"

# Check if Docker is available and accessible
if command -v docker &> /dev/null; then
    echo "Docker is available ✓"
    # Test if we can access Docker daemon
    if ! docker info > /dev/null 2>&1; then
        # Fallback 1: rootless Podman docker-compatible socket — no docker
        # group membership or root daemon required.
        PODMAN_SOCK="/run/user/$(id -u)/podman/podman.sock"
        if command -v podman &> /dev/null; then
            systemctl --user enable --now podman.socket &> /dev/null
            if [ -S "$PODMAN_SOCK" ] && \
               DOCKER_HOST="unix://$PODMAN_SOCK" docker info > /dev/null 2>&1; then
                export DOCKER_HOST="unix://$PODMAN_SOCK"
                echo "Using rootless Podman socket ($DOCKER_HOST) ✓"
            fi
        fi
    fi

    if ! docker info > /dev/null 2>&1; then
        # Fallback 2: the user may have been added to the docker group AFTER
        # this login session started. Group membership only applies to new
        # sessions, but `sg docker` spawns a shell with the group active now.
        if ! id -nG | tr ' ' '\n' | grep -qx docker && \
           getent group docker | tr ',' '\n' | grep -qx "$USER"; then
            echo "You are in the docker group, but this shell session was started before"
            echo "the group was added. Re-running this script with the docker group active..."
            echo ""
            exec sg docker -c "\"$0\" $*"
        fi

        echo "Error: Cannot access Docker daemon or a Podman socket."
        echo "  - Rootless option (no root needed): install podman and re-run this script"
        echo "  - Docker option: sudo systemctl start docker && sudo usermod -aG docker $USER"
        exit 1
    fi
else
    echo "Error: Docker not found. Please install Docker to use this build script."
    exit 1
fi

# Prompt for build target (skipped when given on the command line)
if [ -n "$BUILD_TARGET" ]; then
    echo "Build target: $BUILD_TARGET"
else
    echo ""
    echo "Select build target:"
    echo "1) Flatpak (recommended for Linux desktop environments)"
    echo "2) AppImage (recommended for portable distribution)"
    if [ "$ASSUME_YES" = true ]; then
        BUILD_TARGET="flatpak"
        echo "Non-interactive mode: defaulting to Flatpak"
    else
        read -p "Enter choice (1 or 2): " build_choice

        case $build_choice in
            1)
                BUILD_TARGET="flatpak"
                echo "Selected: Flatpak build"
                ;;
            2)
                BUILD_TARGET="appimage"
                echo "Selected: AppImage build"
                ;;
            *)
                echo "Invalid choice. Defaulting to Flatpak."
                BUILD_TARGET="flatpak"
                ;;
        esac
    fi
fi

# Prompt for patches (skipped when --patches/--no-patches was given)
if [ -n "$ENABLE_PATCHES" ]; then
    echo "Experimental patches: $ENABLE_PATCHES"
else
    echo ""
    echo "Experimental patches support:"
    echo "WARNING: Experimental patches may cause instability or break functionality!"
    sleep 1
    echo "This option is intended for advanced users only."
    echo "Recovery Tool in FW Editor when device bootloops."
    sleep 2
    if [ "$ASSUME_YES" = true ]; then
        ENABLE_PATCHES=false
        echo "Non-interactive mode: patches disabled"
    else
        read -p "Enable experimental patches? (y/N): " patches_choice
        if [[ $patches_choice =~ ^[Yy]$ ]]; then
            ENABLE_PATCHES=true
        else
            ENABLE_PATCHES=false
        fi
    fi
fi

if [ "$ENABLE_PATCHES" = true ]; then
    echo "Experimental patches enabled ✓"
else
    echo "Experimental patches disabled"
fi

echo ""
echo "================================================"
echo "BUILD CONFIGURATION SUMMARY"
echo "================================================"
echo "Build Target: $BUILD_TARGET"
echo "Experimental Patches: $ENABLE_PATCHES"
echo "Repository Directory: $MAIN_REPO_DIR"
echo "Script Directory: $SCRIPT_DIR"
echo "================================================"

# --- Disk space check ---------------------------------------------------
# Ensures enough free space before a build; offers to clear caches if not.
# MIN_BUILD_SPACE_MB: minimum free MB required for the build to succeed.
check_disk_space() {
    local min_mb=${1:-5120}  # default 5 GB
    local mount_point="${2:-.}"

    local avail_kb
    avail_kb=$(df --output=avail "$mount_point" 2>/dev/null | tail -1 | tr -d ' ')
    if [ -z "$avail_kb" ]; then
        echo "WARNING: could not determine free disk space — skipping check."
        return 0
    fi

    local avail_mb=$((avail_kb / 1024))
    if [ "$avail_mb" -ge "$min_mb" ]; then
        echo "Free disk space: ${avail_mb} MB (>= ${min_mb} MB required) ✓"
        return 0
    fi

    echo ""
    echo "⚠  Low disk space: ${avail_mb} MB free, ${min_mb} MB required for this build."
    echo ""
    echo "The following caches can be cleared to reclaim space:"
    echo "  - pip cache           (~$(du -sm ~/.cache/pip 2>/dev/null | cut -f1 || echo 0) MB)"
    echo "  - npm cache           (~$(du -sm ~/.npm/_cacache 2>/dev/null | cut -f1 || echo 0) MB)"
    echo "  - cargo registry      (~$(du -sm ~/.cargo/registry 2>/dev/null | cut -f1 || echo 0) MB)"
    echo "  - flatpak-builder     (~$(du -sm .flatpak-builder 2>/dev/null | cut -f1 || echo 0) MB)"
    echo "  - flatpak cache       (~$(du -sm ~/.cache/flatpak 2>/dev/null | cut -f1 || echo 0) MB)"
    echo "  - node-gyp cache      (~$(du -sm ~/.cache/node-gyp 2>/dev/null | cut -f1 || echo 0) MB)"
    echo "  - electron cache      (~$(du -sm ~/.cache/electron 2>/dev/null | cut -f1 || echo 0) MB)"
    echo "  - uv cache            (~$(du -sm ~/.cache/uv 2>/dev/null | cut -f1 || echo 0) MB)"
    echo "  - playwright cache    (~$(du -sm ~/.cache/ms-playwright 2>/dev/null | cut -f1 || echo 0) MB)"
    echo ""

    if [ "$ASSUME_YES" = true ]; then
        echo "Non-interactive mode: clearing all caches..."
    else
        read -p "Clear these caches now? (Y/n): " clear_choice
        if [[ "$clear_choice" =~ ^[Nn]$ ]]; then
            echo "Aborting build — not enough free space."
            exit 1
        fi
    fi

    echo "Clearing caches..."
    rm -rf ~/.cache/pip
    rm -rf ~/.npm/_cacache
    rm -rf ~/.cargo/registry
    rm -rf .flatpak-builder
    rm -rf ~/.cache/flatpak
    rm -rf ~/.cache/node-gyp
    rm -rf ~/.cache/electron
    rm -rf ~/.cache/electron-builder
    rm -rf ~/.cache/uv
    rm -rf ~/.cache/ms-playwright
    echo "Caches cleared."

    # Re-check
    avail_kb=$(df --output=avail "$mount_point" 2>/dev/null | tail -1 | tr -d ' ')
    avail_mb=$((avail_kb / 1024))
    if [ "$avail_mb" -lt "$min_mb" ]; then
        echo "ERROR: still only ${avail_mb} MB free after clearing caches (need ${min_mb} MB)."
        echo "Free up disk space manually and try again."
        exit 1
    fi
    echo "Free disk space after cleanup: ${avail_mb} MB ✓"
}

# Set patches in local-flags.json
if [ "$ENABLE_PATCHES" = true ]; then
    echo '{"patches": true}' > "$MAIN_REPO_DIR/local-flags.json"
    echo "Enabled experimental patches in local-flags.json"
else
    echo '{"patches": false}' > "$MAIN_REPO_DIR/local-flags.json"
    echo "Disabled experimental patches in local-flags.json"
fi

# Create builds directory if it doesn't exist
BUILD_DIR="$MAIN_REPO_DIR/builds"
mkdir -p "$BUILD_DIR"

# Extract version (portable: python is not guaranteed; grep/sed are POSIX).
VERSION=$(grep -o '"version": *"[^"]*"' "$MAIN_REPO_DIR/package.json" | head -1 | sed 's/.*"\([^"]*\)"$/\1/')
if [ -z "$VERSION" ]; then
    # Fall back to the Tauri config before any hardcoded default — a stale
    # default would silently mislabel release artifacts.
    VERSION=$(grep -o '"version": *"[^"]*"' "$MAIN_REPO_DIR/src-tauri/tauri.conf.json" | head -1 | sed 's/.*"\([^"]*\)"$/\1/')
fi
if [ -z "$VERSION" ]; then
    echo "ERROR: could not determine app version from package.json" >&2
    exit 1
fi

echo "Version: $VERSION"
echo "Repository: $MAIN_REPO_DIR"

# Set default to local build unless explicitly requested
DOCKER_BUILD=1

if [ "$DOCKER_BUILD" = "1" ]; then
    echo "Using Docker build approach for consistent dependencies..."
    
    # The Tauri binary embeds dist/ at compile time (ADR-007 custom-protocol
    # build), so a stale dist/ means the AppImage silently ships old frontend
    # code (and ignores local-flags.json via the __CLOUDY_PATCHES__ vite
    # define). Rebuild dist/ whenever any frontend source is newer.
    cd "$MAIN_REPO_DIR"
    if [ ! -f dist/index.html ] || \
       [ -n "$(find src index.html vite.config.js public -newer dist/index.html -print -quit 2>/dev/null)" ]; then
        echo "dist/ is missing or stale — rebuilding frontend..."
        npm run build
    else
        echo "dist/ is up to date ($(stat -c '%y' dist/index.html | cut -d. -f1))"
    fi

    # Create temporary directory for the Docker build
    TEMP_DIR=$(mktemp -d)
    echo "Using temporary directory: $TEMP_DIR"
    
    # Copy source files to temp directory, replicating the repo layout:
    # tauri.conf.json references sibling paths (../dist, ../public,
    # ../resources, ../sidecar), so src-tauri must sit next to them.
    if [ -d "$MAIN_REPO_DIR/src-tauri" ]; then
        mkdir -p "$TEMP_DIR/src-tauri"
        cp -r "$MAIN_REPO_DIR/src-tauri/"* "$TEMP_DIR/src-tauri/"
        # The build context must not include local build artifacts
        # (target/ can be several GB).
        rm -rf "$TEMP_DIR/src-tauri/target" "$TEMP_DIR/src-tauri/gen"
        echo "Copied src-tauri to temp directory"
    else
        echo "Error: src-tauri directory not found in $MAIN_REPO_DIR"
        exit 1
    fi

    for dir in dist public resources sidecar; do
        if [ -d "$MAIN_REPO_DIR/$dir" ]; then
            cp -r "$MAIN_REPO_DIR/$dir" "$TEMP_DIR/"
            echo "Copied $dir to temp directory"
        else
            echo "Warning: $dir directory not found in $MAIN_REPO_DIR (referenced by tauri.conf.json)"
        fi
    done
    # cargo build only validates that resource paths exist — node_modules
    # is dead weight in the build context (hundreds of MB).
    rm -rf "$TEMP_DIR/sidecar/node_modules"
    echo "Note: dist/ is used as-is — run 'npm run build' first if frontend sources changed."

    # Copy package.json and local-flags.json
    if [ -f "$MAIN_REPO_DIR/package.json" ]; then
        cp "$MAIN_REPO_DIR/package.json" "$TEMP_DIR/"
        echo "Copied package.json to temp directory"
    fi

    if [ -f "$MAIN_REPO_DIR/local-flags.json" ]; then
        cp "$MAIN_REPO_DIR/local-flags.json" "$TEMP_DIR/"
        echo "Copied local-flags.json to temp directory"
    fi
    
    # Create Dockerfile for the build
    cat > "$TEMP_DIR/Dockerfile" << 'EOFDOCKER'
FROM debian:bookworm-slim AS build

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libwebkit2gtk-4.1-dev \
    libjavascriptcoregtk-4.1-dev \
    libsoup-3.0-dev \
    libssl-dev \
    libudev-dev \
    curl \
    ca-certificates \
    file \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain stable
ENV PATH="/root/.cargo/bin:$PATH"

WORKDIR /app
COPY . .
WORKDIR /app/src-tauri
# tauri/custom-protocol is MANDATORY (ADR-007): without it the binary is a
# dev-mode build that loads the frontend from http://localhost:1420.
RUN cargo build --release --features tauri/custom-protocol

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    libwebkit2gtk-4.1-0 \
    libjavascriptcoregtk-4.1-0 \
    libsoup-3.0-0 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=build /app/src-tauri/target/release/cloudy-af .
CMD ["./cloudy-af"]
EOFDOCKER
    
    # Build with Docker
    cd "$TEMP_DIR"
    echo "Building Docker image..."
    # --load: with the buildx container driver the image otherwise stays in
    # the build cache and docker run can't find it locally.
    docker build --load -t cloudy-af-build .
    
    # Extract the binary from the image. Use create+cp instead of a volume
    # mount: rootless podman + SELinux denies in-container writes to /output.
    echo "Extracting built binary..."
    CONTAINER_ID=$(docker create cloudy-af-build)
    docker cp "$CONTAINER_ID:/app/cloudy-af" "$TEMP_DIR/cloudy-af"
    docker rm "$CONTAINER_ID" > /dev/null
    
    # Copy the binary to original location for AppImage creation
    cd "$MAIN_REPO_DIR"
    mkdir -p "src-tauri/target/release/"
    cp "$TEMP_DIR/cloudy-af" "src-tauri/target/release/cloudy-af"
    
    # Clean up temp directory
    rm -rf "$TEMP_DIR"
    
    echo "Docker build completed successfully"
else
    echo "Using local build approach (requires all dependencies installed)"
fi

# Build the Tauri binary locally only when the Docker stage didn't already
# produce it — the host lacks the GTK/soup/dbus/udev dev libraries.
if [ "$DOCKER_BUILD" != "1" ]; then
    echo "Building fresh Tauri binary..."
    cd "$MAIN_REPO_DIR"
    npm run tauri:build -- --no-bundle

    if [ $? -ne 0 ]; then
        echo "ERROR: Tauri build failed!"
        exit 1
    fi

    echo "✓ Tauri build completed successfully"
else
    echo "Skipping local Tauri build — using the Docker-built binary."
fi

# Create appropriate output based on user choice
echo "Creating $BUILD_TARGET..."

# Pre-build disk space check — AppImage needs ~5 GB, Flatpak needs ~5 GB.
if [ "$BUILD_TARGET" = "appimage" ]; then
    check_disk_space 5120 "$MAIN_REPO_DIR"
elif [ "$BUILD_TARGET" = "flatpak" ]; then
    check_disk_space 5120 "$MAIN_REPO_DIR"
fi

if [ "$BUILD_TARGET" = "appimage" ]; then
    # Real AppImage via linuxdeploy, following the documented procedure in
    # AGENTS.md: host GL/EGL stack EXCLUDED (NVIDIA webprocess crash fix),
    # node runtime bundled for the sidecar, `put` symlink fixed, PATH hook.
    
    # Check if binary exists
    if [ ! -f "$MAIN_REPO_DIR/src-tauri/target/release/cloudy-af" ]; then
        echo "ERROR: Tauri binary not found!"
        exit 1
    fi

    # Toolbox selection: CLOUDY_TOOLBOX env wins; default to the project's
    # arcticfox-build container; else auto-detect the only existing toolbox;
    # else fall back to running linuxdeploy on the host directly (works on
    # ordinary dev machines where libwebkit2gtk-4.1 is a system package).
    TOOLBOX_NAME="${CLOUDY_TOOLBOX:-}"
    if [ -z "$TOOLBOX_NAME" ] && command -v toolbox >/dev/null 2>&1; then
        if toolbox list -c 2>/dev/null | grep -q arcticfox-build; then
            TOOLBOX_NAME="arcticfox-build"
        else
            # Exactly one non-host toolbox? Use it. Zero or many: host mode.
            # NB: the NAME is column 2 of `toolbox list -c` (ID NAME CREATED
            # STATUS IMAGE) — $NF is the image name, not the container name.
            N=$(toolbox list -c 2>/dev/null | grep -c .)
            if [ "$N" -eq 1 ]; then
                TOOLBOX_NAME=$(toolbox list -c 2>/dev/null | awk 'NR==1{print $2}')
            fi
        fi
    fi
    if [ -n "$TOOLBOX_NAME" ] && ! toolbox list -c 2>/dev/null | grep -q "$TOOLBOX_NAME"; then
        echo "ERROR: toolbox '$TOOLBOX_NAME' not found (CLOUDY_TOOLBOX=$CLOUDY_TOOLBOX)"
        exit 1
    fi

    # Prerequisite tools for linuxdeploy. Inside a toolbox they must exist
    # THERE (the host having them is not enough); try a dnf install
    # automatically, fail with clear instructions otherwise.
    # (gdk-pixbuf-query-loaders is deliberately NOT required: the gtk plugin
    # only warns when it is missing, and some distros ship it -64-suffixed.)
    check_tools() {
        local missing=""
        for t in desktop-file-validate update-desktop-database file; do
            command -v "$t" >/dev/null 2>&1 || missing="$missing $t"
        done
        [ -n "$missing" ] && { echo "$missing"; return 1; } || return 0
    }
    if [ -n "$TOOLBOX_NAME" ]; then
        if ! toolbox run -c "$TOOLBOX_NAME" sh -c "$(declare -f check_tools); check_tools" >/dev/null 2>&1; then
            echo "Installing linuxdeploy prerequisites into toolbox '$TOOLBOX_NAME'..."
            toolbox run -c "$TOOLBOX_NAME" sudo dnf install -y --quiet \
                desktop-file-utils gdk-pixbuf2 appstream file >/dev/null 2>&1 || true
            if ! toolbox run -c "$TOOLBOX_NAME" sh -c "$(declare -f check_tools); check_tools" >/dev/null 2>&1; then
                echo "ERROR: missing in toolbox '$TOOLBOX_NAME':$(toolbox run -c "$TOOLBOX_NAME" sh -c "$(declare -f check_tools); check_tools" 2>/dev/null)"
                echo "Install manually: toolbox run -c $TOOLBOX_NAME sudo dnf install desktop-file-utils gdk-pixbuf2 appstream file"
                exit 1
            fi
        fi
        echo "Toolbox '$TOOLBOX_NAME' prerequisites OK ✓"
    elif ! check_tools >/dev/null 2>&1; then
        echo "ERROR: missing on host:$(check_tools) — install desktop-file-utils gdk-pixbuf2 appstream file"
        exit 1
    fi

    # linuxdeploy tooling: auto-extract the cached AppImage if the extracted
    # tree is missing (previously this was a dead end requiring a manual
    # tauri bundler run first).
    SQ="$HOME/.cache/tauri/squashfs-root"
    if [ ! -x "$SQ/usr/bin/linuxdeploy" ]; then
        LD_APPIMG="$HOME/.cache/tauri/linuxdeploy-x86_64.AppImage"
        if [ -f "$LD_APPIMG" ]; then
            echo "Extracting linuxdeploy tooling from cached AppImage..."
            rm -rf "$SQ"
            (cd "$HOME/.cache/tauri" && APPIMAGE_EXTRACT_AND_RUN=1 "$LD_APPIMG" --appimage-extract >/dev/null)
        fi
        if [ ! -x "$SQ/usr/bin/linuxdeploy" ]; then
            echo "ERROR: linuxdeploy tooling not found at $SQ"
            echo "Provide $LD_APPIMG (download from https://github.com/linuxdeploy/linuxdeploy/releases),"
            echo "or run 'npm run tauri:build -- --bundles appimage' once (it extracts the tooling even on failure)."
            exit 1
        fi
        echo "linuxdeploy tooling ready at $SQ ✓"
    fi

    # Assemble a fresh AppDir (single-use — never re-run linuxdeploy on a
    # processed AppDir). NB: the work dir must be under $HOME — the toolbox
    # (arcticfox-build) that runs linuxdeploy shares the home dir but not /tmp.
    WORK_DIR=$(mktemp -d "$HOME/cloudy-af-appimage.XXXXXX")
    # Remove the work dir on any exit — including failures BEFORE the collect
    # stage below (a failed deploy otherwise leaves a ~200 MB AppDir behind).
    trap 'rm -rf "$WORK_DIR"' EXIT
    APPDIR="$WORK_DIR/Cloudy AF.AppDir"
    mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib/Cloudy AF" "$APPDIR/apprun-hooks"

    cp "$MAIN_REPO_DIR/src-tauri/target/release/cloudy-af" "$APPDIR/usr/bin/cloudy-af"

    # Resources, laid out as the Tauri bundler would (see tauri.conf.json).
    # NB: `[ -d x ] && cp` as a bare statement would trip `set -e` when the
    # dir is missing, hence the if form.
    if [ -f "$MAIN_REPO_DIR/public/default.afc.json" ]; then
        cp "$MAIN_REPO_DIR/public/default.afc.json" "$APPDIR/usr/lib/Cloudy AF/"
    fi
    if [ -d "$MAIN_REPO_DIR/public/i18n" ]; then
        cp -r "$MAIN_REPO_DIR/public/i18n" "$APPDIR/usr/lib/Cloudy AF/"
    fi
    for res in definitions patches firmware animations; do
        if [ -d "$MAIN_REPO_DIR/resources/$res" ]; then
            cp -r "$MAIN_REPO_DIR/resources/$res" "$APPDIR/usr/lib/Cloudy AF/"
        else
            echo "Warning: resources/$res not found, skipping"
        fi
    done
    # Bundle the node runtime + HID sidecar via the canonical script
    # (scripts/prepare-linux-sidecar.sh). It copies the host's patched
    # node_modules verbatim and REFUSES to bundle unless the node-hid mutex
    # patch is present in HID.cc AND the addon binary is newer than it —
    # the verification gate added 2026-09-12 after the AppImage flicker
    # analysis (an unpatched addon SIGABRTs → device indicator flicker).
    bash "$MAIN_REPO_DIR/scripts/prepare-linux-sidecar.sh"
    # Static official Node build (no libnode.so dependency); must be the real
    # x86_64 binary, not a symlink, or appimagetool arch detection breaks.
    cp "$MAIN_REPO_DIR/src-tauri/binaries/linux/node" "$APPDIR/usr/bin/node"
    cp -a "$MAIN_REPO_DIR/src-tauri/binaries/linux/sidecar" "$APPDIR/usr/lib/Cloudy AF/sidecar"

    # Smoke-test the bundled runtime now (before packaging) — a truncated or
    # corrupt 102 MB node binary otherwise only fails at first launch.
    if ! "$APPDIR/usr/bin/node" -e "process.stdout.write(process.version + '\n')"; then
        echo "ERROR: bundled node runtime failed to execute" >&2
        exit 1
    fi

    # Safety net for the `put` module: the canonical script preserves symlinks
    # verbatim. The host's link is the fixed relative one, but if npm install
    # ever restores an absolute/broken link, re-point it at the bundled
    # put-replacement so the AppImage sidecar can always load it.
    PUT_LINK="$APPDIR/usr/lib/Cloudy AF/sidecar/node_modules/put"
    if [ "$(readlink "$PUT_LINK" 2>/dev/null)" != "../put-replacement" ]; then
        rm -rf "$PUT_LINK"
        ln -s ../put-replacement "$PUT_LINK"
        echo "Re-pointed sidecar put symlink at bundled put-replacement"
    fi

    # Desktop file and icon.
    cat > "$APPDIR/Cloudy AF.desktop" << 'EOFDESKTOP'
[Desktop Entry]
Type=Application
Name=Cloudy AF
Comment=Arctic Fox configuration tool
Exec=cloudy-af
Icon=cloudy-af
Categories=Utility;
Terminal=false
EOFDESKTOP
    cp "$MAIN_REPO_DIR/src-tauri/icons/icon.png" "$APPDIR/Cloudy AF.png"

    # linuxdeploy deploys the icon by the desktop entry's Icon= basename
    # ("cloudy-af") into usr/share/icons/hicolor/<size>/apps — "Cloudy AF.png"
    # (with a space) does not match and aborts the whole build. Provide the
    # lowercase name at the PNG's actual size (read from the IHDR header —
    # no ImageMagick dependency), as well as a 64x64 fallback slot.
    ICON_W=$(od -An -tu1 -j16 -N8 "$MAIN_REPO_DIR/src-tauri/icons/icon.png" \
        | awk '{print $1*16777216 + $2*65536 + $3*256 + $4}')
    ICON_H=$(od -An -tu1 -j16 -N8 "$MAIN_REPO_DIR/src-tauri/icons/icon.png" \
        | awk '{print $5*16777216 + $6*65536 + $7*256 + $8}')
    ICON_DIR="$APPDIR/usr/share/icons/hicolor/${ICON_W}x${ICON_H}/apps"
    mkdir -p "$ICON_DIR"
    cp "$MAIN_REPO_DIR/src-tauri/icons/icon.png" "$ICON_DIR/cloudy-af.png"
    mkdir -p "$APPDIR/usr/share/icons/hicolor/64x64/apps"
    cp "$MAIN_REPO_DIR/src-tauri/icons/icon.png" \
        "$APPDIR/usr/share/icons/hicolor/64x64/apps/cloudy-af.png"

    # WebKit helper processes. Tauri's WebKitWebView spawns
    # WebKitNetworkProcess/WebKitWebProcess/WebKitGPUProcess at runtime from
    # usr/libexec/webkit2gtk-4.1 — linuxdeploy's gtk plugin does NOT bundle
    # them, so without this step the AppImage dies at startup with
    # "Failed to spawn child process .../WebKitNetworkProcess
    # (No such file or directory)". Copy them (and jsc/MiniBrowser, which the
    # known-good build ships) from the build environment — on Fedora the
    # helpers are in /usr/libexec/webkit2gtk-4.1 (the injected-bundle .so
    # stays in /usr/lib64/webkit2gtk-4.1 and is found via the library rpath).
    if [ -n "$TOOLBOX_NAME" ]; then
        echo "Bundling WebKit helper processes from toolbox..."
        toolbox run -c "$TOOLBOX_NAME" sh -c "
            set -e
            mkdir -p '$APPDIR/usr/libexec/webkit2gtk-4.1'
            cp -a /usr/libexec/webkit2gtk-4.1/. '$APPDIR/usr/libexec/webkit2gtk-4.1/' 2>/dev/null || \
            cp -a /usr/lib64/webkit2gtk-4.1/. '$APPDIR/usr/libexec/webkit2gtk-4.1/'
        "
    else
        for d in /usr/libexec/webkit2gtk-4.1 /usr/lib64/webkit2gtk-4.1 \
                 /usr/lib/x86_64-linux-gnu/webkit2gtk-4.1; do
            if [ -f "$d/WebKitNetworkProcess" ]; then
                mkdir -p "$APPDIR/usr/libexec/webkit2gtk-4.1"
                cp -a "$d/." "$APPDIR/usr/libexec/webkit2gtk-4.1/"
                break
            fi
        done
    fi
    WEBKIT_HELPERS="$APPDIR/usr/libexec/webkit2gtk-4.1/WebKitNetworkProcess"
    if [ ! -f "$WEBKIT_HELPERS" ]; then
        echo "WARNING: WebKit helper processes not found — the AppImage will fail at startup."
        echo "  Install the WebKit 4.1 runtime in the build environment (toolbox: sudo dnf install webkit2gtk4.1)."
    else
        echo "WebKit helpers bundled ✓ ($(ls "$APPDIR/usr/libexec/webkit2gtk-4.1" | tr '\n' ' '))"
        # The helpers' own library lookups must resolve against the AppDir
        # libs, and WEBKIT_EXEC_PATH points WebKit at the bundled directory
        # (the library otherwise builds a relative ./../libexec path that
        # does not exist once extracted).
        cat > "$APPDIR/apprun-hooks/zz-webkit-helpers.sh" << 'EOFWBK'
export WEBKIT_EXEC_PATH="${APPDIR}/usr/libexec/webkit2gtk-4.1"
EOFWBK
        chmod +x "$APPDIR/apprun-hooks/zz-webkit-helpers.sh"
    fi

    # PATH hook so the runtime finds the bundled node.
    cat > "$APPDIR/apprun-hooks/zz-cloudy-af.sh" << 'EOFHOOK'
APPDIR="${APPDIR:-"$(dirname "$(realpath "$0")")"}"
export PATH="$APPDIR/usr/bin:$PATH"
EOFHOOK
    chmod +x "$APPDIR/apprun-hooks/zz-cloudy-af.sh"

    # Run linuxdeploy with the GL/EGL stack EXCLUDED — the AppImage must use
    # the host's GL drivers (NVIDIA WebKitWebProcess crash fix).
    #
    # Run INSIDE the build toolbox (CLOUDY_TOOLBOX, default arcticfox-build):
    # linuxdeploy must find the GLib/WebKit 4.1 shared objects it deploys
    # into the AppDir, and on a flatpak-centric host those exist only in the
    # toolbox — on such hosts ldconfig has no libwebkit2gtk-4.1 at all and
    # linuxdeploy aborts with "Could not find dependency:
    # libwebkit2gtk-4.1.so.0" (one of the bugs that broke AppImage builds).
    # Requires desktop-file-utils/gdk-pixbuf2/appstream inside the toolbox.
    echo "Deploying dependencies + packing AppImage (in toolbox: $TOOLBOX_NAME)..."
    toolbox run -c "$TOOLBOX_NAME" sh -c "
        set -e
        PATH='/usr/bin:$SQ/usr/bin' LD_LIBRARY_PATH='$SQ/usr/lib' \
        APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 \
        '$SQ/usr/bin/linuxdeploy' --output appimage --plugin gtk \
            --exclude-library 'libGL*' --exclude-library 'libEGL*' \
            --exclude-library 'libGLES*' --exclude-library 'libgbm*' \
            --exclude-library 'libdrm*' \
            --appdir '$APPDIR' -e '$APPDIR/usr/bin/cloudy-af' \
            --desktop-file='$APPDIR/Cloudy AF.desktop' \
            --icon-file='$APPDIR/Cloudy AF.png' \
            -d '$APPDIR/Cloudy AF.desktop' -i '$APPDIR/Cloudy AF.png'
    "

    # Remove the work dir on any exit (a failed deploy otherwise leaves a
    # ~200 MB AppDir under $HOME).
    trap 'rm -rf "$WORK_DIR"' EXIT

    APPIMAGE_NAME="Cloudy AF_${VERSION}_amd64.AppImage"
    # appimagetool (invoked by the linuxdeploy appimage plugin) writes the
    # output relative to its own working directory — which is NOT necessarily
    # $WORK_DIR (observed: the repo root). Search everywhere it can land, and
    # never rely on a single assumed location.
    PRODUCED=$(find "$WORK_DIR" "$MAIN_REPO_DIR" -maxdepth 1 -name '*.AppImage' \
        -newermt "-10 minutes" 2>/dev/null | head -1)
    if [ -z "$PRODUCED" ]; then
        echo "ERROR: appimagetool produced no AppImage (looked in $WORK_DIR and $MAIN_REPO_DIR)"
        exit 1
    fi
    cp "$PRODUCED" "$BUILD_DIR/$APPIMAGE_NAME"
    # Don't leave a stray copy in the repo root when it landed there.
    case "$PRODUCED" in
        "$MAIN_REPO_DIR"/*) rm -f "$PRODUCED" ;;
    esac

    echo "✓ AppImage created: $BUILD_DIR/$APPIMAGE_NAME"
    echo "  Verify on this host (no FUSE): APPIMAGE_EXTRACT_AND_RUN=1 \"$BUILD_DIR/$APPIMAGE_NAME\""
    
elif [ "$BUILD_TARGET" = "flatpak" ]; then
    # Create Flatpak build
    echo "Building Flatpak bundle..."

    # Check if flatpak-builder is available
    if ! command -v flatpak-builder &> /dev/null; then
        echo "ERROR: flatpak-builder not found. Install with: sudo dnf install flatpak-builder"
        exit 1
    fi

    if [ ! -f "flatpak/org.cloudy.af-local.yml" ]; then
        echo "ERROR: flatpak manifest not found at flatpak/org.cloudy.af-local.yml"
        exit 1
    fi

    echo "Step 1/2: Cleaning flatpak builder cache..."
    rm -rf build-dir .flatpak-builder

    echo "Step 2/3: Building Flatpak repo..."
    flatpak-builder --force-clean --disable-rofiles-fuse --repo=repo build-dir \
        flatpak/org.cloudy.af-local.yml || { echo "FAIL: flatpak-builder"; exit 1; }

    echo "Step 3/3: Exporting Flatpak bundle..."
    flatpak build-bundle --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo repo \
        "$BUILD_DIR/cloudy-af-$VERSION.flatpak" org.cloudy.af \
        || { echo "FAIL: build-bundle"; exit 1; }

    echo "Cleaning up flatpak build cache..."
    rm -rf build-dir .flatpak-builder repo

    echo "✓ Flatpak bundle created: $BUILD_DIR/cloudy-af-$VERSION.flatpak"
fi

echo ""
echo "================================================"
echo "BUILD COMPLETE!"
echo "================================================"
if [ "$BUILD_TARGET" = "appimage" ]; then
    echo "Files created:"
    echo "  - $BUILD_DIR/Cloudy AF_${VERSION}_amd64.AppImage"
    echo "================================================"
    
    # Verify file exists
    if [ -f "$BUILD_DIR/Cloudy AF_${VERSION}_amd64.AppImage" ]; then
        echo "✓ Verification: AppImage exists and is $(du -h "$BUILD_DIR/Cloudy AF_${VERSION}_amd64.AppImage" | cut -f1)"
    fi
elif [ "$BUILD_TARGET" = "flatpak" ]; then
    echo "Files created:"
    echo "  - $BUILD_DIR/cloudy-af-$VERSION.flatpak"
    echo "================================================"

    # Verify file exists
    if [ -f "$BUILD_DIR/cloudy-af-$VERSION.flatpak" ]; then
        echo "✓ Verification: Flatpak bundle exists and is $(du -h "$BUILD_DIR/cloudy-af-$VERSION.flatpak" | cut -f1)"
    fi
else
    echo "No specific build target selected"
fi

echo ""
echo "Build script completed successfully!"

# --- Final cleanup ---
# Remove Docker build artifacts and cargo target (no longer needed once the
# package has been produced).  Keeps ~2.5 G from lingering after every build.
if [ "$DOCKER_BUILD" = "1" ]; then
    docker rmi cloudy-af-build 2>/dev/null || true
    rm -rf "$MAIN_REPO_DIR/src-tauri/target"
    echo "Cleaned Docker image + cargo target"
fi
# Flatpak build artifacts
rm -rf "$MAIN_REPO_DIR/build-dir" "$MAIN_REPO_DIR/.flatpak-builder" "$MAIN_REPO_DIR/repo"
