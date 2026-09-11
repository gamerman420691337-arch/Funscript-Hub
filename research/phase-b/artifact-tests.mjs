#!/usr/bin/env node
/**
 * Independent B1 matcher/durable-artifact checks. Temporary inputs are synthetic.
 * Execute under lamu agent; no product code, media, models or device effects.
 */
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { evaluateClip, benchmarkCorpus } from '../../scripts/benchmark-good.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
let checks = 0, rng = 0x31415926;
const random = () => { rng = (Math.imul(rng, 1103515245) + 12345) >>> 0; return rng; };
const artifact = (strokes, role, subject = 'oracle-subject') => ({
  schema_version: 1, subject_id: subject, window: { start_ns: '0', end_ns: '1000000000' },
  strokes, [role === 'reference' ? 'material_sections' : 'flags']: [],
});
function stroke(id) {
  const startMs = (random() % 9) * 50;
  const up = random() % 3 !== 0;
  const travel = [400000, 500000, 600000, 600001][random() % 4];
  return {
    id, target_id: random() % 3 === 0 ? 'other-target' : 'target', axis: 'stroke',
    start_ns: String(startMs * 1000000), end_ns: String((startMs + 200) * 1000000),
    start_position_ppm: up ? 100000 : 100000 + travel,
    end_position_ppm: up ? 100000 + travel : 100000,
  };
}
// Independent exhaustive assignment oracle: no production matcher helpers.
function bruteForce(references, predictions, r = 0, used = 0) {
  if (r === references.length) return 0;
  let best = bruteForce(references, predictions, r + 1, used);
  const reference = references[r];
  for (let p = 0; p < predictions.length; p++) {
    if ((used & (1 << p)) !== 0) continue;
    const candidate = predictions[p];
    const refSigned = reference.end_position_ppm - reference.start_position_ppm;
    const predSigned = candidate.end_position_ppm - candidate.start_position_ppm;
    const eligible =
      reference.target_id === candidate.target_id && reference.axis === candidate.axis &&
      Math.sign(refSigned) === Math.sign(predSigned) &&
      Math.abs(Number(reference.start_ns) - Number(candidate.start_ns)) <= 100000000 &&
      Math.abs(Number(reference.end_ns) - Number(candidate.end_ns)) <= 100000000 &&
      Math.abs(Math.abs(refSigned) - Math.abs(predSigned)) * 5 <= Math.abs(refSigned);
    if (eligible) best = Math.max(best, 1 + bruteForce(references, predictions, r + 1, used | (1 << p)));
  }
  return best;
}
for (let i = 0; i < 512; i++) {
  const refs = Array.from({ length: random() % 5 }, (_, r) => stroke('r' + r));
  const preds = Array.from({ length: random() % 5 }, (_, p) => stroke('p' + p));
  const actual = evaluateClip(artifact(refs, 'reference'), artifact(preds, 'candidate'));
  assert.equal(actual.axes.stroke.true_positive_strokes, bruteForce(refs, preds), 'independent exhaustive assignment ' + i);
  assert.equal(new Set(actual.matches.map((m) => m.candidate_id)).size, actual.matches.length, 'candidate reuse');
  assert.equal(new Set(actual.matches.map((m) => m.reference_id)).size, actual.matches.length, 'reference reuse');
  checks++;
}

const dense = (id) => ({ id, target_id: 'target', axis: 'stroke', start_ns: '100000000', end_ns: '300000000', start_position_ppm: 0, end_position_ppm: 500000 });
assert.throws(() => evaluateClip(
  artifact(Array.from({ length: 501 }, (_, i) => dense('r' + i)), 'reference'),
  artifact(Array.from({ length: 500 }, (_, i) => dense('p' + i)), 'candidate'),
), /matching edge budget exceeded/); checks++;
assert.throws(() => evaluateClip(
  artifact(Array.from({ length: 5001 }, (_, i) => dense('r' + i)), 'reference'), artifact([], 'candidate'),
), /expected bounded array/); checks++;
const wrongSubject = artifact([], 'candidate', 'wrong');
assert.throws(() => evaluateClip(artifact([], 'reference'), wrongSubject), /subject identity mismatch/); checks++;
const wrongWindow = artifact([], 'candidate'); wrongWindow.window.end_ns = '1000000001';
assert.throws(() => evaluateClip(artifact([], 'reference'), wrongWindow), /evaluation window mismatch/); checks++;

