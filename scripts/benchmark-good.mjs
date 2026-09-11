#!/usr/bin/env node
/**
 * B1 benchmark measurement, not a release qualifier.
 * No third-party dependencies; media/model bytes never leave this process.
 */
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath, pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';

const MAX_JSON_BYTES = 8 * 1024 * 1024;
const MAX_STROKES = 5000;
const MAX_CORPUS_ENTRIES = 256;
const MAX_ANNOTATION_BYTES = 64 * 1024 * 1024;
const MAX_EDGES = 250000;
const I64_MIN = -(1n << 63n);
const I64_MAX = (1n << 63n) - 1n;
const TOLERANCE_NS = 100000000n;
const AXES = new Set(['stroke', 'sway', 'surge', 'roll', 'pitch', 'yaw']);
const SPLITS = new Set(['training', 'development', 'heldout']);
const EVIDENCE_CLASSES = new Set(['synthetic', 'real-media']);
const SHA256 = /^[a-f0-9]{64}$/;
const own = (o, k) => Object.prototype.hasOwnProperty.call(o, k);
const fail = (message) => { throw new Error(message); };
const abs = (n) => n < 0n ? -n : n;
const fraction = (n, d) => d === 0 ? null : n / d;

function object(value, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail(label + ': expected object');
  return value;
}
function keys(value, allowed, required, label) {
  object(value, label);
  for (const k of Object.keys(value)) if (!allowed.includes(k)) fail(label + ': unknown field ' + k);
  for (const k of required) if (!own(value, k)) fail(label + ': missing field ' + k);
}
function text(value, label) {
  if (typeof value !== 'string' || value.length === 0 || value.length > 1024) fail(label + ': invalid string');
  return value;
}
function integer(value, min, max, label) {
  if (!Number.isSafeInteger(value) || value < min || value > max) fail(label + ': invalid integer');
  return value;
}
function ns(value, label) {
  if (typeof value !== 'string' || !/^-?(0|[1-9][0-9]*)$/.test(value) || value === '-0' || value.length > 20) fail(label + ': expected canonical decimal nanoseconds');
  const result = BigInt(value);
  if (result < I64_MIN || result > I64_MAX) fail(label + ': outside signed 64-bit project time');
  return result;
}
function interval(value, label) {
  keys(value, ['start_ns', 'end_ns'], ['start_ns', 'end_ns'], label);
  const start = ns(value.start_ns, label + '.start_ns');
  const end = ns(value.end_ns, label + '.end_ns');
  if (end <= start) fail(label + ': empty or reversed interval');
  return { start, end };
}
function contained(range, window, label) {
  if (range.start < window.start || range.end > window.end) fail(label + ': outside evaluation window');
}
function unique(items, label) {
  const seen = new Set();
  for (const item of items) {
    if (seen.has(item.id)) fail(label + ': duplicate id ' + item.id);
    seen.add(item.id);
  }
}
function array(value, max, label) {
  if (!Array.isArray(value) || value.length > max) fail(label + ': expected bounded array');
  return value;
}

