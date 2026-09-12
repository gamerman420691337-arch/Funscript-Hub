# Portable project P1c: clone import checkpoint

Status: **IMPLEMENTED CHECKPOINT; FULL QUALIFICATION BLOCKED.**

Nine confirmed import defects have been repaired. A separate final-source
component campaign passed 494 Rust tests and both portable-package lifecycle
models. The combined qualification command still fails its architecture
scanner. This is a checkpoint for commit/push, not phase completion or a
release-qualified import guarantee.

P1b baseline is `b4c038a`; see the containing Git history for this checkpoint's
commit identity. The software specification and major roadmap remain
authoritative. See [ADR 0011](adr/0011-portable-project-clone-import.md) and
the [implementation plan](PULSAR_PORTABLE_PROJECT_IMPLEMENTATION_PLAN.md).

## Implemented scope

- Fresh-project clone import through shared engine APIs and CLI, with no overwrite or merge.
- Authenticated bounded upload, exact replay identities, same-epoch recovery, and strict caller-full-byte validation even on CAS hits.
- Checked provisional/sealed/verified/published storage stages and durable original-container recovery before other objects and active SQL publication.
- Pure fresh-identity planning with exact inert origin archives, transitive map composition, preserved immutable history, and conservative local imported-trust overlay.
- Mixed local editing, re-export, and second-clone preservation without creating operational jobs, original actors, or imported device/human authority.
- Cancellation, expiry, restart, generation/nonce ACK fences, and retained in-flight snapshot-cap accounting.
- Typed occupied-connection retry that preserves lease, generation, buffered bytes, and the original transfer deadline.

GUI portable import/export, cross-platform parity, relocation, reference-media
performance, neural accuracy, and hardware qualification remain outside this
checkpoint. The overall Good phase is unfinished.

## Full gate remains failed

`bash scripts/qualify-project-imports.sh`, with both large-fixture flags set,
exited 1 immediately at `phase-a-structure`. The source scanner at
`scripts/check-architecture.mjs:50` uses an unanchored `ort::` alternative.
It therefore treats `project_package_import::` and
`PreparedPackageImport::open` as native-runtime access.

The three flags are in client `lib.rs`, `project_package_import.rs`, and
`session.rs`. Cached source review and an exact regular-expression
reproduction confirm these lexical false positives. No native inference
dependency or call was added on these paths. The scanner was not weakened,
bypassed, or changed. Its identifier-boundary correction and regression need
the requested approval.

The separate component command below is **not** a successful rerun of the
combined gate. It does not replace the remaining architecture/motion/lifecycle
checks behind the failed scanner. See [failed full-gate log](evidence/phase-b-portable-project/p1c-final-gate.log)
and [blocker reproduction](evidence/phase-b-portable-project/p1c-final.json).

## Final-source component evidence

One explicit offline/locked component campaign completed with exit 0 and
**494 passing Rust test executions, zero failed**, across 29 reported groups
including zero-test groups. Earlier development runs are not added to this
count. Four tests ignored in ordinary runs were explicitly exercised later.

| Surface | Passing executions | Scope |
| --- | --- | --- |
| Core | 93 | Checked contracts, editing, DSP, perception, synthesis. |
| Clients | 66 | Shared CLI/GUI contracts, upload/download helpers, typed contention and fatal-error behavior. |
| Engine | 203 + 1 explicitly selected worker-origin case | Includes 14 planner, 17 import-storage, and 13 import-lifecycle tests. |
| Protocol | 115 | Includes six new contention/deadline tests; three of these are Unix-specific. |
| Ordinary export process | 5 | Real secured engine/client export and replay. |
| Large export process | 1 | Generated source greater than 1 GiB. |
| Ordinary import process | 8 | Includes actual engine contention observed through an unchanged-byte proxy. |
| Explicit import qualifications | 2 | Actual constrained Text worker and greater-than-1-GiB import/re-export. |

The rebuilt worker-capable binary was used for explicit worker cases.
Existing compiler warnings remain; no clean-lint or physical/neural
qualification is claimed.

