#!/bin/bash
# Updates the launcher's bundled peeky binary with a fresh build
set -e

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY_DIR="$REPO_ROOT/launcher/src-tauri/binaries"
ARCH="$(uname -m)"

# Map arch to Rust target triple
case "$ARCH" in
    arm64|aarch64) TARGET="aarch64-apple-darwin" ;;
    x86_64)        TARGET="x86_64-apple-darwin" ;;
    *)             echo "Unknown arch: $ARCH"; exit 1 ;;
esac

BINARY_NAME="peeky-$TARGET"

echo "Building peeky (release) for $TARGET..."
cd "$REPO_ROOT"
# On macOS the default `hyprland` feature is a Linux-only no-op, so the
# default build takes the winit path (same as the release.yml macOS job).
cargo build --release -p peeky --bin peeky

echo "Copying binary to launcher..."
cp "$REPO_ROOT/target/release/peeky" "$BINARY_DIR/$BINARY_NAME"
chmod +x "$BINARY_DIR/$BINARY_NAME"

echo "Done! Updated: $BINARY_DIR/$BINARY_NAME"
