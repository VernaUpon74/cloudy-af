#!/bin/bash

# Enhanced build script for Cloudy-af with Docker support and user prompts
# Supports both Flatpak and AppImage builds with experimental patches option

set -e  # Exit on any error

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

# Prompt for build target
echo ""
echo "Select build target:"
echo "1) Flatpak (recommended for Linux desktop environments)"
echo "2) AppImage (recommended for portable distribution)"
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

# Prompt for patches
echo ""
echo "Experimental patches support:"
echo "WARNING: Experimental patches may cause instability or break functionality!"
sleep 1
echo "This option is intended for advanced users only."
echo "Recovery Tool in FW Editor when device bootloops."
sleep 2
read -p "Enable experimental patches? (y/N): " patches_choice

if [[ $patches_choice =~ ^[Yy]$ ]]; then
    ENABLE_PATCHES=true
    echo "Experimental patches enabled ✓"
else
    ENABLE_PATCHES=false
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

# Extract version
VERSION=$(cd "$MAIN_REPO_DIR" &> /dev/null && grep -o '"version": "[^"]*"' package.json | cut -d'"' -f4)
if [ -z "$VERSION" ]; then
    VERSION="1.2.0"
fi

echo "Version: $VERSION"
echo "Repository: $MAIN_REPO_DIR"

# Set default to local build unless explicitly requested
DOCKER_BUILD=1

if [ "$DOCKER_BUILD" = "1" ]; then
    echo "Using Docker build approach for consistent dependencies..."
    
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

if [ "$BUILD_TARGET" = "appimage" ]; then
    # Real AppImage via linuxdeploy, following the documented procedure in
    # AGENTS.md: host GL/EGL stack EXCLUDED (NVIDIA webprocess crash fix),
    # node runtime bundled for the sidecar, `put` symlink fixed, PATH hook.
    
    # Check if binary exists
    if [ ! -f "$MAIN_REPO_DIR/src-tauri/target/release/cloudy-af" ]; then
        echo "ERROR: Tauri binary not found!"
        exit 1
    fi

    SQ="$HOME/.cache/tauri/squashfs-root"
    if [ ! -x "$SQ/usr/bin/linuxdeploy" ]; then
        echo "ERROR: extracted linuxdeploy not found at $SQ"
        echo "It is created by a 'npm run tauri:build -- --bundles appimage' run (which may fail; the AppDir tooling is still extracted)."
        exit 1
    fi

    # One-time prerequisite: replace the bundled strip with the system one
    # (the bundled one crashes on .relr.dyn sections).
    if [ ! -f "$SQ/usr/bin/strip.bundled-orig" ]; then
        cp "$SQ/usr/bin/strip" "$SQ/usr/bin/strip.bundled-orig"
        cp /usr/bin/strip "$SQ/usr/bin/strip"
        echo "Replaced bundled strip with system strip (backup at strip.bundled-orig)"
    fi

    # Assemble a fresh AppDir (single-use — never re-run linuxdeploy on a
    # processed AppDir).
    WORK_DIR=$(mktemp -d)
    APPDIR="$WORK_DIR/Cloudy AF.AppDir"
    mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib/Cloudy AF" "$APPDIR/apprun-hooks"

    cp "$MAIN_REPO_DIR/src-tauri/target/release/cloudy-af" "$APPDIR/usr/bin/cloudy-af"

    # Node runtime for the sidecar — must be -L (the real x86_64 binary,
    # not a symlink, or appimagetool arch detection breaks).
    cp -L /usr/bin/node "$APPDIR/usr/bin/node"

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
    cp -r "$MAIN_REPO_DIR/sidecar" "$APPDIR/usr/lib/Cloudy AF/sidecar"

    # Fix the broken `put` symlink.
    rm -f "$APPDIR/usr/lib/Cloudy AF/sidecar/node_modules/put"
    cp -r "$MAIN_REPO_DIR/sidecar/put-replacement" "$APPDIR/usr/lib/Cloudy AF/sidecar/node_modules/put"

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

    # PATH hook so the runtime finds the bundled node.
    cat > "$APPDIR/apprun-hooks/zz-cloudy-af.sh" << 'EOFHOOK'
