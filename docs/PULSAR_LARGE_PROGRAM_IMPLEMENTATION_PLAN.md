# Large Motion Artifact Transport: Next Good-Phase Work Package

Date: 2026-09-11
Status: Implemented with local contract gates; see [current checkpoint](PULSAR_PHASE_B_LARGE_MOTION_STATUS.md) for remaining qualification.
Parent: [ADR 0007](adr/0007-large-motion-artifacts.md)
Phase status: [Good-phase checkpoint](PULSAR_PHASE_B_STATUS.md)

This work closes the source-confirmed full-detail program-size blocker before reference-length quality/performance campaigns. It does not replace remaining B1-B6 deliverables. The wire-major transition and capacity numbers below are proposed technical defaults, not new product-owner decisions or reduced product scope.

## 1. Contract and compatibility

Wire v2 uses bounded motion descriptors instead of inline programs. Preserve documented CLI syntax. Legacy wire requests receive bounded, typed upgrade errors before effects; do not silently replace a running engine or interrupt its jobs to force compatibility.

A MotionDescriptor binds engine-owned artifact identity, SHA-256, exact byte length, codec/schema, project, and revision or candidate/base revision. It contains bounded axis/action/gap summaries, never client-selected filesystem paths. Descriptor possession grants no access.

Keep existing control and chunk bounds: 1 MiB control messages and 256 KiB bulk chunks. Proposed initial artifact admission is 64 MiB per program and 1,000,000 total actions, subject to aggregate RAM/storage reservations and measured validation costs. These are bounded implementation defaults, not qualified product capacity claims. Reject insufficient resources before admission; do not silently thin motion.

## 2. Minimal API

| Command | Authority and result |
| --- | --- |
| BeginEditUpload { length, sha256, label } | Requires Edit, project and expected revision. Reserves staging resources and returns a scoped lease. |
| TransferStatus { lease } | Requires the owning authenticated session. Returns accepted prefix, expiry and engine epoch. |
| FinishEditUpload { lease } | Rechecks Edit, complete length/digest and semantic validity. Creates an engine-owned candidate bound to the captured base; never commits. |
| BeginMotionDownload { locator, offset, length } | Requires Read. Resolves actual project/revision/candidate ownership and a checked range. |
| AbandonTransfer { lease } | Requires the owning session. Releases staging resources after writers stop. |

Reuse CommitCandidate, RebaseCandidate, undo/redo and protected-edit enforcement. GUI edit helpers upload a proposal, receive a candidate, then commit it through existing authority. Do not introduce a second commit authority.

## 3. Upload trust boundary

The edit-values-json-v1 upload codec contains checked axes, times, positions and explicit gaps only. Reject caller-supplied evidence, lineage, model/backend qualification and worker-authority fields.

The engine compares values against the pinned base, inherits ancestry only across genuinely unchanged spans and labels changed spans as authored using the current Synthesized convention. Changed-span analysis includes adjacent interpolation influence, not merely inserted timestamps.

The engine assigns candidate identity and provenance. Its receipt links upload digest, base artifact, edit-kernel version, actor, changed spans and resulting program digest. The input upload digest is not the digest of the resulting engine program. Edit, Generate and Read remain separate grants.

## 4. Dependency-ordered implementation

Paths are planned targets, relative to the repository.

| Step | Files and ownership |
| --- | --- |
| 1. Freeze protocol contracts | Update crates/pulsar-protocol/src/lib.rs; add motion.rs and bulk.rs. Define descriptors, leases, edit-values codec, metadata replies and version handling. |
| 2. Checked edit semantics | Add crates/pulsar-core/src/edit_values.rs and register it in core lib.rs. Implement bounded validation and ancestry-preserving, interpolation-aware edit transitions without effects. |
| 3. Durable motion storage | Add crates/pulsar-engine/src/motion_artifacts.rs; update authority.rs, artifacts.rs and resources.rs. Store immutable program objects and SQLite references for projects/revisions/candidates. Migration preserves recoverable original program bytes. |
| 4. Transfer broker | Add engine transfers.rs and bulk.rs; update engine lib.rs. Reuse protocol framing.rs chunk codec and transport.rs deadline machinery over a separate secured bulk endpoint. Existing chunk framing supplies no authorization or lease enforcement. |
| 5. Production engine integration | Update authority.rs and worker-publication acceptance. Route import, merge and history through motion storage. Metadata replies must not hydrate entire programs. Remove inline-limit assumptions only after the replacements exist. |
| 6. Shared clients | Add crates/pulsar-clients/src/motion.rs; update session.rs, lib.rs, desktop.rs and editor.rs. Share authenticated transfer, cache and validation code between CLI and GUI. |
| 7. Qualification | Add protocol/core/engine contract tests and root subprocess integration tests. Record an exact source/artifact-bound gate receipt and independent review. |