Export lifecycle model: 4,088 distinct / 15,885 generated states, safe exit 0,
and 12 expected negative invariant violations with normal TLC exit 12.
Import model: 372 distinct / 1,325 generated states, safe exit 0, and 16
expected negative invariant violations with normal TLC exit 12. The models
are separate from the 494 Rust test count.

See [component log](evidence/phase-b-portable-project/p1c-final-components.log)
and [machine-readable results](evidence/phase-b-portable-project/p1c-final.json).

## Repaired defects

| ID | Defect and repair | Regression evidence |
| --- | --- | --- |
| P1C-001 | Contradictory ancestry maps despite equal payloads. Require exact partial-map equality: direct mapping equals the composition through each intermediate origin, for all four namespaces. | Original regression plus missing-middle and direct-only counterexamples failed before their repairs; final planner suite 14/14. |
| P1C-002 | Forged immutable history position zero when source revision is absent. Preserve exact original undo baseline. | Reproduced before repair; included in final planner suite. |
| P1C-003 | Candidate simultaneously native-authored and imported-mapped, allowing archived review metadata to be shadowed. Reject conflicting classification. | Reproduced before repair; included in final planner suite. Live conservative trust overlay remains separate from stored original fields. |
| P1C-004 | JSON-quoted source kinds did not match native enum encoding. Store checked native encoding. | Engine and secured-engine/client process reproduced the failure; latest suites pass. |
| P1C-005 | Old generation/connection could acknowledge after abandonment and replacement. Fence generation, nonce, and deadline in the same critical section as progress update. | Deterministic stale-acknowledgement regression failed before repair and passes afterward. |
| P1C-006 | Cancellation retired snapshot-cap accounting while old I/O could still write. Retain accounting until connection and I/O drain. | Reproducer exceeded configured cap by 40,066 bytes before repair; final lifecycle suite passes. |
| P1C-007 | Expiry-worker thread startup failure was ignored. Fail engine initialization if the required sweeper cannot start. | Injected startup-failure test failed before repair; final lifecycle suite 11/11 includes successful retry. Direct initialization, not daemon startup, is covered. |
| P1C-008 | An unrelated busy runtime entry blocked lookup of an idle target lease. Search remaining same-session entries before returning a contention error. | Deterministic target-status/abandon case failed before repair; final import-lifecycle suite passes 13/13. |
| P1C-009 | Lost-ACK reconnect could reach an occupied old connection guard and abandon instead of recovering. Emit a distinct exactly-bound retryable error and retry only that pair within bounded admission limits. | Engine typed-rejection case failed before repair. Client lost-ACK/two-busy/exact-replay, fatal-error tests, timeout tests, and actual-process contention all pass. |

For P1C-009, `PackageUploadConnectionBusy` plus `retryable=true` is emitted
only after exact authenticated lease/session/epoch/operation/generation/content
binding. Generic ResourceExhausted, authorization failures, malformed replies,
and nonretryable contention remain terminal. The client retains the same
operation, lease, generation, and buffered chunk.

Engineering defaults are at most 20 retries, 50 ms backoff, and a two-second
contention budget intersected with the original transfer deadline. These are
bounded recovery defaults, not measured OS or physical stopping guarantees.
Temporary connect/handshake deadlines do not shorten the successful transfer's
original lifetime or later per-chunk timeout. Wire version and framing remain
unchanged; older strict clients reject the new code safely rather than gaining
automatic contention recovery.

The real-process regression observed one actual typed busy reply before
releasing the held original connection, then one admitted reconnect with the
same operation/lease/generation, the existing 4,096-byte prefix preserved, and
exactly one publication. Its private proxy forwards exact captured header and
payload bytes, rather than reserializing or manufacturing an engine response.

## Historical uncertainty and separate debt

The earlier opaque process handshake failure did not capture its error reply.
Subsequent diagnostic runs and 20 exact repetitions passed. That historical
failure remains **unattributed**; the new deterministic contention test does
not retroactively prove its cause.

The generic request/response journal has no global row/byte cap. Import
operation limits and filesystem-headroom reservations do not establish such
a cap. This is preexisting architectural debt, not a new finding from the
connection repairs.