APPDIR="${APPDIR:-"$(dirname "$(realpath "$0")")"}"
export PATH="$APPDIR/usr/bin:$PATH"
EOFHOOK
    chmod +x "$APPDIR/apprun-hooks/zz-cloudy-af.sh"

    # Run linuxdeploy with the GL/EGL stack EXCLUDED — the AppImage must use
    # the host's GL drivers (NVIDIA WebKitWebProcess crash fix).
    echo "Running linuxdeploy..."
    cd "$WORK_DIR"
    # The host has no webkit2gtk (immutable base), so linuxdeploy on the host
    # fails with "Could not find dependency: libwebkit2gtk-4.1.so.0". Run it
    # inside the arcticfox-build toolbox, which has the GTK/WebKit stack; the
    # toolbox shares $HOME, so $SQ and $WORK_DIR paths resolve identically.
    LINUXDEPLOY_CMD="PATH=\"$SQ/usr/bin:\$PATH\" LD_LIBRARY_PATH=\"$SQ/usr/lib:\$LD_LIBRARY_PATH\" \
    APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 \
    \"$SQ/usr/bin/linuxdeploy\" --output appimage --plugin gtk \
        --exclude-library \"libGL*\" --exclude-library \"libEGL*\" \
        --exclude-library \"libGLES*\" --exclude-library \"libgbm*\" \
        --exclude-library \"libdrm*\" \
        --appdir \"$APPDIR\" -e \"$APPDIR/usr/bin/cloudy-af\" \
        -d \"$APPDIR/Cloudy AF.desktop\" -i \"$APPDIR/Cloudy AF.png\""
    if command -v toolbox &> /dev/null && \
       toolbox run -c arcticfox-build ldconfig -p 2>/dev/null | grep -q libwebkit2gtk-4.1; then
        toolbox run -c arcticfox-build sh -c "cd \"$WORK_DIR\" && $LINUXDEPLOY_CMD"
    else
        eval "$LINUXDEPLOY_CMD"
    fi

    # Repack into the final AppImage.
    echo "Packing AppImage..."
    APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 \
    "$SQ/plugins/linuxdeploy-plugin-appimage/usr/bin/appimagetool" "$APPDIR"

    APPIMAGE_NAME="Cloudy AF_${VERSION}_amd64.AppImage"
    # appimagetool derives the output name from the desktop file; accept any
    # produced AppImage rather than assuming one exact name.
    PRODUCED=$(ls "$WORK_DIR"/*.AppImage 2>/dev/null | head -1)
    if [ -z "$PRODUCED" ]; then
        echo "ERROR: appimagetool produced no AppImage in $WORK_DIR"
        exit 1
    fi
    cp "$PRODUCED" "$BUILD_DIR/$APPIMAGE_NAME"
    rm -rf "$WORK_DIR"

    echo "✓ AppImage created: $BUILD_DIR/$APPIMAGE_NAME"
    echo "  Verify on this host (no FUSE): APPIMAGE_EXTRACT_AND_RUN=1 \"$BUILD_DIR/$APPIMAGE_NAME\""
    
elif [ "$BUILD_TARGET" = "flatpak" ]; then
    # Create Flatpak build
    
    # Check if flatpak-builder is available
    if ! command -v flatpak-builder &> /dev/null; then
        echo "Warning: flatpak-builder not found. Installing..."
        sudo dnf install -y flatpak flatpak-builder
    fi
    
    # Build with flatpak
    echo "Building Flatpak bundle..."
    echo "Note: Flatpak build requires org.cloudy.af.yaml manifest file"
    echo "This step is skipped in this enhanced script but can be implemented as needed"
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
        echo "✓ Verification: AppImage exists and is $(ls -lh "$BUILD_DIR/Cloudy AF_${VERSION}_amd64.AppImage" | awk '{print $5}')"
    fi
else
    echo "Flatpak build completed (requires manifest file)"
fi

echo ""
echo "Build script completed successfully!"
