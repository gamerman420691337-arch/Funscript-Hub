#!/usr/bin/env node
// Finite design model and safeguard-removal checks, not implementation proof.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const jar = process.env.TLA2TOOLS_JAR;
if (!jar || !fs.existsSync(jar)) throw new Error("TLA2TOOLS_JAR must name an existing TLC jar");
const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pulsar-import-model-"));
const base = fs.readFileSync(path.join(root, "assurance/PulsarPackageImport.cfg"), "utf8");
const model = "PulsarPackageImport.tla";
fs.copyFileSync(path.join(root, "assurance", model), path.join(dir, model));
const cases = [
  ["none", null], ["full_bytes", "VerifiedPlan"], ["content", "VerifiedPlan"],
  ["closure", "VerifiedPlan"], ["cas_hit", "VerifiedPlan"],
  ["recovery", "RecoveryBeforeObjectPublication"], ["object_sync", "DurableVisibility"], ["authorization", "NoCancelledPublication"],
  ["atomic_rows", "AtomicVisibility"], ["identity", "FreshIdentity"],
  ["origin_authority", "NoImportedAuthority"], ["reservation", "RetainedReservation"],
  ["shared_object", "PreservePreexisting"], ["replay", "AtMostOneProject"],
  ["terminal_replay", "NoTerminalResurrection"], ["request_binding", "RequestBinding"],
  ["ack", "SafeAcknowledgement"],
];
function normalExit(result, code) {
  return !result.error && !result.signal && result.status === code;
}
// Regression: an expected diagnostic printed before a signal is not a valid result.
if (normalExit({status:null, signal:"SIGTERM"}, 12) ||
    normalExit({status:12, signal:"SIGTERM"}, 12) ||
    normalExit({status:null}, 12) ||
    !normalExit({status:12, signal:null}, 12)) {
  throw new Error("TLC exit classification regression");
}
const results = [];
try {
  for (const [fault, expected] of cases) {
    const cfg = path.join(dir, fault + ".cfg");
    fs.writeFileSync(cfg, base.replace('Fault = "none"', 'Fault = "' + fault + '"'));
    const result = spawnSync("java", ["-Xmx768m", "-XX:+UseParallelGC", "-cp", jar, "tlc2.TLC", "-workers", "1",
      "-metadir", path.join(dir, fault), "-config", path.basename(cfg), model],
      { cwd: dir, encoding: "utf8", timeout: 120000, maxBuffer: 8 * 1024 * 1024 });
    const output = (result.stdout ?? "") + (result.stderr ?? "");
    const invariant = output.match(/Invariant (\w+) is violated/);
    const safe = normalExit(result, 0) && /Model checking completed\. No error has been found/.test(output);
    const killed = normalExit(result, 12) && invariant?.[1] === expected;
    const counts = output.match(/([\d,]+) states generated, ([\d,]+) distinct states found/);
    const entry = { fault, expected_invariant: expected, passed: expected ? killed : safe,
      exit_code: result.status, signal: result.signal ?? null, error: result.error?.message ?? null,
      violated_invariant: invariant?.[1] ?? null,
      generated_states: counts?.[1] ?? null, distinct_states: counts?.[2] ?? null };
    results.push(entry);
    process.stdout.write(JSON.stringify(entry) + "\n");
    if (!entry.passed) process.stdout.write(output + "\n");
  }
} finally {
  fs.rmSync(dir, { recursive: true, force: true });
}
process.exitCode = results.every(r => r.passed) ? 0 : 1;
