# ADR 0008: Immutable motion artifacts and scoped transfer leases

Date: 2026-09-11
Status: Implemented; local contract qualification recorded in the linked checkpoint.
Scope: Good-phase large-motion transport work package, not completion of B1-B6.
Supersedes: ADR 0007's unimplemented transport proposal, for this work package only.
Authority: The software specification and ARCH-001-040 remain authoritative.

## Context

Inline JSON motion in a 1 MiB control message and the former 512 KiB publication limit could not carry full-detail reference-length motion. Increasing only one limit would leave downstream query/edit paths broken. Silent thinning would violate neutral motion and timing requirements. The replacement must preserve the engine's existing commit authority, permission and revision checks, protected regions, review state, and history.

## Decision and ownership

Use engine-owned immutable JSON motion objects, compact descriptors on wire v2, and a separately authenticated bulk channel. Keep four product libraries and the existing worker publication schema.

| Library | Responsibility |
| --- | --- |
| pulsar-core | Checked values-only edit codec and pure interpolation-aware reconciliation. No I/O or authority. |
| pulsar-protocol | Versioned descriptors, leases, locator identities, checked framing, typed failures, and local connection deadlines. |
| pulsar-engine | Immutable object storage, SQL references and legacy backups, scope checks, reservations, lease lifecycle, candidate publication, and sole commit authority. |
| pulsar-clients | Shared authenticated download/upload, descriptor and digest validation, bounded hydration cache, and GUI/CLI adaptation. |

Project and candidate replies contain descriptors, not whole programs. A descriptor binds engine artifact identity, SHA-256, exact byte length, codec, axis counts, and project/revision or candidate/base revision. Descriptor possession is not authorization. Client cache hits require the outer project/revision/candidate binding to match before lookup.

The control channel remains length-framed JSON with a 1 MiB cap. Bulk headers are limited to 16 KiB and raw chunks to 256 KiB. Wire-v1 requests receive a typed upgrade rejection before command deserialization or effects. Documented CLI syntax remains; incompatible running engines are not killed or replaced.

## API surface

| Command | Contract |
| --- | --- |
| BeginEditUpload | Requires Edit, project and expected revision. Reserves resources and captures the immutable base. |
| FinishEditUpload | Validates complete upload and publishes an engine-owned candidate; never commits. |
| TransferStatus | Authenticated owner query for prefix, expiry, direction and epoch. |
| AbandonTransfer | Authenticated cancellation with durable request identity and replay outcome. |
| BeginMotionDownload | Requires Read; resolves a project revision or candidate and checks requested byte range. |
| CommitCandidate | Remains the only candidate commit path, with normal scope, revision, protection and idempotency checks. |

Successful mutating requests share one durable identity namespace. Different payloads cannot reuse a request ID across normal commands and transfer commands. A successful fresh-ID alias of an already completed finalization is journaled too. Disconnection alone is not revocation. Restart invalidates unfinished leases; durable candidates, commit results, and journaled finalization outcomes survive.

## Upload semantics and provenance

The edit-values codec accepts axes, integer project times, positions and explicit gaps only. Unknown fields, including caller-supplied evidence, provenance, model qualification and worker authority, reject. The engine assigns authored provenance and candidate identity.

Unchanged knots can retain their original evidence while changed interpolation spans are authored. The pure reconciliation kernel compares interpolation influence, not merely changed timestamps. Exact authored and inherited ranges remain in an immutable receipt linked to the base artifact, input digest, resulting object, actor and kernel. The uploaded values digest is not the resulting motion-object digest.

Review display uses conservative per-axis envelopes, at most six, rather than dropping exact receipt lineage. Prior active reviews are clipped to the resulting program's supported intervals. Removed support ceases to be active display support, not human resolution; the base revision/history retains original state. Explicit human review resolution remains separate unfinished work.

The editor preserves explicit gaps and does not draw continuous motion through them. Nonmonotonic in-flight drag drafts do not render a misleading continuous curve. Client evidence lookup is bounded by binary search rather than a full scan for every point.

Standard funscript exports strip internal provenance and retain local export receipts. Internal nanosecond timing remains exact. Existing funscript millisecond quantization uses checked nearest-half-up rounding with at most 0.5 ms error; colliding output timestamps reject rather than silently dropping actions.

## Persistence, resources and lifecycle

