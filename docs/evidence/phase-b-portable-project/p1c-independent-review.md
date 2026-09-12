# P1c independent review record

Status: **Checkpoint: nine repaired defects; combined gate blocked by a scanner false positive.**
Not release qualification or external commit approval.

## Review method and boundaries

Independent owners/reviewers examined protocol, storage, planner, client,
engine, and finite-model surfaces. Later assessments used cached originals
and exact supplied deltas rather than claiming fresh whole-source reads.
Owner-run test results are labelled as such; no single final-source campaign
has been completed.

Protocol import/upload source review found no additional confirmed defect.
Storage review found no additional byte/CAS publication defect, but exposed
the model-ordering mismatch corrected below. These findings are scoped,
not proof of complete correctness.

## Provenance planner

Initial nine tests missed three defects: contradictory transitive identity
maps despite identical payloads, a forged immutable history position zero,
and native-authored/imported candidate classification overlap.

The three new regressions failed before repair. Subsequent independent
review found two more map-composition counterexamples: a missing
middle-to-ancestor mapping and a direct-only key missing its local-to-middle
mapping. Both failed before the final correction.

The final contract requires exact partial-map equality between the direct
map and composed maps, across revisions, candidates, sources, and actors.
The planner owner reported 14/14 passing tests. The independent reviewer
assessed the final supplied delta and found no further confirmed defect.
This is not a proof of arbitrary lineage graphs or unlimited input sizes.

## Engine repairs

Source-kind encoding failed both engine and actual secured-engine/client
process tests. Old-generation ACK acceptance failed a deterministic test.
The cancellation accounting reproducer admitted 40,066 bytes beyond the
configured snapshot cap: retained filesystem-headroom reservations did not
enforce this separate limit.

The approved repairs use native source-kind encoding, a final ACK fence
within the progress-update critical section, and retained canceled-upload
accounting until live connection/I/O ownership drains.

A later review found ignored expiry-thread startup failure. The user
separately approved that repair. The injected fault reproduced successful
initialization without its required sweeper; Engine::open now propagates
startup failure. The owner reported 11/11 lifecycle tests passing, including
successful retry after the injected fault. Independent review accepted the
supplied delta without claiming a daemon-startup test or independent rerun.

## Approved connection repairs

P1C-008: lookup now examines remaining same-session entries before returning
an unrelated lock-contention failure. Its deterministic status/abandon test
failed before repair and passes afterward.

P1C-009: only exactly bound authenticated occupied connections emit
PackageUploadConnectionBusy with retryable=true. Only that pair retries;
same lease, generation, buffered bytes, and original transfer deadline remain
fixed. Admission retry defaults are 20 retries, 50 ms backoff, and a two-second
contention budget. Temporary connect/handshake deadlines do not persist into
successful transfers. Generic quota/auth/protocol failures remain terminal.

Independent cached-delta review found no new confirmed finding in these
repairs. The reviewer did not rerun tests. Root subsequently ran the actual
process proxy fixture successfully; it observed the real typed rejection
before releasing the original held connection, with exact prefix preservation
and exactly one publication.

The original process handshake failure did not capture the returned reply.
The latest seven-case process run passed, as did 20 exact serial repetitions
of the reconnect case. These passes neither attribute that prior failure
nor refute the source-confirmed recovery gap. UploadStatus does not fence
connection retirement, so the fixture's immediate-reconnect assumption
needs an explicit contract.

The generic request/response journal lacks a global row/byte cap. This is a
preexisting separate limitation; MAX_IMPORTS and physical free-space checks
are not evidence of a bounded complete SQLite journal.

## Finite lifecycle model

Earlier review strengthened request binding and separated original recovery
from other-object synchronization faults. Source correspondence then exposed
an ordering error: durable original publication occurs after validation,
not at provisional-file fsync.

The corrected sequence is validation, durable original publication, other
object publication, and active SQL commit. RecoveryBeforeObjectPublication
is now separately checked by its negative configuration.

The root's aligned campaign completed with 372 distinct and 1,325 generated
states, safe exit 0, and 16 expected invariant violations with normal TLC
exit 12. Independent supplied-delta review found no further confirmed
model/runner defect. The old 402-state result is superseded, not added to
the final model count.

The model excludes exact parser bytes, graph-map composition, upload
generation/nonce races, unsealed in-flight quota accounting, and actual OS
filesystem guarantees. Rust regressions are separate correspondence
evidence, not a proof of neural, runtime, or physical behavior.

## Process, worker, and large-data evidence

Latest owner-reported ordinary process result: 7/7 passed, 12.73 seconds,
with the historical handshake uncertainty above retained.

Actual constrained Text worker import: 1/1 passed, 2.07 seconds. Exact
archived bytes survived; no original jobs/attempts became operational
destination authority. Not neural/media qualification.

Generated large-data import/re-export: 1/1 passed, 52.15 seconds; package
1,074,012,357 bytes; at least 4,098 bounded chunks. Sampled additional RSS:
engine 1,150,976 bytes, client 303,104 bytes. Maximum observed snapshot
control latency: 14.097 ms. These are local sampled streaming results, not
a general resource-bound proof or reference-hardware acceptance.

The GUI was rebuilt and launched separately from a frozen pre-repair
binary. The user confirmed visibility and normal rendering. This is not
functional tracking or portable-workflow acceptance.

## External review

P1b commit b4c038a went through the required review tool, which returned
Transport closed. No external verdict exists. Independent local review does
not replace required external review. P1c remains uncommitted and unqualified.

## Final source component campaign and combined-gate failure

Root's final-source component campaign passed 494 Rust test executions,
zero failed, exit 0. Export/import finite models passed their safe runs and
12/16 exact negative invariants respectively. Ordinary import process cases
passed 8/8; explicit actual-worker and greater-than-1-GiB cases passed 2/2.
Earlier development counts above are historical, not additive.

The combined command itself exited 1 before those components: the existing
source scanner uses unanchored ort:: and misclassifies project_package_import::
and PreparedPackageImport::open as runtime access. Exact regex reproduction
and cached source inspection confirm the false positives. The scanner has not
been changed or bypassed. Its repair approval is pending. Separate component
passes do not make this combined gate green.

The required P1b external review retry again returned Transport closed.
A post-commit tool call remains required; neither this independent review nor
commit/push is an external verdict. See p1c-final.json for exact identities,
commands, test groups, model outputs, and local measurement limits.
