# ADR 0001: Phase A executable boundaries and fail-closed migration

Date: 2026-09-11
Status: Selected implementation defaults under ARCH-001-041; qualification pending

## Context and authority

The product owner approved four Rust libraries, one executable with separate
process roles, one default engine per OS user, a checked pure core, identical
GUI/CLI authority, and architecture-first delivery. Those are user decisions.
Concrete schema/storage versions, Linux confinement mechanisms, bootstrap files,
resource constants and initial adapter restrictions are implementation choices,
not additional product-owner approvals or changes to the software specification.

## Decision

Use `pulsar-core`, `pulsar-protocol`, `pulsar-engine` and `pulsar-clients` as actual
workspace libraries. The executable dispatches roles only. `PulsarDesktop` and
CLI submit the same engine operations. Historical orchestration is quarantined,
not imported as a convenient alternate authority path.

The core supplies checked domain values and pure transitions. The engine owns
durable state and effects. Serialized input must traverse checked deserialization;
pure-core tests alone cannot certify an engine adapter that bypasses a predicate.
Correspondence tests therefore exercise the authoritative implementation too.

Use bundled SQLite with explicit transaction boundaries and durable mutation
outcomes. Public session identity is not a credential: requests carry a separate
secret token. Event payloads can name a session without disclosing authentication.
ManageProtection and ImportSource are separate scopes from ordinary editing.
History records protection state as well as motion. Read-only query responses
must not become an unbounded durable request cache.

Time-seek requests and exact displayed-frame requests have different matching
semantics. Seek results echo the requested timestamp and request generations,
then identify the actual decoded frame and rational timestamp. Rendering adopts
that actual frame context and requires observations to match it exactly. A
neighboring frame cannot satisfy an exact-frame request merely by being internally
consistent. Missing inference remains unavailable, not a fabricated tracking box.

Linux native workers run under engine-owned confinement with immutable inputs,
bounded artifact egress and process-tree resource limits. Engine alone publishes
artifacts and commits projects. Active source/model/runtime dependencies remain
leased across admission and execution. Storage admission accounts for aggregate
use/reservations rather than repeating an unreserved per-file free-space check.
Control messages, output bytes, deadlines and attempts have explicit bounds.
Unsupported confinement must reject work; it must not silently run unrestricted.

Physical devices, assistant-native effects and missing backend implementations
remain disabled behind explicit contracts. A role manifest or enum does not
qualify that capability. Development use of installed FFmpeg, bubblewrap or a
downloaded ONNX library does not satisfy the portable offline package requirement.

## Rejected alternatives

| Alternative | Reason rejected |
| --- | --- |
| GUI wraps a background CLI terminal | Duplicates presentation/process semantics instead of sharing typed authority. |
| Engine imports old GUI/batch pipeline | Retains hidden effects and duplicated project ownership. |
| Session ID serves as bearer token | Public lifecycle events can leak cross-project authority. |
| Per-process memory limit alone | Decoder grandchildren and concurrent jobs can exceed aggregate budgets. |
| Worker writes final project/artifact files | Crosses authority boundary and weakens publication validation. |
| Pretend unavailable algorithms are qualified fallbacks | Conceals product gaps and corrupts evidence semantics. |
| Treat passing lifecycle model as complete proof | Omits implementation correspondence, foreign runtime behavior and physical reality. |

## Consequences and limits

This is a narrower functioning architecture build, not a feature-complete release.
The capability ledger retains every disabled/missing workstream and owner.
Windows/macOS enforcement, GPU execution, complete modalities, model quality,
portable distribution, robust stopping and release assurance remain required.

Schema migration must preserve recoverable original bytes. Refusing an unsupported
experimental schema is safer than silently rewriting it. Compatibility handling
must be completed before shipping required non-destructive project imports.

The finite lifecycle model covers explicit bounded transitions and assumptions.
Its authorization-negative mutant is an adequacy probe, not a full mutation
campaign. Kani harnesses without an executed solver remain unproved obligations.

## Evidence and invalidation

`scripts/check-architecture.mjs` records the production source digest and direct
dependency/source restrictions. Workspace tests, real subprocess integration,
GUI runs and lifecycle-model results belong in `PULSAR_PHASE_A_STATUS.md` and its
evidence artifacts. Changed code invalidates affected receipts until rerun.
No commit, formal result, human approval, real-media accuracy or physical
qualification is implied by acceptance of this ADR.