export function validateAnnotation(doc, role) {
  if (!['reference', 'candidate'].includes(role)) fail('invalid annotation role');
  const allowed = ['schema_version', 'subject_id', 'window', 'strokes', role === 'reference' ? 'material_sections' : 'flags'];
  keys(doc, allowed, ['schema_version', 'subject_id', 'window', 'strokes'], role);
  if (doc.schema_version !== 1) fail(role + ': unsupported schema version');
  text(doc.subject_id, role + '.subject_id');
  const window = interval(doc.window, role + '.window');
  const strokes = array(doc.strokes, MAX_STROKES, role + '.strokes').map((s, i) => {
    const label = role + '.strokes[' + i + ']';
    keys(s, ['id', 'target_id', 'axis', 'start_ns', 'end_ns', 'start_position_ppm', 'end_position_ppm'], ['id', 'target_id', 'axis', 'start_ns', 'end_ns', 'start_position_ppm', 'end_position_ppm'], label);
    text(s.id, label + '.id'); text(s.target_id, label + '.target_id');
    if (!AXES.has(s.axis)) fail(label + ': unknown axis');
    const start = ns(s.start_ns, label + '.start_ns');
    const end = ns(s.end_ns, label + '.end_ns');
    if (end <= start) fail(label + ': non-positive stroke duration');
    contained({ start, end }, window, label);
    const from = integer(s.start_position_ppm, 0, 1000000, label + '.start_position_ppm');
    const to = integer(s.end_position_ppm, 0, 1000000, label + '.end_position_ppm');
    if (from === to) fail(label + ': plateaus are intervals, not zero-travel strokes');
    return { ...s, start, end, travel: Math.abs(to - from), direction: Math.sign(to - from) };
  });
  unique(strokes, role + '.strokes');
  const field = role === 'reference' ? 'material_sections' : 'flags';
  const sections = array(doc[field] ?? [], MAX_STROKES, role + '.' + field).map((s, i) => {
    const label = role + '.' + field + '[' + i + ']';
    keys(s, ['id', 'start_ns', 'end_ns', 'reasons'], ['id', 'start_ns', 'end_ns', 'reasons'], label);
    text(s.id, label + '.id');
    const start = ns(s.start_ns, label + '.start_ns'), end = ns(s.end_ns, label + '.end_ns');
    if (end <= start) fail(label + ': non-positive section duration');
    contained({ start, end }, window, label);
    array(s.reasons, 32, label + '.reasons').forEach((v) => text(v, label + '.reason'));
    if (s.reasons.length === 0) fail(label + ': reason required');
    return { ...s, start, end };
  });
  unique(sections, role + '.' + field);
  return { subject: doc.subject_id, window, strokes, sections };
}