## Large-data and worker measurements

The final import fixture used a 1,074,003,968-byte generated source and a
1,074,012,357-byte package, requiring at least 4,098 chunks of 256 KiB.
Measured import time was 10.619 seconds; original export 3.144 seconds;
re-export 4.279 seconds; retained-file hash preparation 0.585 seconds.

Across 558 RSS samples, additional engine RSS was 1,105,920 bytes and
additional client RSS 282,624 bytes. Control probes included 880 during
Uploading, 248 during Validating, and 742 in other active states. Maximum
observed GetSnapshot latency was 1.648 ms; maximum observed control message
was 1,298 bytes. These are local sampled streaming results, not hard heap
bounds, page-cache bounds, reference-hardware acceptance, or neural speed.

The actual constrained Text worker's exact manifest, receipt, program, and
input bytes survived import. Destination operational jobs and attempts stayed
zero. Candidate commit was explicit; undo/redo created fresh revisions.
Its backend remains unqualified. No GPU or physical device was exercised.

## Reproducibility and provenance

Source identity: `f036ea10c003118727c04cbc7a58f7cb494f9e390b6532e57a52797344dba0ec`.

Binary SHA256: `6c26d87e06d66de842f957eeda3ed9fcee56586c360939466d86878d10443db7`.

[Source identity manifest](evidence/phase-b-portable-project/p1c-source-manifest.json)
contains 204 tracked-plus-explicit input entries, excluding docs and
unrelated untracked artifacts. The identity hashes sorted relative-path,
NUL, per-entry SHA256, NUL records; symlinks hash link-target bytes. It is a
source identity, not a hermetic-build or toolchain qualification claim.

The component run used the offline locked dependency graph, two Cargo build
jobs, disabled incremental builds, file-descriptor limit 1,024, the private
disk-backed TMPDIR, and the locally installed TLC development tool.
The exact command sequence is recorded in the JSON evidence.

## Assurance and privacy limits

The import model orders validation, durable original-container publication,
other object publication, then active SQL commit. Provisional-file fsync is
not directory durability. The model does not cover exact parser bytes, graph
composition, upload-generation/nonce races, unsealed in-flight quota accounting,
actual OS fsync semantics, foreign runtimes, or physical behavior.

Original actors, requests, jobs, receipts, and acceptance claims remain inert
data. Standard exports/device payloads do not gain authority from archives.
The staged evidence opener hashes role-bounded objects outside control/database
locks, retaining admission/capabilities and checking cancellation before and
after; no hard filesystem I/O deadline is promised.

## CLI and preview

```sh
pulsar package-import SOURCE --request-id START --begin-request-id BEGIN --seal-request-id SEAL
pulsar package-import-status OP
pulsar package-import-cancel OP
pulsar package-import-resume OP SOURCE --begin-request-id BEGIN --seal-request-id SEAL
```

Resume is same-epoch only; interrupted imports require a deliberate new Start.
Important project imports remain unqualified pending the full gate and review.

The user confirmed a separately launched GUI visible and rendering normally.
That running preview is a frozen pre-repair binary with isolated state and
was left untouched. See [preview evidence](evidence/phase-b-portable-project/p1c-gui-preview.json).
Visibility does not establish tracking, detector, media, or import correctness.

## Review and remaining work

Independent cached-delta review found no new confirmed defects in the
P1C-008/009 repairs. Owner-run tests and root's separate component campaign
are distinct from that source review.

Required external review of P1b `b4c038a` again returned `Transport closed`;
no external verdict exists. External review must be attempted after the
checkpoint commit too. Commit/push must not be described as external approval.

1. Obtain approval for the scanner identifier-boundary repair and regression.
2. Rerun the complete unmodified qualification entry point after that repair; do not relabel the separate component run as a full pass.
3. Obtain the required external review verdict and resolve confirmed findings.
4. Continue P1d GUI/platform/relocation qualification and the remaining Good phase.

Unrelated `{`, `.pulsar-motion-artifacts-harness/`, and
`crates/pulsar-core/src/lib.rs.orig` are excluded from the checkpoint commit.
