#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${HOME}/.local/bin"

echo "=================================================="
echo "Installing Pulsar — Funscript Generator (pulsar)"
echo "=================================================="

# Check Cargo
if ! command -v cargo &> /dev/null; then
    echo "Error: cargo could not be found. Please install Rust: https://rustup.rs"
    exit 1
fi

CARGO_FEATURES=""
if command -v nvidia-smi &> /dev/null; then
    echo "-> NVIDIA GPU detected! Bundling hardware CUDA acceleration..."
    CARGO_FEATURES="--features cuda"
fi

echo "-> Building optimized release binary..."
cd "${SCRIPT_DIR}"
cargo build --release --bin pulsar ${CARGO_FEATURES}

mkdir -p "${BIN_DIR}"

TARGET_BIN="${SCRIPT_DIR}/target/release/pulsar"
if [ ! -f "${TARGET_BIN}" ]; then
    TARGET_BIN="${SCRIPT_DIR}/open-fungen/target/release/pulsar"
fi

echo "-> Installing binary symlinks into ${BIN_DIR}..."
ln -sf "${TARGET_BIN}" "${BIN_DIR}/pulsar"
ln -sf "${TARGET_BIN}" "${BIN_DIR}/fs-hub"
ln -sf "${TARGET_BIN}" "${BIN_DIR}/open-fungen"

# Install Desktop Entry and Icons (Linux Freedesktop)
if [ -d "${SCRIPT_DIR}/assets" ]; then
    APPS_DIR="${HOME}/.local/share/applications"
    ICONS_DIR="${HOME}/.local/share/icons/hicolor"
    MIME_DIR="${HOME}/.local/share/mime/packages"

    mkdir -p "${APPS_DIR}" "${ICONS_DIR}" "${MIME_DIR}"

    echo "-> Installing desktop launcher and icons..."
    if [ -f "${SCRIPT_DIR}/assets/pulsar.desktop" ]; then
        cp "${SCRIPT_DIR}/assets/pulsar.desktop" "${APPS_DIR}/pulsar.desktop"
    fi

    if [ -d "${SCRIPT_DIR}/assets/icons/hicolor" ]; then
        cp -r "${SCRIPT_DIR}/assets/icons/hicolor/"* "${ICONS_DIR}/" 2>/dev/null || true
    fi

    if [ -f "${SCRIPT_DIR}/assets/mime/pulsar-mime.xml" ]; then
        cp "${SCRIPT_DIR}/assets/mime/pulsar-mime.xml" "${MIME_DIR}/pulsar-mime.xml"
        if command -v update-mime-database &> /dev/null; then
            update-mime-database "${HOME}/.local/share/mime" 2>/dev/null || true
        fi
    fi

    if command -v update-desktop-database &> /dev/null; then
        update-desktop-database "${APPS_DIR}" 2>/dev/null || true
    fi
fi

echo "=================================================="
echo "Pulsar installation complete!"
echo "Target binary: ${TARGET_BIN}"
echo "You can now run:"
echo "  pulsar         # Open unified desktop GUI"
echo "  pulsar --help  # Show CLI commands"
echo "  fs-hub         # Backward-compatible alias"
echo "Desktop launcher and .funscript file associations installed!"
echo "=================================================="