function union(ranges) {
  const sorted = ranges.map((r) => ({ start: r.start, end: r.end })).sort((a, b) => a.start < b.start ? -1 : a.start > b.start ? 1 : 0);
  const result = [];
  for (const next of sorted) {
    const prev = result[result.length - 1];
    if (prev && next.start <= prev.end) { if (next.end > prev.end) prev.end = next.end; }
    else result.push(next);
  }
  return result;
}
function covered(range, ranges) {
  let total = 0n;
  for (const other of ranges) {
    const start = range.start > other.start ? range.start : other.start;
    const end = range.end < other.end ? range.end : other.end;
    if (end > start) total += end - start;
  }
  return total;
}
function lowerBound(strokes, target) {
  let lo = 0, hi = strokes.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (strokes[mid].start < target) lo = mid + 1; else hi = mid;
  }
  return lo;
}
function maximumMatching(edges, predictedCount) {
  const matchedRef = new Int32Array(edges.length).fill(-1);
  const matchedPred = new Int32Array(predictedCount).fill(-1);
  for (let root = 0; root < edges.length; root++) {
    const parentCandidate = new Int32Array(predictedCount).fill(-1);
    const seenRef = new Uint8Array(edges.length);
    const queue = [root]; seenRef[root] = 1;
    let found = -1;
    for (let q = 0; q < queue.length && found < 0; q++) {
      const r = queue[q];
      for (const p of edges[r]) {
        if (parentCandidate[p] !== -1) continue;
        parentCandidate[p] = r;
        if (matchedPred[p] === -1) { found = p; break; }
        const next = matchedPred[p];
        if (!seenRef[next]) { seenRef[next] = 1; queue.push(next); }
      }
    }
    while (found !== -1) {
      const r = parentCandidate[found];
      const previous = matchedRef[r];
      matchedRef[r] = found; matchedPred[found] = r; found = previous;
    }
  }
  return { matchedRef, matchedPred };
}
function analyze(referenceDocument, candidateDocument, mutation = null) {
  const reference = validateAnnotation(referenceDocument, 'reference');
  const candidate = validateAnnotation(candidateDocument, 'candidate');
  if (reference.subject !== candidate.subject) fail('reference/candidate subject identity mismatch');
  if (reference.window.start !== candidate.window.start || reference.window.end !== candidate.window.end) fail('reference/candidate evaluation window mismatch');
  const refs = reference.strokes.slice().sort((a, b) => a.start < b.start ? -1 : a.start > b.start ? 1 : a.id.localeCompare(b.id));
  const preds = candidate.strokes.slice().sort((a, b) => a.start < b.start ? -1 : a.start > b.start ? 1 : a.id.localeCompare(b.id));
  let edgeCount = 0;
  const edges = refs.map((r) => {
    const matches = [];
    for (let p = lowerBound(preds, r.start - TOLERANCE_NS); p < preds.length && preds[p].start <= r.start + TOLERANCE_NS; p++) {
      const c = preds[p];
      if ((mutation !== 'ignore_target' && r.target_id !== c.target_id) || r.axis !== c.axis || r.direction !== c.direction) continue;
      const tolerance = mutation === 'reject_timing_boundary' ? TOLERANCE_NS - 1n : TOLERANCE_NS;
      if (abs(r.start - c.start) > tolerance || abs(r.end - c.end) > tolerance) continue;
      const denominator = mutation === 'candidate_travel_denominator' ? c.travel : r.travel;
      if (Math.abs(r.travel - c.travel) * 100 > denominator * 20) continue;
      if (++edgeCount > MAX_EDGES) fail('matching edge budget exceeded; split evaluation into independently identified windows');
      matches.push(p);
    }
    return matches.sort((a, b) => {
      const aError = abs(r.start - preds[a].start) + abs(r.end - preds[a].end);
      const bError = abs(r.start - preds[b].start) + abs(r.end - preds[b].end);
      return aError < bError ? -1 : aError > bError ? 1 : preds[a].id.localeCompare(preds[b].id);
    });
  });
  let { matchedRef, matchedPred } = maximumMatching(edges, preds.length);
  if (mutation === 'reuse_candidate') {
    matchedRef = Int32Array.from(edges.map((e) => e[0] ?? -1));
    matchedPred = new Int32Array(preds.length).fill(-1);
    matchedRef.forEach((p, r) => { if (p >= 0) matchedPred[p] = r; });
  }
  const matches = [];
  const axes = {};
  for (const axis of AXES) {
    const referenceCount = refs.filter((r) => r.axis === axis).length;
    const predictedCount = preds.filter((p) => p.axis === axis).length;
    let truePositives = 0;
    refs.forEach((r, i) => { if (r.axis === axis && matchedRef[i] >= 0) truePositives++; });
    if (referenceCount || predictedCount || axis === 'stroke') axes[axis] = {
      reference_strokes: referenceCount, predicted_strokes: predictedCount, true_positive_strokes: truePositives,
      precision: mutation === 'empty_perfect' && predictedCount === 0 ? 1 : fraction(truePositives, predictedCount),
      recall: mutation === 'empty_perfect' && referenceCount === 0 ? 1 : fraction(truePositives, referenceCount),
    };
  }
  refs.forEach((r, i) => {
    if (matchedRef[i] < 0) return;
    const p = preds[matchedRef[i]];
    matches.push({
      reference_id: r.id, candidate_id: p.id, axis: r.axis,
      start_timing_error_ns: abs(r.start - p.start).toString(),
      end_timing_error_ns: abs(r.end - p.end).toString(),
      travel_relative_error: Math.abs(r.travel - p.travel) / r.travel,
    });
  });
  const flags = union(candidate.sections);
  const materials = union(reference.sections);
  let any = 0, half = 0, full = 0;
  const sectionCoverage = reference.sections.map((r) => {
    const coverage = covered(r, flags), duration = r.end - r.start;
    if (coverage > 0n) any++;
    if (coverage * 2n >= duration) half++;
    if (coverage === duration) full++;
    return { id: r.id, covered_ns: coverage.toString(), duration_ns: duration.toString(), fraction: Number(coverage) / Number(duration) };
  });
  const flaggedNs = flags.reduce((sum, r) => sum + r.end - r.start, 0n);
  const flaggedMaterialNs = flags.reduce((sum, r) => sum + covered(r, materials), 0n);
  return {
    subject_id: reference.subject,
    evidence_scope: 'annotation-metrics-only; no neural, physical, or release qualification',
    axes,
    matches,
    unmatched_reference_ids: refs.filter((_, i) => matchedRef[i] < 0).map((r) => r.id),
    unmatched_candidate_ids: preds.filter((_, i) => matchedPred[i] < 0).map((p) => p.id),
    timing_error_distribution_scope: 'matched strokes only; unmatched strokes remain false positives/negatives',
    review: {
      independently_annotated_material_sections: reference.sections.length,
      any_overlap_hits: any, half_coverage_hits: half, full_coverage_hits: full,
      any_overlap_recall: fraction(any, reference.sections.length),
      half_coverage_recall: fraction(half, reference.sections.length),
      full_coverage_recall: fraction(full, reference.sections.length),
      section_coverage: sectionCoverage,
      flagged_duration_ns: flaggedNs.toString(),
      flagged_duration_fraction: Number(flaggedNs) / Number(reference.window.end - reference.window.start),
      flagged_nonmaterial_duration_ns: (flaggedNs - flaggedMaterialNs).toString(),
      flagged_duration_material_precision: flaggedNs === 0n ? null : Number(flaggedMaterialNs) / Number(flaggedNs),
      release_hit_rule: 'UNAPPROVED: three descriptive coverage rules are not interchangeable release gates',
    },
    release_qualified: false,
  };
}
export function evaluateClip(reference, candidate) { return analyze(reference, candidate); }

