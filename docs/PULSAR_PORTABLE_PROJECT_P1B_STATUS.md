# Portable project P1b: retained export and scoped download

Status: **Export slice implemented and locally exercised on Linux. Portable clone import, GUI parity, cross-platform qualification, and the Good phase remain incomplete.**

The [portable-project plan](PULSAR_PORTABLE_PROJECT_IMPLEMENTATION_PLAN.md) remains the sequence. The software specification and architecture decisions retain authority. [ADR 0010](adr/0010-portable-project-export-lifecycle.md) records selected contracts and research; [P1a](PULSAR_PORTABLE_PROJECT_P1A_STATUS.md) remains historical foundation evidence.

## Implemented slice

- Explicit host-local packaging approval for existing projects; no pairing-token privilege escalation or automatic grant broadening.
- Durable, idempotent export operations with exact-revision capture, retained external objects, bounded admission, cancellation, and authorization-generation fencing.
- Streaming materialization, verified immutable bundles, honest publication uncertainty, and explicit release with retained tombstones.
- Separate scoped package bulk channel, bounded chunk/replay state, absolute deadlines, and asynchronous current-epoch verification after restart.
- Shared client helper and CLI: allow-packaging, package-export, package-status, package-cancel, package-download, and package-release.
- Recoverable caller-visible request IDs, same-ID transient transport retries, complete receipt hash/length verification, and no-clobber client publication.
- Finite lifecycle model, guard-negative campaigns, deterministic authority/storage regressions, and real-interface process fixtures.

New project creators already receive PackageProject. Grant is additive; reproducing legacy reduced authority requires a real revoke/regrant sequence, not merely issuing a smaller Grant.

Cancelled fences publication immediately but does not assert worker exit. Source holds and resource reservations remain until teardown. A Ready receipt is historical; restart verification runs asynchronously before new leases.

## Exact final local evidence

The final command combined the existing motion-transfer gate, package model campaign, locked offline root build, ordinary package process suite, separately selected large transfer, and actual confined-worker capture fixture.

- **420 successful Rust test executions, zero failures**, across 30 result groups.
- Five ordinary package process cases intentionally ran twice. This is an execution count, not a claim of 420 unique test identities.
- Three default ignored occurrences represented two distinct fixtures: large transfer, reported twice, and actual-worker capture. Both were separately invoked successfully and are included in 420.
- Shared client suite: 53 passed. Protocol suites: 98 passed. Engine library default pass: 154 passed with the actual-worker fixture separately invoked. Focused storage and package-lifecycle regressions are included.
- Package model: 4,088 distinct states, 15,885 generated states, twelve expected safeguard-negative counterexamples. Existing lifecycle/transfer model campaigns also passed.
- Source fingerprint: `59209395c4145c36cd5f5c4746063a4775d71d0937a56d49680f551f153249c0`.
- Linux debug executable: `df3f6c410b84c7d379414c73e93a82d1a2875ce3577744371ea4c75ff3d29798`.

### Generated-data streaming measurement

The independently checked package contained 1,074,012,347 bytes from a 1,074,003,968-byte generated source, requiring at least 4,098 capped 256 KiB chunks.

Sampled RSS growth was 1,224,704 bytes for the engine and 913,408 bytes for the client over 284 samples. The regression tripwire was 256 MiB growth per process. Import plus capture took 3.323 seconds; shared-client download took 1.346 seconds; the full large case took 6.78 seconds. Observed control queries during streaming and verification took 0.206-0.259 milliseconds.

These are observations on this host using generated marker data, not actual-media quality evidence, a proof of peak allocation, or reference-hardware throughput qualification.

Process cases exercised coherent captured state during edits/catalog changes, actual retained-source eviction rejection, active cancellation and eventual teardown, scoped permissions, changed-intent replay rejection, disconnection, lost acknowledgements, interrupted export recovery, asynchronous Ready verification, revocation/regrant lease fencing, and destination no-clobber behavior.

Artifacts: [machine-readable evidence](evidence/phase-b-portable-project/p1b.json), [final combined gate](evidence/phase-b-portable-project/p1b-final-gate.log), [model campaign](evidence/phase-b-portable-project/p1b-model.log), [review disposition](evidence/phase-b-portable-project/p1b-independent-review.md), [model review counterexample](evidence/phase-b-portable-project/p1b-model-review-red.log), [correctly failed redundant-mutant campaign](evidence/phase-b-portable-project/p1b-model-redundant-mutant.log).

## Findings, corrections, and infrastructure

Confirmed repairs cover lost-response identities, absolute control deadlines, retry classification, stale progress reporting, uncharged failed-cleanup staging, partial cleanup uncertainty, revocation/regrant races, stale verifier replacement, delayed callback overreach, final admission fencing, and failed thread creation. Filesystem enumeration was also moved out of a runtime mutex to remove a database-wait lock chain; that repair has a source argument, not a dedicated blocked-enumeration measurement.

Independent reviewers read their assigned source once. Owners reproduced findings, repaired them, and ran regressions; final combined gates passed. This is not an independent second review of repaired source or external commit approval.

Initial process fixtures incorrectly assumed a new creator lacked packaging scope and that Cancelled guaranteed immediate cleanup. Those were oracle corrections, not production authorization changes.

Shared /tmp filled during qualification, causing linker SIGBUS and ResourceExhausted. Those attempts failed. After all compilers stopped, only this task's 6.5 GiB regenerable incremental cache was removed. Source, dependency artifacts, and evidence were preserved. Final gates disabled incremental builds and placed generated-data fixtures in private disk-backed temporary storage.

## Remaining gates

1. P1c: validated clone import with fresh local identities, inert historical authority, atomic publication, replay, and crash recovery.
2. P1d: shared GUI integration, relocation and editable round trips, platform publication/installer qualification.
3. Deeper qualification: package fuzzing/mutation campaigns, broader fault injection, Kani where applicable, real-media behavior, and reference-hardware benchmarks.

Native filesystem I/O/fsync and bounded compact serde parsing are not hard wall-clock interruptible. Process death can leave private client staging; unreferenced engine orphans remain charged rather than being speculatively deleted. Long-term tombstone compaction is not solved by admission caps.

No Windows/macOS, neural accuracy, physical-device safety, or bundled model/runtime installation qualification is claimed. Existing dead-code warnings remain; this was not a clean lint gate. No commit or push was made in this slice. Earlier required commit-review attempts returned Transport closed, not PASS.
