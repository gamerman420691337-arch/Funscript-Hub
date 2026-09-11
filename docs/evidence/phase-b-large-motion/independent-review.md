# Independent review: large motion transport slice

Recorded: 2026-09-11 16:59:12 UTC.
Reviewer: /root/phase_a_core.

## Disposition

No remaining confirmed P1/P2 finding in the reviewed storage, client, and engine
transfer slice after the repairs below. This is a bounded code review and targeted
test record, not completion of the Good phase, full integration qualification,
formal proof, neural-quality evidence, or physical-device qualification.

The reviewer authored the core edit-values kernel. Its test results below are
author evidence, not an independent review of that kernel. Root performs the
separate review and final integrated qualification.

## Confirmed findings and repair disposition

### P2: successful completed-upload aliases did not claim their request identity

Surface: crates/pulsar-engine/src/transfers.rs, finish_edit_upload completed
edit_upload_receipts shortcut. The repaired branch is near line 620.

After lease finalization with request A, a fresh FinishEditUpload request B could
return the completed candidate without recording B in requests or
request_identities. Reusing B for BeginEditUpload could therefore allocate a new
transfer instead of returning RequestConflict.

The implementation owner reproduced the pre-repair failure with the existing
engine fixture: the conflicting request returned a Transfer. The reviewer did
not independently run that RED version because compilation was temporarily
blocked by a separately acknowledged formatting-header artifact.

Repair: the completed-result shortcut now constructs its candidate response,
calls remember, and commits an IMMEDIATE transaction before success. The database
mutex remains held across authentication, replay checking, and this transaction.

The reviewer independently read the repaired branch and ran the complete focused
transfer suite. The new regression checks both durable journal rows, conflicting
reuse rejection without a new reservation/candidate, and restart replay.

Sibling audit: AbandonTransfer already records its response and shared identity
transactionally before releasing the lease, and checks replay before looking up
a potentially swept lease. Begin replay is backed by transfer_begins and the
shared identity table. No equivalent successful mutation shortcut was found.

Disposition: closed for the demonstrated case; independent GREEN confirmed.

### P2: a transient unordered drag could render through an explicit gap

Surface: crates/pulsar-clients/src/editor.rs, available_segments.

Actual-source reviewer probe used draft timestamps [0, 80, 40, 100] and explicit
gap [50, 60). Before repair it emitted [(0, 50), (60, 80), (40, 100)], drawing the
last segment through unavailable motion. The probe exited 101.

Repair: any non-strictly-increasing transient draft emits no continuous segments.
Point handles remain available; gesture finalization still sorts and validates
the proposed track. Crossing and duplicate-order regressions were added.

The reviewer recompiled the same actual-source probe after repair. It emitted no
segments and exited 0. The independent client library suite passed 37 tests.

Disposition: closed for the demonstrated case; independent RED and GREEN observed.

## Independently executed gates

All commands ran locally, without commits, physical actuation, or GPU work.

- Engine:
  CARGO_TARGET_DIR=/tmp/pulsar-phase-a-target CARGO_BUILD_JOBS=2 cargo test -p pulsar-engine --lib authority::transfers::tests --offline
  Exit 0; 11 passed, 0 failed, 0 ignored, 83 filtered out.
- Clients, earlier reviewed checkpoint:
  CARGO_TARGET_DIR=/tmp/pulsar-phase-a-target CARGO_BUILD_JOBS=2 cargo test -p pulsar-clients --lib --offline
  Exit 0; 37 passed, 0 failed, 0 ignored.
- Immutable motion storage, earlier reviewed checkpoint:
  CARGO_TARGET_DIR=/tmp/pulsar-motion-artifacts-target CARGO_BUILD_JOBS=2 cargo test --manifest-path .pulsar-motion-artifacts-harness/Cargo.toml --offline -- --nocapture
  Exit 0; 13 passed, 0 failed, 0 ignored. This temporary harness referenced the
  actual engine module rather than a copied implementation.
- Storage sizing fixtures preserved 54,000 stroke actions in 3,169,071 canonical
  bytes and 324,000 six-axis actions in 19,014,348 canonical bytes.
- The final focused engine gate emitted a dead-code warning for CandidateState
  candidate_id and project_id fields. This was not a behavioral finding.

These earlier storage/client checkpoints are not substitutes for root's final
frozen-source workspace and process gates.

## Reviewed contracts

- Engine files: transfers.rs, motion_state.rs, bulk.rs, relevant authority.rs
  dispatcher/authorization/request-journal/commit/rebase paths, lineage.rs, and
  artifact helpers used during publication.
- Storage: current-user private regular files, no-follow directory anchoring,
  canonical full-SHA identity, no-replace publication, file/directory sync order,
  corruption rejection, aggregate action/gap decode bounds, and restart behavior.