function boundedJson(filename) {
  const fd = fs.openSync(filename, 'r');
  try {
    const stat = fs.fstatSync(fd);
    if (!stat.isFile() || stat.size > MAX_JSON_BYTES) fail('JSON artifact exceeds size/type budget: ' + filename);
    // Bound the read itself; a growing file cannot force an unbounded allocation.
    const storage = Buffer.allocUnsafe(MAX_JSON_BYTES + 1);
    let used = 0;
    while (used < storage.length) {
      const count = fs.readSync(fd, storage, used, storage.length - used, null);
      if (count === 0) break;
      used += count;
    }
    if (used > MAX_JSON_BYTES) fail('JSON artifact grew beyond byte budget: ' + filename);
    const bytes = storage.subarray(0, used);
    return { value: JSON.parse(bytes.toString('utf8')), sha256: crypto.createHash('sha256').update(bytes).digest('hex'), byte_len: used };
  } finally { fs.closeSync(fd); }
}
function relativeArtifact(root, artifact, label, budget) {
  keys(artifact, ['path', 'sha256'], ['path', 'sha256'], label);
  text(artifact.path, label + '.path');
  if (!SHA256.test(artifact.sha256)) fail(label + ': invalid sha256');
  if (path.isAbsolute(artifact.path)) fail(label + ': absolute annotation path forbidden');
  const resolved = fs.realpathSync(path.resolve(root, artifact.path));
  const rel = path.relative(root, resolved);
  if (rel === '..' || rel.startsWith('..' + path.sep) || path.isAbsolute(rel)) fail(label + ': annotation path escapes corpus root');
  const result = boundedJson(resolved);
  budget.bytes += result.byte_len;
  if (budget.bytes > MAX_ANNOTATION_BYTES) fail('aggregate annotation byte budget exceeded');
  if (result.sha256 !== artifact.sha256) fail(label + ': annotation digest mismatch');
  return result.value;
}
export function validateCorpus(manifest) {
  keys(manifest, ['schema_version', 'policy_version', 'heldout_lock', 'entries'], ['schema_version', 'policy_version', 'entries'], 'corpus');
  if (manifest.schema_version !== 1 || manifest.policy_version !== 'B1.1') fail('unsupported corpus/measurement policy');
  const entries = array(manifest.entries, MAX_CORPUS_ENTRIES, 'corpus.entries');
  const groups = new Map(), hashes = new Map(), ids = new Set();
  for (const entry of entries) {
    keys(entry, ['id', 'category', 'split', 'source_group_id', 'evidence_class', 'source_sha256', 'rights', 'annotation_provenance', 'reference', 'candidate', 'effort', 'execution'], ['id', 'category', 'split', 'source_group_id', 'evidence_class', 'source_sha256', 'rights', 'annotation_provenance', 'reference', 'candidate'], 'corpus entry');
    for (const name of ['id', 'category', 'source_group_id']) text(entry[name], 'entry.' + name);
    if (ids.has(entry.id)) fail('duplicate corpus entry id'); ids.add(entry.id);
    if (!SPLITS.has(entry.split) || !EVIDENCE_CLASSES.has(entry.evidence_class)) fail('invalid split/evidence class');
    if (!SHA256.test(entry.source_sha256)) fail('invalid source digest');
    for (const [map, key] of [[groups, entry.source_group_id], [hashes, entry.source_sha256]]) {
      if (map.has(key) && map.get(key) !== entry.split) fail('training/development/heldout leakage: ' + key);
      map.set(key, entry.split);
    }
    keys(entry.rights, ['status', 'evidence_id', 'permitted_uses', 'redistribution'], ['status', 'evidence_id', 'permitted_uses', 'redistribution'], 'rights');
    text(entry.rights.evidence_id, 'rights.evidence_id');
    const uses = array(entry.rights.permitted_uses, 4, 'rights.permitted_uses');
    if (uses.some((u) => !['training', 'evaluation', 'redistribution'].includes(u)) || new Set(uses).size !== uses.length) fail('invalid or duplicate permitted use');
    if (entry.rights.status !== 'permitted' || !['allowed', 'forbidden'].includes(entry.rights.redistribution)) fail('rights must be established before corpus admission');
    if (!uses.includes(entry.split === 'training' ? 'training' : 'evaluation')) fail('use not permitted by rights record');
    if (entry.rights.redistribution === 'allowed' && !uses.includes('redistribution')) fail('redistribution claim missing permission');
    keys(entry.annotation_provenance, ['reference_author', 'independent', 'method', 'review_record'], ['reference_author', 'independent', 'method', 'review_record'], 'annotation provenance');
    for (const name of ['reference_author', 'method', 'review_record']) text(entry.annotation_provenance[name], 'annotation_provenance.' + name);
    if (typeof entry.annotation_provenance.independent !== 'boolean') fail('annotation independence must be explicit');
    if (entry.evidence_class === 'real-media' && !entry.annotation_provenance.independent) fail('real-media references must be independent of evaluated predictions');
    if (entry.split === 'heldout') {
      keys(manifest.heldout_lock, ['frozen_at', 'owner', 'decision_record'], ['frozen_at', 'owner', 'decision_record'], 'heldout lock');
      if (!Number.isFinite(Date.parse(manifest.heldout_lock.frozen_at))) fail('invalid heldout freeze timestamp');
      text(manifest.heldout_lock.owner, 'heldout_lock.owner'); text(manifest.heldout_lock.decision_record, 'heldout_lock.decision_record');
    }
    if (entry.effort !== undefined) {
      keys(entry.effort, ['rater_id', 'session_record', 'active_edit_ns', 'review_plus_edit_ns'], ['rater_id', 'session_record', 'active_edit_ns', 'review_plus_edit_ns'], 'effort');
      text(entry.effort.rater_id, 'effort.rater_id'); text(entry.effort.session_record, 'effort.session_record');
      const active = ns(entry.effort.active_edit_ns, 'effort.active_edit_ns'), total = ns(entry.effort.review_plus_edit_ns, 'effort.review_plus_edit_ns');
      if (active < 0n || total < active || entry.evidence_class !== 'real-media') fail('invalid human effort or synthetic effort claim');
    }
    if (entry.execution !== undefined) {
      keys(entry.execution, ['wall_time_ns', 'cpu', 'gpu', 'ram_bytes', 'backend', 'model_sha256', 'runtime_sha256', 'configuration_sha256', 'preset', 'axis_mode', 'qualification_record'], ['wall_time_ns', 'cpu', 'gpu', 'ram_bytes', 'backend', 'model_sha256', 'runtime_sha256', 'configuration_sha256', 'preset', 'axis_mode', 'qualification_record'], 'execution');
      if (ns(entry.execution.wall_time_ns, 'execution.wall_time_ns') <= 0n) fail('wall time must be positive');
      for (const name of ['cpu', 'gpu', 'backend', 'preset', 'axis_mode', 'qualification_record']) text(entry.execution[name], 'execution.' + name);
      integer(entry.execution.ram_bytes, 1, Number.MAX_SAFE_INTEGER, 'execution.ram_bytes');
      for (const name of ['model_sha256', 'runtime_sha256', 'configuration_sha256']) if (!SHA256.test(entry.execution[name])) fail('invalid execution identity');
      if (!['stroke-only', 'six-axis'].includes(entry.execution.axis_mode)) fail('execution axis mode required');
    }
  }
  return manifest;
}

