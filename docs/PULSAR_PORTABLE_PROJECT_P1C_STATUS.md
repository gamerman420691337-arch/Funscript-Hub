# Portable project P1c: clone import status

Status: **IMPLEMENTED; FULL LOCAL GATE PASSED. EXTERNAL REVIEW AND BROADER QUALIFICATION REMAIN OPEN.**

The scanner blocker recorded in checkpoint `c9f4951` is resolved with the
user-approved identifier-boundary correction and regression coverage. The
complete import qualification entry point now exits 0. This does not complete
P1d, the Good phase, or release qualification.

The software specification and major roadmap remain authoritative. See
[ADR 0011](adr/0011-portable-project-clone-import.md), the
[implementation plan](PULSAR_PORTABLE_PROJECT_IMPLEMENTATION_PLAN.md), and
[current gate evidence](evidence/phase-b-portable-project/p1c-scanner-closure.json).
Previous component evidence and failed full-gate logs remain unchanged as
historical records, not current failures.

## Implemented scope

- Fresh-project clone import through shared engine APIs and CLI; no overwrite or merge.
- Bounded authenticated uploads, exact request/lease/generation/prefix replay, and same-epoch recovery.
- Full caller-byte validation even on CAS hits; checked staging and durable original recovery before other objects and active SQL publication.
- Fresh local identities, exact inert origin archives, map composition, preserved history, and conservative imported trust.
- Local editing, undo/redo, re-export, and second cloning without importing operational jobs, actors, grants, or device/human acceptance.
- Cancellation, expiry, restart, post-write ACK fencing, retained in-flight accounting, and bounded typed connection-contention recovery.

## Scanner correction

The old unanchored `ort::` alternative matched benign identifiers such as
`project_package_import::` and `PreparedPackageImport::open`.
The scanner now calls one shared matcher with Unicode XID_Continue boundaries,
optional Rust raw-identifier prefixes, exact dispatcher identifiers, and
whitespace before path separators. Actual engine/native-runtime references
remain rejected.

The full gate first runs 36 matcher regressions. Independent mutation QA also
executed the unchanged test bodies with data-URL helper substitutions:
the legacy matcher failed 18 assertions, allow-all failed 18, and reject-all
failed 19. Each run discovered 36 tests; failures were assertions, not setup,
import, timeout, or signal failures. The repaired baseline passed all 36.

This remains a conservative lexical tripwire. It does not parse comments,
strings, use trees, macros, interleaved comments, or aliases. Cargo direct
dependency checks remain separate; neither mechanism proves transitive runtime
isolation. See [mutation evidence](evidence/phase-b-portable-project/p1c-scanner-mutations.json).

## Full local qualification

```sh
PULSAR_RUN_LARGE_PACKAGE_TEST=1 PULSAR_RUN_LARGE_PACKAGE_IMPORT_TEST=1 \
  bash scripts/qualify-project-imports.sh
```

The recorded environment uses offline locked dependencies, two Cargo build
jobs, disabled incremental builds, descriptor limit 1,024, a private disk-backed
TMPDIR, and the locally installed TLC tool. Exact environment and identities
are in the JSON evidence.

| Check | Result |
| --- | --- |
| Architecture/dependency/source checks | 104 passed, zero failed. |
| Scanner regressions | 36 passed, zero failed. |
| Rust test executions | 517 passed, zero failed, 35 reported groups including zero-test groups. |
| Basic lifecycle model | 273 distinct / 888 generated states; authorization-negative mutant killed. |
| Motion-transfer lifecycle model | 852 distinct / 2,268 generated states; four negative invariants exercised. |
| Portable export model | 4,088 distinct / 15,885 generated states; 12 expected negative invariant violations. |
| Portable import model | 372 distinct / 1,325 generated states; 16 expected negative invariant violations. |
| Actual worker and large-file fixtures | Passed with explicit selection; included in the Rust execution count. |

The Rust count includes ordinary export and import suites run twice by the
gate chain. It is an execution count, not 517 unique tests. Node tests and
model states/mutants are separate counts. Earlier 494-test component evidence
is not added to this campaign.

The complete run covers shared contracts, large-motion transport, launcher
deadlines, process integration, actual constrained Text worker evidence,
typed contention, exact recovery, and greater-than-1-GiB export/import.
Compiler warnings and historical whitespace nits remain; no clean-lint claim.

## Regression-backed repairs

