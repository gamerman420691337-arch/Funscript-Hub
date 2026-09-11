# ADR 0010: Retained portable-project export operations

Status: Accepted engineering design for B6-P1b. Implementation and qualification status are recorded separately. This ADR does not complete portable import or the Good phase.

## Context and authority

ADR 0009 defines the archival format and coherent logical capture. A valid manifest alone does not retain external media, reserve resources, authorize copying private content, or make destination publication durable.

The user chose offline self-contained editable projects, one per-user engine, explicit authority, and durable provenance boundaries. The lifecycle/API details and numeric caps below are selected engineering defaults, not newly inferred product requirements. The software specification and architecture decisions remain authoritative.

## Separate operations, scopes, and approval

Exports have their own durable operation identity, request tombstone, capture receipt, progress, and terminal state. They are not generation jobs and do not gain job restart/fallback privileges.

PackageProject authorizes packaging a particular project. Read, ExportMotion, ManageGrants, a caller-chosen client name, and knowledge of the pairing token do not independently grant it. Existing projects require an explicit host-local allow-packaging command. That command requires both the authenticated caller's existing ManageGrants and possession of a distinct private package-owner.token capability. It grants PackageProject only to the calling session. Ordinary scoped Grant can then delegate that scope.

The approval proof is redacted in Debug and excluded from durable request payloads, receipts, and logs. Automatic startup/export approval, pairing-token reuse, arbitrary-target privileged grants, and lost-owner recovery are outside this seam. Possession establishes OS-user authority, not cryptographic evidence that a human clicked. Unrestricted same-UID processes remain outside this boundary. Bounded assistant and plugin interfaces must not inherit this capability.

Export and approval disclose that packages are unencrypted and can contain original media, prompts, private lineage, and edit history.

## Snapshot and lifetime boundaries

Capture checks the expected revision once, together with the complete project-scoped catalog and event cursor. Later edits do not invalidate that pinned snapshot. Publication instead rechecks the operation's current authorization and cancellation fence.

Use lock order artifact_gate, then database, then brief retention-registry access. Hold no registry lock across SQL or I/O; guard destruction must not acquire the gate or database. Retention is metadata-based rather than one open descriptor per manifest entry. A conservative required-object hold is established before releasing the capture gate, then refined after parsing.

Source holds last until durable self-contained Ready publication, or until a terminated worker has actually stopped using them. A cancellation acknowledgement does not release a running worker's memory/storage reservation. Ready bundles and active downloads use separate holds. Ordinary source-cache eviction must not remove held objects.