export function benchmarkCorpus(filename, split = 'development') {
  if (!SPLITS.has(split) || split === 'training') fail('benchmark split must be development or heldout; training is not evaluation');
  const absolute = fs.realpathSync(filename);
  const root = path.dirname(absolute);
  const parsed = boundedJson(absolute);
  const manifest = validateCorpus(parsed.value);
  const clips = [];
  const annotationBudget = { bytes: 0 };
  for (const entry of manifest.entries.filter((e) => e.split === split)) {
    const reference = relativeArtifact(root, entry.reference, entry.id + '.reference', annotationBudget);
    const candidate = relativeArtifact(root, entry.candidate, entry.id + '.candidate', annotationBudget);
    const metrics = evaluateClip(reference, candidate);
    if (metrics.subject_id !== entry.id) fail('manifest/annotation subject identity mismatch');
    const duration = ns(reference.window.end_ns, 'window end') - ns(reference.window.start_ns, 'window start');
    clips.push({ entry_id: entry.id, category: entry.category, evidence_class: entry.evidence_class, source_group_id: entry.source_group_id, metrics,
      effort: entry.effort ? {
        ...entry.effort,
        thirty_minute_subject: duration === 1800000000000n,
        stable_active_budget_met: duration === 1800000000000n ? ns(entry.effort.active_edit_ns, 'active') <= 300000000000n : null,
        stable_total_budget_met: duration === 1800000000000n ? ns(entry.effort.review_plus_edit_ns, 'total') <= 600000000000n : null,
      } : null,
      execution: entry.execution ? { ...entry.execution, realtime_factor: Number(duration) / Number(ns(entry.execution.wall_time_ns, 'wall')), qualification_claimed: false } : null,
    });
  }
  const categories = {};
  for (const clip of clips) {
    // Never mix synthetic observations with real media or hide weak categories.
    const key = clip.evidence_class + '/' + clip.category;
    const out = categories[key] ??= { evidence_class: clip.evidence_class, category: clip.category, clips: 0, axes: {}, material_sections: 0, any_overlap_hits: 0, half_coverage_hits: 0, full_coverage_hits: 0, effort_eligible_subjects: 0, active_budget_passes: 0, total_budget_passes: 0, source_groups: new Set() };
    out.clips++; out.source_groups.add(clip.source_group_id);
    for (const [axis, metric] of Object.entries(clip.metrics.axes)) {
      const counts = out.axes[axis] ??= { reference_strokes: 0, predicted_strokes: 0, true_positive_strokes: 0 };
      for (const k of Object.keys(counts)) counts[k] += metric[k];
    }
    out.material_sections += clip.metrics.review.independently_annotated_material_sections;
    for (const k of ['any_overlap_hits', 'half_coverage_hits', 'full_coverage_hits']) out[k] += clip.metrics.review[k];
    if (clip.effort?.thirty_minute_subject) {
      out.effort_eligible_subjects++;
      out.active_budget_passes += Number(clip.effort.stable_active_budget_met);
      out.total_budget_passes += Number(clip.effort.stable_total_budget_met);
    }
  }
  for (const out of Object.values(categories)) {
    out.independent_source_groups = out.source_groups.size; delete out.source_groups;
    for (const counts of Object.values(out.axes)) {
      counts.precision = fraction(counts.true_positive_strokes, counts.predicted_strokes);
      counts.recall = fraction(counts.true_positive_strokes, counts.reference_strokes);
    }
    out.any_overlap_recall = fraction(out.any_overlap_hits, out.material_sections);
    out.half_coverage_recall = fraction(out.half_coverage_hits, out.material_sections);
    out.full_coverage_recall = fraction(out.full_coverage_hits, out.material_sections);
    out.active_budget_fraction = fraction(out.active_budget_passes, out.effort_eligible_subjects);
    out.total_budget_fraction = fraction(out.total_budget_passes, out.effort_eligible_subjects);
  }
  return {
    schema_version: 1, policy_version: 'B1.1', corpus_sha256: parsed.sha256, split,
    thresholds: { reversal_timing_ns: TOLERANCE_NS.toString(), neutral_stroke_travel_relative_error_percent: 20, preview_precision_recall_floor: 0.8, preview_target: 0.9, stable_precision_recall_floor: 0.95, material_flag_recall_floor: 0.95 },
    categories, clips, release_qualified: false,
    missing_release_gates: ['Approved category taxonomy, stroke segmentation and plateau reversal policy', 'Approved material-section hit rule and false-alarm/review-burden policy', 'Permitted independent heldout corpus and sample-size/statistical decision rule', 'Candidate-bound model/runtime/hardware and full release qualification'],
  };
}

