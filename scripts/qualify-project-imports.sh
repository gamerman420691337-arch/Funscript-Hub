#!/usr/bin/env bash
# P1c local qualification. No device actuation, private media, downloads, or GPU work.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -n 1024
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_INCREMENTAL=0
export CARGO_NET_OFFLINE=true
: "${TLA2TOOLS_JAR:?Set TLA2TOOLS_JAR to the installed development TLC jar}"

# Qualify the identifier guard before using the shared architecture scanner.
node --test scripts/architecture-source-guards.test.mjs

# Preserve the existing motion/export contract gates before exercising import.
bash scripts/qualify-project-packages.sh
node scripts/qualify-package-import-model.mjs

# Process fixtures must execute the real worker-capable binary, not a test harness.
cargo build --bin pulsar --offline --locked
target_dir="${CARGO_TARGET_DIR:-target}"
export PULSAR_PACKAGE_TEST_WORKER="$(cd "$target_dir/debug" && pwd)/pulsar"
cargo test -p pulsar-engine --lib \
  authority::project_package_closure::project_closure_captures_actual_generated_input_and_inert_job_origin \
  --offline --locked -- --ignored --exact --nocapture
cargo test --test project_package_import --offline --locked -- --nocapture --test-threads=1

# Large generated data is distinct from normal unit/process evidence.
if [[ "${PULSAR_RUN_LARGE_PACKAGE_IMPORT_TEST:-0}" == "1" ]]; then
  cargo test --test project_package_import --offline --locked -- --ignored --nocapture --test-threads=1
fi
