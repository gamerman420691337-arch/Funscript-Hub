#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/qualify-architecture.sh
node scripts/check-motion-transfers-model.mjs
printf '%s\n' 'Motion transport structural/test/model checks passed; GUI, resource and phase qualification evidence remain separate.'
