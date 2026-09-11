#!/usr/bin/env node
// Finite transfer model gate. Does not prove Rust, filesystem or timing behavior.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const jar = process.env.TLA2TOOLS_JAR;
if (!jar || !fs.existsSync(jar)) {
  process.stderr.write("NOT RUN: TLA2TOOLS_JAR must identify a local TLC jar.\n");
  process.exit(2);
}
const source = fs.readFileSync(path.join(root, "assurance/PulsarTransfers.tla"), "utf8");
const config = fs.readFileSync(path.join(root, "assurance/PulsarTransfers.cfg"), "utf8");
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "pulsar-transfer-model-"));
const checks = [];
const mutants = [
  ["write-authorization", "MUTATION_WRITE_AUTH", "NoUnauthorizedEffects"],
  ["lease-epoch", "MUTATION_EPOCH", "NoStaleEpochEffects"],
  ["digest-publication", "MUTATION_DIGEST", "NoBadDigestPublication"],
  ["revision-commit", "MUTATION_REVISION", "NoStaleCommit"],
];
function run(name, model, expectedInvariant) {
  const dir = path.join(scratch, name);
  fs.mkdirSync(dir);
  fs.writeFileSync(path.join(dir, "PulsarTransfers.tla"), model);
  fs.writeFileSync(path.join(dir, "PulsarTransfers.cfg"), config);
  const result = spawnSync("java", [
    "-Xmx512m", "-XX:+UseParallelGC", "-cp", path.resolve(jar),
    "tlc2.TLC", "-workers", "1", "-config", "PulsarTransfers.cfg",
    "PulsarTransfers.tla"
  ], { cwd: dir, encoding: "utf8", timeout: 120_000, maxBuffer: 4 * 1024 * 1024 });
  const output = (result.stdout ?? "") + (result.stderr ?? "");
  process.stdout.write("=== " + name + " ===\n" + output);
  const passed = expectedInvariant
    ? result.status !== 0 && !result.error &&
      output.includes("Invariant " + expectedInvariant + " is violated.")
    : result.status === 0 && !result.error &&
      output.includes("Model checking completed. No error has been found.");
  const states = output.match(/(\d+) states generated, (\d+) distinct states found/);
  const depth = output.match(/depth of the complete state graph search is (\d+)/);
  checks.push({ name, passed, exit_code: result.status,
    expected_invariant_violation: expectedInvariant ?? null,
    generated_states: states ? Number(states[1]) : null,
    distinct_states: states ? Number(states[2]) : null,
    depth: depth ? Number(depth[1]) : null,
    error: result.error?.message ?? null });
}
try {
  run("baseline", source, null);
  for (const [name, marker, invariant] of mutants) {
    const lines = source.split("\n");
    const index = lines.findIndex(line => line.includes(marker));
    if (index < 0 || lines.filter(line => line.includes(marker)).length !== 1) {
      throw new Error("Mutation marker must occur exactly once: " + marker);
    }
    lines[index] = "  /\\ TRUE \\* deliberately removed " + marker;
    run(name, lines.join("\n"), invariant);
  }
  const passed = checks.every(check => check.passed);
  process.stdout.write(JSON.stringify({ gate: "motion-transfer-lifecycle", passed,
    model_sha256: crypto.createHash("sha256").update(source).digest("hex"),
    config_sha256: crypto.createHash("sha256").update(config).digest("hex"),
    tlc_jar_sha256: crypto.createHash("sha256").update(fs.readFileSync(jar)).digest("hex"),
    checks, limitations: [
      "One transfer/project, bounded byte count/revisions/epochs; no fairness or real-time proof.",
      "Digest verification, atomic durable publication and reservations are abstract predicates.",
      "No Rust, operating-system, fsync, memory-accounting, download or cryptographic proof.",
      "Lost acknowledgments and rejected requests are stuttering steps; concrete replay tests remain required."
    ] }, null, 2) + "\n");
  process.exitCode = passed ? 0 : 1;
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