SQLite snapshots stabilize SQL observations but do not retain external artifact files. Long-lived readers also affect WAL checkpointing. These facts motivate a short explicit capture transaction plus a separate artifact lifetime protocol. [SQLite WAL documentation](https://www.sqlite.org/wal.html)

## Admission, materialization, and publication

Use bounded fixed-buffer streaming into private engine staging. Content-addressed published paths contain only validated digests, never package-provided paths. Unix filesystem effects use anchored directory descriptors and no-follow operations.

The initial operation admission defaults are two active operations, a one-hour absolute operation deadline, and 10,000 durable operation rows. Download-begin request tombstones have a separate 100,000-row admission cap. The package-store ceiling uses max_snapshot_bytes, initially 32 GiB, as a separately accounted package-store budget. The format's 64 GiB ceiling remains distinct. Actual physical disk admission uses the shared ResourcePool so concurrent imports, workers, and package staging cannot independently spend the same observed free bytes. Ready artifacts, retained orphans, and output reservations count toward the package-store ceiling. Pending staging bytes also remain charged if cleanup fails. Live staging can therefore be conservatively counted alongside its reservation; early admission rejection is preferable to uncharged leftovers.

Synchronize complete verified bytes, publish without overwriting existing destinations, synchronize the containing directory, and only then commit/acknowledge Ready. A failed post-publication synchronization can leave durable effects uncertain; report UnknownOutcome rather than claiming that nothing happened. Keep immutable artifact identity and final length in the receipt.

A no-clobber helper alone is not a portable transaction: it may leave links and does not automatically synchronize data or the directory. File synchronization and namespace publication are separate obligations. [tempfile publication documentation](https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html#method.persist_noclobber), [Rust file synchronization](https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_all)

Unsupported safe-publication platforms must fail explicitly. A Linux implementation or generated-byte fixture does not qualify Windows or macOS.

## Download authority and reconnect

Package downloads use a separate tagged handshake on the bulk socket. They never broaden ordinary motion-transfer scopes, framing, or limits. Bind each lease to the initiating session, project, operation, artifact, epoch, and permitted range; require current PackageProject authority. Use 256 KiB chunks, a 30-minute absolute lease deadline, eight global leases and two per operation, one active connection per lease, and O(1) sequential cursor plus one exact previous-chunk replay. Fresh subrange leases allow transfers longer than a single lease lifetime.

Advance a cursor only after a complete successful write. Partial transport failure does not create a successful acknowledgement. Recheck authority after blocking reads and before admission of new bytes. Revocation increments a durable authorization generation in the same transaction. Leases, verifier results, and pending Begin requests bind that generation, including a final transaction recheck after unlocked I/O. Delayed advisory callbacks affect only older-generation work, never fresh regranted work. Revocation cannot retract bytes already admitted to transport. No database lock may span an unbounded socket write.

Disconnection does not cancel an export. Explicit cancel and revocation fence solely authorized work. Reusing request identity with changed intent rejects; replay cannot resurrect cancelled, interrupted, or released work.

## Restart verification

A historical Ready receipt survives restart; live leases and current-epoch verification do not. Never rehash all retained packages at startup or hash a large file inside a control handler.

The first post-restart BeginDownload durably binds request identity and range, queues bounded asynchronous verification, and promptly returns PackageDownloadPending with progress. Durable export status remains Ready. Exact retries remain Pending until verification succeeds, then obtain the same reserved lease outcome. Verification checks authorization and cancellation, obeys admission/resource limits, and holds the bundle outside database/control locks. Missing or corrupt bytes produce an explicit unavailable/failure outcome and no lease. A draining old verifier retains its active slot until exit; a replacement cannot overwrite that slot. Worker creation is fallible: spawn failure must produce a defined durable outcome and release admission/holds rather than panic after admission.

Interrupted unfinished export operations are not automatically restarted. A new operation requires a new deliberate request.

## Explicit release and client publication

Release requires the owning session and PackageProject. It preserves compact receipt/request tombstones, invalidates leases, and removes bytes only when no Ready reference or active read/write hold remains, under the artifact gate. Crashes may leave retained orphans; do not delete unidentified files or silently evict request history.

The shared client helper drives start/status/cancel/download and publishes a caller-selected destination only after full receipt length/hash validation and durable non-clobber publication. Export never reads the owner capability or autoapproves. Allow-packaging is the separate explicit host operation. Caller-supplied request IDs and bounded same-ID transport retries support lost acknowledgements without issuing duplicate intent. Network/control operations share an absolute deadline. Native filesystem writes and synchronization are not wall-clock interruptible by this helper. Process death can leave a private staging directory; no verified final destination is implied by those staging bytes. No automatic release follows download; the explicit release command clearly discards the engine copy. A valid export hash is not clone-import semantic qualification.

CLI is a thin shared-client tracer. GUI parity, fresh-project clone import, relocated editing, all-platform publication, and full user-facing recovery remain later P1 work.

## Assurance and limitations

The finite PulsarPackageExport model separates capture, streaming, byte publication, namespace durability, Ready acknowledgement, epoch verification, chunk admission, write completion, and release. Negative variants remove individual safeguards. These are bounded design checks, not Rust, SQL, cryptographic, or OS proofs. Real-interface correspondence tests must exercise the concrete implementation independently. [TLC documentation](https://docs.tlapl.us/using:tlc:start)

The model does not establish closure completeness, multiple-session isolation, aggregate scheduling fairness, multi-operation quotas, digest correctness, same-UID adversary confinement, arbitrary interrupted-write behavior, or physical safety. A large sparse generated-media test is a bounded-memory regression fixture, not neural accuracy or reference-hardware performance qualification. Kani, package fuzzing/mutation campaigns, import crash recovery, and platform qualification remain separately reported gates.

The 10,000-row admission cap prevents unbounded growth but is not a long-term compaction design. Future tombstone compaction must preserve replay guarantees rather than deleting history opportunistically.