| ID | Defect and repair | Evidence |
| --- | --- | --- |
| P1C-001 | Contradictory ancestry maps despite equal payloads. Require exact partial-map equality: direct mapping equals the composition through each intermediate origin, for all four namespaces. | Original regression plus missing-middle and direct-only counterexamples failed before their repairs; final planner suite 14/14. |
| P1C-002 | Forged immutable history position zero when source revision is absent. Preserve exact original undo baseline. | Reproduced before repair; included in final planner suite. |
| P1C-003 | Candidate simultaneously native-authored and imported-mapped, allowing archived review metadata to be shadowed. Reject conflicting classification. | Reproduced before repair; included in final planner suite. Live conservative trust overlay remains separate from stored original fields. |
| P1C-004 | JSON-quoted source kinds did not match native enum encoding. Store checked native encoding. | Engine and secured-engine/client process reproduced the failure; latest suites pass. |
| P1C-005 | Old generation/connection could acknowledge after abandonment and replacement. Fence generation, nonce, and deadline in the same critical section as progress update. | Deterministic stale-acknowledgement regression failed before repair and passes afterward. |
| P1C-006 | Cancellation retired snapshot-cap accounting while old I/O could still write. Retain accounting until connection and I/O drain. | Reproducer exceeded configured cap by 40,066 bytes before repair; final lifecycle suite passes. |
| P1C-007 | Expiry-worker thread startup failure was ignored. Fail engine initialization if the required sweeper cannot start. | Injected startup-failure test failed before repair; current import-lifecycle suite 13/13 includes successful retry. Direct initialization, not daemon startup, is covered. |
| P1C-008 | An unrelated busy runtime entry blocked lookup of an idle target lease. Search remaining same-session entries before returning a contention error. | Deterministic target-status/abandon case failed before repair; final import-lifecycle suite passes 13/13. |
| P1C-009 | Lost-ACK reconnect could reach an occupied old connection guard and abandon instead of recovering. Emit a distinct exactly-bound retryable error and retry only that pair within bounded admission limits. | Engine typed-rejection case failed before repair. Client lost-ACK/two-busy/exact-replay, fatal-error tests, timeout tests, and actual-process contention all pass. |

Typed retry accepts only `PackageUploadConnectionBusy` with
`retryable=true`, after exact authenticated binding. Other quota,
authorization, malformed-response, and nonretryable failures remain terminal.
The same operation, lease, generation, accepted prefix, and buffered bytes
survive recovery.

Engineering defaults remain 20 retries, 50 ms backoff, and a two-second
contention budget intersected with the original transfer deadline. Temporary
connection/handshake budgets do not shorten successful transfer lifetime.
Wire version/framing are unchanged; older strict clients reject the additive
code safely instead of gaining automatic recovery.

## Current local streaming evidence

The generated import source is 1,074,003,968 bytes and package
1,074,012,357 bytes, requiring at least 4,098 chunks of 256 KiB.
Import took 11.522 seconds, original export 3.346 seconds, re-export
4.564 seconds, and prepared-handle hashing 0.624 seconds.

Across 606 samples, additional engine RSS was 1,126,400 bytes and client RSS
266,240 bytes. Maximum observed GetSnapshot latency was 4.361 ms; maximum
observed control message was 1,300 bytes. These are host-local sampled
streaming results, not hard heap/page-cache bounds, reference-hardware
acceptance, or neural-performance evidence.

The real-process contention fixture observed an actual busy reply before
releasing the held old connection, preserved the 4,096-byte prefix, and
published exactly once. The actual constrained Text worker retained exact
original evidence with zero destination operational jobs or attempts;
its backend remains unqualified. No GPU or physical device was exercised.

## Identities and evidence

Source/tooling identity: `2aad12dbceeaa8c854da354b6c000c165635e610d367d2307018069f02b2569a`.

Rebuilt binary SHA256: `6c26d87e06d66de842f957eeda3ed9fcee56586c360939466d86878d10443db7`.

[Source manifest](evidence/phase-b-portable-project/p1c-scanner-closure-source.json)
covers 206 tracked-plus-explicit non-doc inputs. The original
production-only scanner hash is unchanged because no Rust production source
changed in this repair. This is not a hermetic toolchain/build qualification.

[Complete gate log](evidence/phase-b-portable-project/p1c-scanner-closure-gate.log)
and [structured results](evidence/phase-b-portable-project/p1c-scanner-closure.json)
are new artifacts. The prior failed gate and component logs were not rewritten.

## Remaining boundaries

The historical opaque handshake failure remains unattributed: the original
reply was not recorded, and later repetitions passing do not prove its cause.
The deterministic repaired-contention case does not retroactively attribute
that separate event.

The generic request/response journal still lacks a global row/byte cap.
Import-operation limits and filesystem headroom are not such a cap.
This remains separate architectural debt.

Finite models do not prove exact parsing, lineage composition, Rust/foreign
runtime behavior, OS durability, neural quality, or physical safety.
Implementation regressions and source correspondence remain separate evidence.

Required external review of `c9f4951` was retried and again returned
`Transport closed`, not approval. Prior `b4c038a` also has no verdict.
A post-commit review call remains required for this repair. Local gates do not
substitute for those external verdicts.

P1d GUI portable workflows, relocation and platform qualification remain
unfinished, as do broader neural/reference-performance/hardware gates.
The previously visible GUI preview is still a frozen pre-repair binary with
isolated state; it was not restarted by this repair.

Unrelated `{`, `.pulsar-motion-artifacts-harness/`, and
`crates/pulsar-core/src/lib.rs.orig` remain untouched and excluded.

## Next work

1. Obtain required external review verdicts and resolve confirmed findings.
2. Continue P1d shared GUI import/export and platform/relocation qualification.
3. Keep the remaining Good-phase and release gates explicit; this local pass is not phase completion.