Objects use SHA-derived names and identities, private immutable regular files, anchored directory access, no-follow checks, no-replace publication, and file/directory fsync before transactional references become visible. Readback verifies length/digest and semantic bounds. Legacy inline JSON is backed up byte-for-byte during migration.

Initial technical admission defaults are 64 MiB per program, one million aggregate actions, one million aggregate gaps and six unique axes. Upload admission reserves 1 GiB validation RAM plus upload bytes, one 64 MiB program allowance and two 128 MiB receipt allowances through the aggregate pool. These are conservative implementation bounds, not measured capacity guarantees.

At most eight active leases and sixteen bulk handlers are admitted; handler capacity is checked before spawn/handshake. Each lease has at most 4,096 replay records and an absolute five-minute lifetime. Per-frame timeout is ten seconds and client operations also intersect the remaining absolute lease deadline. Client cache is bounded by 128 MiB and eight entries.

A nominal 250 ms weak-owner maintenance sweep releases expired idle leases. This cadence is not a hard real-time scheduling or reclamation bound. Startup removes regular staging remnants from invalidated prior epochs.

Expensive upload hashing, parsing, reconciliation and object publication occur outside the authority database lock. Final publication reacquires an IMMEDIATE transaction and rechecks authentication, Edit scope, owner, project, engine epoch, expiry/cancellation and pinned base. Fallible setup must finish before marking a lease as finalizing. Existing commit/merge/protected-history object reads can still occur inside authority transactions; control-latency and memory qualification remain open.

## Evidence and rejected alternatives

See the current large-motion checkpoint and evidence manifest for exact tested source/binary identities, process tests, GUI captures, review dispositions and remaining gates.

Rejected: raising inline caps alone; lossy decimation to fit transport; trusting uploaded observed evidence; a second edit commit authority; authorizing cache hits by digest alone; treating a successful network write or an abstract model as proof of durable application/physical behavior.

The finite transfer lifecycle model covers authorization, engine epochs, complete verified publication, revisions and reservation state. Four guard-removal mutants must violate their named invariants. The model assumes atomic durable publication and digest/resource predicates; it does not prove Rust, OS calls, fsync, crypto, neural accuracy or physical stopping.

Linux nonblocking connect can return EAGAIN when local socket backlog is exhausted. The shared connector verifies both socket error and connected peer, retries boundedly under one deadline, and never treats an unconnected descriptor as success. Primary reference: [Linux connect(2)](https://man7.org/linux/man-pages/man2/connect.2.html).

## Startup correction

The launcher must not call blocking UnixStream::connect before the bounded protocol client. A saturated AF_UNIX listener was reproduced with an actual kernel EAGAIN condition and an actual CLI subprocess that exceeded its parent timeout.

Use the shared bounded connector for readiness probes. One absolute ten-second startup budget includes the initial probe; each connect gets at most 250 ms and never more than the remaining budget. Only absent/refused endpoints admit startup. Busy, inaccessible and unexpected errors do not authorize replacing an existing engine.

An engine acquires its instance lock before loading durable state and publishing listeners. A competing launch can therefore observe a held lock without a ready socket. The read-only instance-lock probe opens anchored, private, same-user existing objects without following symlinks, creating files or unlinking anything. Only actual lock contention counts as held. This is an advisory initialization hint, not readiness or authority. After its single contender exits, the launcher can wait for the known initializing owner within the remaining budget; it never loops spawning replacements.

The initial saturation fixture incorrectly equated a short scheduling timeout with queue saturation and was rejected. A later 4,097-connection fixture proved the bug but failed under a 1,024-file-descriptor limit; use a test-only small listen backlog instead. These fixture corrections do not establish production regressions by themselves.

## Open qualification and consequences

This implementation removes the inline transport blocker without reducing product scope. JSON parsing, validation and dense UI rendering still need reference-hardware cost measurements. Same-UID hostile direct filesystem mutation is outside this slice's isolation claims. Windows/macOS and vendor backends are not qualified by these Linux tests. Kani has not run.

Real corpus and human review work, licensed bundled inference/assistant models, semantic modalities and observed six-axis reconstruction, portable project packaging, explicit review resolution, plugin/assistant execution, and real Handy/Handy 2 live/stop/reconnect qualification remain open. No Preview, Stable, whole-Good completion, flawless-program claim or physical actuation is implied.
