#!/usr/bin/env bash
# P1b local qualification. No device actuation, private media, network downloads, or GPU work.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -n 1024
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_NET_OFFLINE=true
: "${TLA2TOOLS_JAR:?Set TLA2TOOLS_JAR to the installed development TLC jar}"
bash scripts/qualify-motion-transfers.sh
node scripts/qualify-package-model.mjs
cargo build --bin pulsar --offline --locked
cargo test --test project_package_export --offline --locked -- --nocapture
if [[ "${PULSAR_RUN_LARGE_PACKAGE_TEST:-0}" == "1" ]]; then
  cargo test --test project_package_export --offline --locked -- --ignored --nocapture
fi
