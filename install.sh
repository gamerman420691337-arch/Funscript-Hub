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

echo "-> Building optimized release binary..."
cd "${SCRIPT_DIR}"
cargo build --release --bin pulsar

mkdir -p "${BIN_DIR}"

TARGET_BIN="${SCRIPT_DIR}/target/release/pulsar"
if [ ! -f "${TARGET_BIN}" ]; then
    TARGET_BIN="${SCRIPT_DIR}/open-fungen/target/release/pulsar"
fi

echo "-> Installing binary symlinks into ${BIN_DIR}..."
ln -sf "${TARGET_BIN}" "${BIN_DIR}/pulsar"
ln -sf "${TARGET_BIN}" "${BIN_DIR}/fs-hub"
ln -sf "${TARGET_BIN}" "${BIN_DIR}/open-fungen"

echo "=================================================="
echo "Pulsar installation complete!"
echo "Target binary: ${TARGET_BIN}"
echo "You can now run:"
echo "  pulsar         # Open unified desktop GUI"
echo "  pulsar --help  # Show CLI commands"
echo "  fs-hub         # Backward-compatible alias"
echo "=================================================="
