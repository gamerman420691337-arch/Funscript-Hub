#!/usr/bin/env bash
# Check the finite model and require an authorization-negative mutant to fail.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
: "${TLA2TOOLS_JAR:?Set TLA2TOOLS_JAR to the local TLC 1.7.4 release jar}"
expected=936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88
actual="$(sha256sum "$TLA2TOOLS_JAR" | cut -d ' ' -f 1)"
if [[ "$actual" != "$expected" ]]; then
  printf '%s\n' 'TLC artifact differs from the recorded Phase A tool. Review the tool change before updating its pin.' >&2
  exit 2
fi
work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT
cp "$here/PulsarLifecycle.tla" "$here/PulsarLifecycle.cfg" "$work/"
(
  cd "$work"
  java -cp "$TLA2TOOLS_JAR" tlc2.TLC -workers 1 -config PulsarLifecycle.cfg PulsarLifecycle
)
node - "$work/PulsarLifecycle.tla" <<'NODE'
const fs = require('node:fs');
const file = process.argv[2];
const source = fs.readFileSync(file, 'utf8');
const guarded = 'Edit ==\n  /\\ authorized /\\ revision < MaxRevision';
if (source.split(guarded).length !== 2) throw new Error('Mutation target changed; update adequacy probe explicitly');
fs.writeFileSync(file, source.replace(guarded, 'Edit ==\n  /\\ revision < MaxRevision'));
NODE
set +e
(
  cd "$work"
  java -cp "$TLA2TOOLS_JAR" tlc2.TLC -workers 1 -config PulsarLifecycle.cfg PulsarLifecycle
) >"$work/mutant.log" 2>&1
rc=$?
set -e
cat "$work/mutant.log"
if [[ "$rc" -eq 0 ]] || ! grep -q 'Invariant NoUnauthorizedCommit is violated' "$work/mutant.log"; then
  printf '%s\n' 'Authorization mutant survived or failed for an unrelated reason.' >&2
  exit 1
fi
printf '%s\n' 'PASS: authorization-negative mutant killed by NoUnauthorizedCommit.'