Each implementation owner must state exact file ownership before editing, preserve concurrent work, and retain current tested behavior until its replacement passes.

## 5. Lifecycle and resource invariants

- A lease binds actor, project, direction, artifact/upload, byte range, grant and engine epoch.
- Authenticate the bulk handshake and recheck the relevant grant before each chunk. Revocation fences subsequent chunks and cancels solely authorized transfers. Already-delivered bytes cannot be recalled.
- Enforce sequential offsets and bounded chunk headers. Reject overflow, unexpected lengths, conflicting replay and overlap. Exact acknowledged chunk retries may return the prior acknowledgment without rewriting.
- Bound whole-operation deadlines, active leases, staged bytes and parsing memory. Disconnection alone is not revocation.
- The initial restart policy may explicitly invalidate unfinished leases. It must not pretend partial uploads survived. Finalized candidates and commit outcomes remain durable and idempotent.
- Verify complete length and digest before parsing or publishing. Reserve semantic-validation memory before allocation.
- Downloaded programs are usable only after complete digest verification. Cache keys include artifact identity; partial files remain unusable.
- Write and fsync the immutable object before transactional publication. Orphans can be collected; committed objects and compact lineage are not disposable cache.
- Prevent raw filesystem access, unscoped artifact references, ambient device access and fabricated execution authority across the new boundary.

## 6. Acceptance matrix

| Gate | Required evidence |
| --- | --- |
| Reference stroke detail | 54,000 deterministic stroke actions fetched, edited, committed, undone/redone, reopened and exported without lost timestamps/actions. |
| Six axes | 324,000 actions through the same path within declared aggregate budgets. |
| Trust | Forged observed evidence, provenance, qualification and worker fields reject. |
| Revision | Concurrent edits cause stale commit rejection; explicit rebase creates a fresh binding. |
| Protection | Large edits/uploads cannot bypass protected regions or history rules. |
| Authorization | Cross-project references, stolen lease IDs and wrong-grant operations reject. |
| Transfer faults | Bad hashes, offsets, overlaps, truncation, expiry and revoked leases publish nothing. |
| Recovery | Disconnect/reconnect, lost acknowledgments, engine restart and duplicate finalization have explicit tested outcomes. |
| Storage faults | Disk-full, fsync and transaction failures leave no partial revision or leaked reservation. |
| Transport | Every control response remains at most 1 MiB and every bulk chunk at most 256 KiB. |
| Exact export | Independent expected action/time references match full exports; internal provenance stays out of standard payloads. |
| Performance | Measurements include the completed artifact path, not inline fixtures or decoder-only timing. |

Passing these gates closes this transport work package only. Real-media quality, human review, bundled inference, full modality support, device integration and physical/platform qualification remain separate.


## Large-motion and startup checkpoint: 2026-09-11

See [PULSAR_PHASE_B_LARGE_MOTION_STATUS.md](PULSAR_PHASE_B_LARGE_MOTION_STATUS.md) and [ADR 0008](adr/0008-motion-artifacts-and-transfer-leases.md). Immutable motion transport, values-only edit proposals, durable request/replay fixes and bounded startup are implemented. Final local gate under a 1,024-descriptor limit passed 305 Rust tests plus bounded lifecycle/model mutation checks; final binary GUI reopen/export parity passed for a 180,001-action synthesized project. Source fingerprint: `8b12bd2343e9c10b16485f517bc8a28af33b0a2b323ce625bb45965bae05bc1f`.

This replaces the earlier unimplemented large-motion blocker status only. Good Phase B remains IN PROGRESS; no whole B milestone, Preview/Stable, real-media quality, reference-hardware speed, physical stopping or full memory/control-latency qualification is implied. [Portable-project work](PULSAR_PORTABLE_PROJECT_IMPLEMENTATION_PLAN.md) is the next proposed B6 slice, not an implemented feature. The software specification remains authoritative.