const temporaryRoot = fs.mkdtempSync(path.join(root, 'research/phase-b/results/.artifact-test-'));
try {
  const corpusRoot = path.join(temporaryRoot, 'corpus');
  fs.mkdirSync(corpusRoot);
  const write = (filename, value) => {
    const bytes = Buffer.from(typeof value === 'string' ? value : JSON.stringify(value) + '\n');
    fs.writeFileSync(filename, bytes);
    return crypto.createHash('sha256').update(bytes).digest('hex');
  };
  const refPath = path.join(corpusRoot, 'reference.json');
  const candPath = path.join(corpusRoot, 'candidate.json');
  const ref = artifact([dense('r1')], 'reference');
  const cand = artifact([dense('p1')], 'candidate');
  const referenceHash = write(refPath, ref), candidateHash = write(candPath, cand);
  const entry = {
    id: 'oracle-subject', category: 'synthetic-kat', split: 'development',
    source_group_id: 'authored-test-group', evidence_class: 'synthetic', source_sha256: referenceHash,
    rights: { status: 'permitted', evidence_id: 'authored-ephemeral-test-fixture', permitted_uses: ['evaluation'], redistribution: 'forbidden' },
    annotation_provenance: { reference_author: 'test-fixture', independent: false, method: 'analytic', review_record: 'synthetic-only' },
    reference: { path: 'reference.json', sha256: referenceHash }, candidate: { path: 'candidate.json', sha256: candidateHash },
  };
  const baseline = { schema_version: 1, policy_version: 'B1.1', entries: [entry] };
  const manifestPath = path.join(corpusRoot, 'corpus.json');
  const reset = () => { write(refPath, ref); write(candPath, cand); write(manifestPath, baseline); };
  reset();
  let result = benchmarkCorpus(manifestPath);
  assert.equal(result.clips.length, 1); assert.equal(result.clips[0].metrics.axes.stroke.precision, 1); checks++;
  assert.equal(result.release_qualified, false); checks++;
  assert.equal(result.clips[0].effort, null); checks++;
  write(refPath, { ...ref, strokes: [] });
  assert.throws(() => benchmarkCorpus(manifestPath), /annotation digest mismatch/); checks++; reset();
  write(candPath, { ...cand, strokes: [] });
  assert.throws(() => benchmarkCorpus(manifestPath), /annotation digest mismatch/); checks++; reset();
  const outside = path.join(temporaryRoot, 'outside-reference.json'); write(outside, ref);
  const escaped = structuredClone(baseline); escaped.entries[0].reference.path = '../outside-reference.json';
  write(manifestPath, escaped);
  assert.throws(() => benchmarkCorpus(manifestPath), /escapes corpus root/); checks++; reset();
  fs.symlinkSync(outside, path.join(corpusRoot, 'escape-link.json'));
  const linked = structuredClone(baseline); linked.entries[0].reference.path = 'escape-link.json';
  write(manifestPath, linked);
  assert.throws(() => benchmarkCorpus(manifestPath), /escapes corpus root/); checks++; reset();
  const absolute = structuredClone(baseline); absolute.entries[0].reference.path = refPath;
  write(manifestPath, absolute);
  assert.throws(() => benchmarkCorpus(manifestPath), /absolute annotation path forbidden/); checks++; reset();
  fs.writeFileSync(refPath, Buffer.alloc(8 * 1024 * 1024 + 1, 0x20));
  assert.throws(() => benchmarkCorpus(manifestPath), /exceeds size\/type budget/); checks++; reset();
  const directory = structuredClone(baseline); directory.entries[0].reference.path = '.';
  write(manifestPath, directory);
  assert.throws(() => benchmarkCorpus(manifestPath), /exceeds size\/type budget/); checks++; reset();
  assert.throws(() => benchmarkCorpus(manifestPath, 'training'), /training is not evaluation/); checks++;
  write(manifestPath, { schema_version: 1, policy_version: 'B1.1', entries: [] });
  result = benchmarkCorpus(manifestPath, 'heldout');
  assert.deepEqual(result.categories, {}); assert.equal(result.release_qualified, false); checks++;
} finally {
  // Only this process's fresh temporary directory; never user media or projects.
  fs.rmSync(temporaryRoot, { recursive: true, force: true });
}
const receipt = {
  schema_version: 1, status: 'PASS', checks, exhaustive_matching_cases: 512,
  matching_and_identity_boundaries: 4, artifact_pipeline_cases: 12,
  fixture_evidence_class: 'synthetic', real_media_subjects: 0,
  release_qualified: false,
};
const output = process.argv[2];
if (output) fs.writeFileSync(output, JSON.stringify(receipt, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
process.stdout.write(JSON.stringify(receipt, null, 2) + '\n');
