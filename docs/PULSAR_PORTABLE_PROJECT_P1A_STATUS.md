# Portable project P1a: contracts, capture, and codec

Status: **Internal P1a foundation implemented and locally exercised. Portable export/import and the Good phase remain incomplete.**

The [portable-project plan](PULSAR_PORTABLE_PROJECT_IMPLEMENTATION_PLAN.md) remains the implementation sequence. The software specification and architecture decisions retain authority. See [ADR 0009](adr/0009-portable-project-contracts.md) for design and research rationale.

## Implemented slice

- Strict typed logical manifest records, checked counts/lengths, canonical ordering, exact graph relationships, content-addressed objects, and role unions.
- Versioned uncompressed streaming codec with a 52-byte header, independent known-answer vector, exact hashes/lengths/EOF, bounded framing, short/interrupted I/O handling, and provisional sinks.
- Engine-private authorized capture of one SQL snapshot at an expected head revision and event cursor. It preserves all revisions, retained history/restoration relationships, protection, candidates, source kinds/pin intent, compact lineage, raw recovery cells, and inert origin/export evidence.
- Exact original worker-program bytes remain separate from canonical motion when their digests differ. Raw receipts are not reconstructed or reserialized under their original identity.
- Exact project ownership is applied before legacy recovery budget accounting. Primary worker sources must match captured source identities; absent executable dependencies remain identity-only.
- Incremental metadata accounting, a retained memory reservation, and indexed candidate/revision/job lookups. No external object hashing occurs under the authority database mutex.
- A separate PackageProject scope is granted only to newly created project creators. Existing grants are not broadened. Existing-project package authorization still needs the explicit P1b workflow.

Capture and its object resolver remain private to engine authority. There is no new package command, importer, exporter, or GUI control. A capture plan reports NeedsMaterialization and does not promise durable availability of external objects. Generic codec I/O also has no independent wall-clock deadline or authorization enforcement.

## Exact local evidence

Final command sequence used an FD limit of 1024, two Cargo build jobs, offline dependencies, and the existing motion-transfer qualification script, followed by a fresh root binary build and the explicitly selected real-worker closure test.

- Combined result: **343 passed, 0 failed**, across 26 Rust test-result groups.
- One real-worker test is explicitly ignored in default runs and was separately invoked successfully. That successful execution is included in 343.
- Final protocol suite: 81 passed, including 15 package-contract tests.
- Codec: 13 passed, including an independent literal-byte golden vector and adversarial framing cases.
- Durable closure fixtures: five ordinary tests plus the separate actual confined-worker test.
- Capture review regressions: four passed, including two demonstrated behavioral failures, a positive identity-only control, and a 10,000-candidate/10,000-revision/128-job-bucket membership fixture.
- Existing bounded lifecycle and transfer model campaigns passed. These are not package lifecycle proofs. Kani was not run.
- Source fingerprint: `4b3f30457e82aedc5de50ea211ac1d3089ba143009f791c34b228d57828215a4`.
- Linux debug executable: `631a4e3ade22aa0d1e2d106af209fefdbb34b3f95f5543cd7494b73459a02e88`.

The golden package is 1652 bytes, with SHA-256 `1e9f93581dd9e32be46d2dc982f776b803eb2f83d560036ec2e82b083a6508a8`. Its expected bytes came from a handwritten manifest and independent Node cryptographic/framing calculation, not a Pulsar encoder round trip.

Artifacts: [machine-readable increment](evidence/phase-b-portable-project/p1a.json), [final gate](evidence/phase-b-portable-project/final-gate.log), [golden vector](evidence/phase-b-portable-project/p1a-golden-vector.json), [scoped review record](evidence/phase-b-portable-project/independent-review.md), [checkpoint review failure](evidence/phase-b-portable-project/checkpoint-review.json).

## Findings and evidence limits

Six protocol review findings had RED-to-GREEN regressions. Actual confined-worker generation exposed an additional missing raw-program reference, which was represented explicitly rather than suppressed. Two capture behavior defects also had RED-to-GREEN tests; the quadratic scan repair has a source complexity argument and scale correctness fixture, not a timing benchmark.

A prior 339-pass gate predates the final capture repairs and is retained only as [historical evidence](evidence/phase-b-portable-project/pre-capture-review-gate.log). Independent reviewers read their assigned source once; owner verification and regression checks do not constitute a second independent final-source review. Required external review of checkpoint f24dfca failed with `Transport closed`; no external approval is claimed.

The durable fixture's initial provenance-count mismatch was a test-oracle mistake: receipts count contiguous intervals, not action knots. Correcting that expectation was not a product fix.

## Remaining next work

1. P1b: purpose-specific package operations, existing-project authorization, consistent artifact holds, cancellation/revocation, resource admission, streamed export, and durable non-clobber publication.
2. P1c: strict semantic clone import, fresh identity mapping, inert origin archives, atomic publication, request replay, and recoverable crash ordering.
3. P1d: shared CLI/GUI workflows and a relocated editable-project round trip, including large-media and platform qualification.

No multi-GiB resource/performance measurement, import crash/fault campaign, package fuzz/mutation campaign, neural accuracy, physical-device qualification, or Good-phase completion is implied.