function selected(metrics) {
  return {
    tp: metrics.axes.stroke.true_positive_strokes, precision: metrics.axes.stroke.precision, recall: metrics.axes.stroke.recall,
    any: metrics.review.any_overlap_recall, half: metrics.review.half_coverage_recall, full: metrics.review.full_coverage_recall,
    flagged: metrics.review.flagged_duration_fraction,
  };
}
export function selfTest() {
  const fixturePath = fileURLToPath(new URL('../research/phase-b/fixtures/metric-kat.json', import.meta.url));
  const fixture = boundedJson(fixturePath).value;
  let checks = 0;
  for (const kat of fixture.cases) { assert.deepEqual(selected(evaluateClip(kat.reference, kat.candidate)), kat.expected, kat.id); checks++; }
  let state = 0x42a1967;
  const random = () => { state = (Math.imul(state, 1664525) + 1013904223) >>> 0; return state; };
  const template = fixture.cases.find((c) => c.id === 'perfect').reference;
  for (let i = 0; i < 256; i++) {
    const shifted = structuredClone(template);
    const offset = BigInt(random()) * 1000000n;
    shifted.window.start_ns = (BigInt(shifted.window.start_ns) + offset).toString();
    shifted.window.end_ns = (BigInt(shifted.window.end_ns) + offset).toString();
    for (const stroke of shifted.strokes) {
      stroke.start_ns = (BigInt(stroke.start_ns) + offset).toString();
      stroke.end_ns = (BigInt(stroke.end_ns) + offset).toString();
    }
    const candidate = structuredClone(shifted); delete candidate.material_sections; candidate.flags = [];
    assert.equal(evaluateClip(shifted, candidate).axes.stroke.recall, 1, 'time-translation property');
    checks++;
  }
  const invalid = [
    ['noncanonical time', (r) => { r.strokes[0].start_ns = '01'; }],
    ['time overflow', (r) => { r.window.end_ns = '9223372036854775808'; }],
    ['NaN position', (r) => { r.strokes[0].start_position_ppm = NaN; }],
    ['fractional position', (r) => { r.strokes[0].start_position_ppm = 0.5; }],
    ['unknown axis', (r) => { r.strokes[0].axis = 'vibration'; }],
    ['zero travel', (r) => { r.strokes[0].end_position_ppm = r.strokes[0].start_position_ppm; }],
    ['duplicate stroke id', (r) => { r.strokes.push(structuredClone(r.strokes[0])); }],
    ['out-of-window stroke', (r) => { r.strokes[0].end_ns = '999999999999'; }],
    ['unknown field', (r) => { r.quality_claim = 'perfect'; }],
  ];
  for (const [name, change] of invalid) {
    const r = structuredClone(template); change(r);
    assert.throws(() => validateAnnotation(r, 'reference'), undefined, name); checks++;
  }
  const emptyCorpus = { schema_version: 1, policy_version: 'B1.1', entries: [] };
  assert.deepEqual(validateCorpus(emptyCorpus), emptyCorpus); checks++;
  const sampleEntry = {
    id: 'synthetic-validation-only', category: 'synthetic-kat', split: 'development',
    source_group_id: 'synthetic-group', evidence_class: 'synthetic', source_sha256: 'a'.repeat(64),
    rights: { status: 'permitted', evidence_id: 'authored-test-fixture', permitted_uses: ['evaluation'], redistribution: 'forbidden' },
    annotation_provenance: { reference_author: 'fixture-author', independent: false, method: 'analytic-synthetic-kat', review_record: 'self-test-only' },
    reference: { path: 'not-read-reference.json', sha256: 'b'.repeat(64) }, candidate: { path: 'not-read-candidate.json', sha256: 'c'.repeat(64) },
  };
  validateCorpus({ ...emptyCorpus, entries: [sampleEntry] }); checks++;
  const invalidCorpus = [
    ['group leakage', (m) => { const e = structuredClone(m.entries[0]); e.id += '-2'; e.split = 'training'; e.source_sha256 = 'd'.repeat(64); e.rights.permitted_uses.push('training'); m.entries.push(e); }],
    ['content leakage', (m) => { const e = structuredClone(m.entries[0]); e.id += '-2'; e.split = 'training'; e.source_group_id += '-2'; e.rights.permitted_uses.push('training'); m.entries.push(e); }],
    ['rights missing', (m) => { m.entries[0].rights.status = 'unknown'; }],
    ['evaluation forbidden', (m) => { m.entries[0].rights.permitted_uses = ['training']; }],
    ['unfrozen heldout', (m) => { m.entries[0].split = 'heldout'; }],
    ['self-referenced real media', (m) => { m.entries[0].evidence_class = 'real-media'; }],
    ['fabricated synthetic human effort', (m) => { m.entries[0].effort = { rater_id: 'nobody', session_record: 'none', active_edit_ns: '0', review_plus_edit_ns: '0' }; }],
  ];
  for (const [name, change] of invalidCorpus) {
    const manifest = { ...emptyCorpus, entries: [structuredClone(sampleEntry)] }; change(manifest);
    assert.throws(() => validateCorpus(manifest), undefined, name); checks++;
  }
  const mutations = ['ignore_target', 'reject_timing_boundary', 'candidate_travel_denominator', 'reuse_candidate', 'empty_perfect'];
  const killed = [];
  for (const mutation of mutations) {
    const witness = fixture.cases.find((kat) => {
      try { assert.deepEqual(selected(analyze(kat.reference, kat.candidate, mutation)), kat.expected); return false; }
      catch { return true; }
    });
    assert.ok(witness, 'surviving metric mutation: ' + mutation);
    killed.push({ mutation, witness: witness.id }); checks++;
  }
  return { schema_version: 1, harness: 'B1.1', status: 'PASS', checks, golden_cases: fixture.cases.length, time_translation_property_cases: 256, invalid_annotation_cases: invalid.length, invalid_corpus_cases: invalidCorpus.length, mutations_killed: killed, real_media_subjects: 0, release_qualified: false };
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    const args = process.argv.slice(2);
    let corpus = null, split = 'development', output = null;
    for (let i = 0; i < args.length; i++) {
      if (args[i] === '--self-test') continue;
      if (['--corpus', '--split', '--report'].includes(args[i])) {
        const name = args[i], value = args[++i];
        if (!value || value.startsWith('--')) fail('missing value for ' + name);
        if (name === '--corpus') corpus = value; else if (name === '--split') split = value; else output = value;
      } else fail('unknown argument: ' + args[i]);
    }
    const result = corpus ? benchmarkCorpus(corpus, split) : selfTest();
    const bytes = JSON.stringify(result, null, 2) + '\n';
    if (output) {
      // Evidence receipts are append-only by filename; never silently replace prior evidence.
      fs.writeFileSync(output, bytes, { flag: 'wx', mode: 0o600 });
    }
    process.stdout.write(bytes);
  } catch (error) {
    process.stderr.write('benchmark-good: ' + error.message + '\n');
    process.exitCode = 1;
  }
}