- Client: project/revision/candidate binding before cache lookup; complete digest
  and count checks before exposure; endpoint redirect rejection before bearer
  handshake; absolute bulk deadlines; candidate-only upload finalization; a
  separate pinned commit; and preserved explicit gaps with bounded lookups.
- Engine: authorization and shared request-identity checks before command effects;
  immutable captured-base reconciliation; final authority/lease rechecks in the
  publication transaction; expired-lease sweeping; revocation and restart fences;
  reservation retention during finalization; and descriptor lookup through
  project/revision or project/candidate ownership rather than artifact possession.
- The repaired coarse-protection path uses checked protected-segment comparison
  for engine-authored edit candidates. Unchanged knot evidence is not discarded
  merely because an adjacent unprotected interpolation segment changed.
- No core whole-bracket gap-suppression defect was confirmed. The client renderer
  clips the explicit unavailable interval, matching the reviewed influence model.

## Core author evidence

The earlier targeted edit-values suite passed 15 tests, including strict nested
rejection of uploaded authority fields, required explicit gaps, checked values
and aggregate limits, point-versus-span provenance, protected anchors, deletion,
interpolation influence, generated coverage properties, and a 324,000-action
six-axis no-thinning fixture.

This is author testing, not independent assurance. Edit-values parsing bounds
individual arrays; aggregate action/gap checks follow track deserialization.
Engine admission reserves a separate validation-memory budget before parsing.
Exact authored/inherited ranges are not silently discarded to fit review UI
limits; the engine stores the exact receipt and labels review envelopes as
conservative.

## Limits and exclusions

- No claim of exhaustive race coverage, all filesystem fault behavior, or
  power-loss qualification follows from the targeted tests.
- Same-OS-user unsandboxed malicious mutation is explicitly outside the
  read-only-permission defense-in-depth claim.
- Non-Unix storage/bulk support remains fail-closed and unqualified.
- Real large-program GUI behavior, integrated process scenarios, and final
  frozen-source workspace qualification are root-owned gates.
- Formal-model assumptions were not treated as implementation proof. No new
  Kani, TLC, hardware, real-media accuracy, or model qualification was performed
  by this review.
- Source review was read-only. The reviewer writes only this new evidence record.


## Launcher follow-up

Recorded: 2026-09-11 17:30:24 UTC.

Read-only scope: src/main.rs, crates/pulsar-protocol/src/transport.rs,
crates/pulsar-engine/src/instance.rs, and tests/launcher_connect_deadline.rs.
No remaining confirmed production P1/P2 finding in this narrowly reviewed path.

The implementation owner reported two actual pre-repair failures: a genuinely
saturated accept queue left the old blocking launcher connect running beyond the
test deadline, and a competing child exited before an existing lock owner had
published its listener. The reviewer did not independently run those original
production RED versions.

The repaired launcher uses the bounded local connector. Only NotFound and
ConnectionRefused permit an absent-engine result; busy, timeout, and inaccessible
errors leave the existing endpoint untouched. Initial probes, retries, and
contender-exit handling retain one startup budget. An exited contender waits for
the existing owner only when the advisory instance lock is actually held; the
loop never spawns another child. Lock contention is not treated as readiness or
authentication.

The instance helper independently passed five focused tests. It opens an existing
checked directory and lock without creating, truncating, or unlinking either.
Only WouldBlock means held; unsafe file types, symlinks, permissions, and other
errors remain failures. A successful probe releases only its own temporary lock.

A separate fixture portability defect was independently reproduced: under
RLIMIT_NOFILE=1024, the original default-backlog test exhausted descriptors at
tests/launcher_connect_deadline.rs:64 before proving saturation. The command
exited 101 with EMFILE; this was a fixture failure, not a launcher-behavior
failure. The repaired test uses socket2's safe listen(1) API and verifies actual
backpressure with two connected sockets followed by NotConnected, rather than
inferring saturation from a short scheduling deadline.

Independent follow-up gates:

- CARGO_TARGET_DIR=/tmp/pulsar-phase-a-target CARGO_BUILD_JOBS=2 cargo test -p pulsar-engine --lib instance::tests --offline
  Exit 0; 5 passed, 0 failed, 0 ignored.
- ulimit -n 1024; CARGO_TARGET_DIR=/tmp/pulsar-phase-a-target CARGO_BUILD_JOBS=2 cargo test --test launcher_connect_deadline --offline -- --nocapture
  Exit 0; 2 passed, 0 failed, 0 ignored. The fixture reported two connected
  sockets followed by NotConnected. Both busy-listener preservation and delayed
  existing-owner negotiation passed.

The production launcher gate is Linux-local evidence. It does not establish
cross-platform launch qualification, precise timing guarantees under arbitrary
OS/filesystem stalls, or completion of the overall phase. Root owns the final
entire-workspace/process/GUI checkpoint after this source freeze.

