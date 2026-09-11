# ADR 0007: Move large motion programs off the control-message path

- Date: 2026-09-11
- Status: proposed implementation follow-up; not implemented.
- Authority: existing architecture requirement for bounded control messages
  separate from bulk artifacts.
- Priority: Good-phase release-readiness blocker, before performance claims.

## Confirmed limitation

The engine currently limits a serialized motion program to 512 KiB. Candidate
publication rejects larger worker artifacts. GetCandidate and project/editor
surfaces also carry complete programs inline, while framed control messages are
limited to 1 MiB.

The video worker currently retains an action for each sampled frame with usable
evidence. A 30-minute, 30-FPS full-detail trace has roughly 54,000 actions; even a
conservative lower bound exceeds 2.48 MB. It cannot pass publication. This is a
source-confirmed bound, not an executed 30-minute benchmark. Missing observations
can reduce the number of actions, but missing evidence is not a valid capacity
solution.

Increasing only PROGRAM_LIMIT would move the failure into the transport.
Increasing the global control-frame limit would violate the accepted separation
of control and bulk data. Lossy downsampling is not a substitute for capacity.

## Proposed implementation shape

Keep small versioned control envelopes. Add authenticated, authorization-scoped,
immutable motion-artifact descriptors and bounded bulk upload/download leases.
Descriptors bind project, candidate or revision, content kind/schema, exact byte
length, digest, and resource admission. They are opaque capabilities, not file
paths selected by clients.

Candidate and project metadata queries should return descriptors for large
programs. Legacy inline queries must either remain bounded or return an explicit
typed upgrade/capability error. GUI and CLI must use the same descriptor and
artifact validation path.

Large edits should submit a checked artifact reference plus expected revision
and request identity, not copy an entire program into a control envelope.
Engine validation, authorization, protection checks, revision fencing, and
durable idempotency remain authoritative at commit. Uploading an artifact does
not authorize committing it.

Artifact transfer must validate chunks, offsets, declared length and final digest;
bound incomplete transfers and expiry; revoke access without cancelling unrelated
work; and avoid publishing partial bytes. No arbitrary path, SQL, or shell
surface is introduced. Existing worker bulk egress and frame-artifact transport
are mechanisms to audit/reuse, not evidence that motion transfers already work.

Client rendering may retain an admitted local immutable program cache. Timeline
window queries and virtualized rendering can follow where needed. They cannot
change project timing, evidence labels, or full-detail export semantics.

## Required regression gates

1. Publish, fetch, edit, undo/redo, persist/reopen, and export a deterministic
   30-minute full-detail stroke program without lost timestamps or actions.
2. Repeat with six axes and aggregate admitted memory/storage budgets.
3. Reject over-budget length before allocation; reject bad digests, overlapping
   chunks, expired/revoked leases, stale revisions and cross-project references.
4. Exercise interrupted transfers, reconnect/resynchronization, duplicate
   requests and disk failures without partial commits or leaked authority.
5. Keep ordinary control messages within their current bound.
6. Compare the final exported data against independent exact references.
7. Bind performance measurements to the actual end-to-end artifact path.

Exact artifact capacity, wire additions, storage layout and migration details
remain implementation decisions. No implementation or qualification is implied
by this ADR.


## Implementation checkpoint: 2026-09-11

The transport proposal is now implemented as [ADR 0008](0008-motion-artifacts-and-transfer-leases.md), with [source-bound local evidence](../PULSAR_PHASE_B_LARGE_MOTION_STATUS.md). This supersedes this ADR's earlier unimplemented status for the transport slice only. Good Phase B, resource/performance qualification and product scope remain unfinished.
