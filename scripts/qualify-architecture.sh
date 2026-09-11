#!/usr/bin/env bash
# Local architecture qualification, not product or hardware qualification.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
: "${CARGO_BUILD_JOBS:=2}"
export CARGO_BUILD_JOBS
node scripts/check-architecture.mjs
cargo check --workspace --all-targets --locked --offline
cargo test --workspace --locked --offline
if [[ -z "${TLA2TOOLS_JAR:-}" || ! -f "$TLA2TOOLS_JAR" ]]; then
  printf '%s\n' 'TLA2TOOLS_JAR must name the approved local TLC jar; formal gate not run.' >&2
  exit 2
fi
bash assurance/check-lifecycle.sh
printf '%s\n' 'Architecture checks passed. GUI evidence and capability ledger still require review.'
