# ADR 0011: Fresh-project clone import and inert origin archives

Status: Accepted P1c design; the full local import gate passes. External review and broader qualification remain unfinished.

## Authority and scope

Clone import creates a new project. It does not overwrite or merge an existing project. Its admission uses exactly the existing authenticated CreateProject authority: an authenticated session with an existing actor. This is a composition of creating a fresh project and importing caller-supplied data into that project, not permission to read another project's objects.

Every container byte must be uploaded and verified, even when an identical digest already exists in local storage. There is no digest-only fetch/adoption shortcut. The new project's owner grants appear only in the final transaction. No package-provided paths, SQL, executable jobs, model installation, sessions, grants, device acceptance, or archived actors become authority.

## Immutable archive versus active identity

The engine allocates the fresh project identity and fresh candidate identities. Project-local revision numbers, retained history and restoration relationships, protection, source kinds and exact content, candidates, review state, and recovery data remain semantically preserved. Import does not invent an extra editing revision merely to record importer attribution.

A pure engine-local planner consumes validated manifests, resolved origin manifests, and explicit newly assigned identities. It returns typed translated rows, required object identities, and origin mappings. It performs no SQL, filesystem access, randomness, or authorization.

Original worker requests, receipts, and authored evidence embed original identities and sometimes paths. They remain exact inert bytes under their original hashes. They are not rewritten or inserted into operational jobs/attempts. Current native-origin validation remains strict; imported provenance uses a separate typed archive projection.

Manifest version 2 adds typed imported origins: exact original canonical manifest identity, original container identity, original object catalog, and explicit local-to-origin mappings. Version 1 serialization, known-answer bytes, and native validators remain unchanged. A mixed project containing imported history and later local edits must re-export and clone again without pretending old receipts refer to new local identities.

Required ancestral payloads remain present and deduplicated by digest. Do not recursively embed complete original containers in exports, which would multiply media bytes with each clone. The exact uploaded container is retained locally for recovery; its canonical manifest and ordered object payloads establish the portable origin identity.

Initial engineering bounds are 64 origin manifests/depth, 256 MiB aggregate encoded origin-manifest bytes, and 500,000 aggregate origin logical/mapping entries, in addition to existing per-manifest bounds. Exceeding a bound rejects the operation; it never truncates lineage.

## Byte and publication lifecycle

Use distinct opaque capabilities for provisional upload, sealed container, fully verified/staged bytes, and durable published objects. Progress labels are not substitutes for these checked states.

Upload uses a separate authenticated package-import channel, bounded chunks and replay state, exact operation/session/epoch binding, and an absolute deadline. Start and Seal identities remain recoverable after lost responses. Disconnection is not cancellation.

Strict decoding validates canonical framing, graph references, lengths, hashes, object roles, and exact EOF before admission. Motion must already be exact canonical bytes with matching descriptors before publication; calling a normalizing publisher and comparing its result afterward is not validation.

Retain the original complete container before migration. Publish verified motion, sources, and inert evidence into engine-owned content-addressed namespaces using anchored filesystem operations, checked synchronization, and no-clobber semantics. Hold required objects through the final transaction. Failures leave charged recoverable artifacts; cleanup never blindly removes shared or preexisting digest objects.

The visibility point is one durable transaction containing the complete fresh project, revisions/history/candidates, inert origin mappings, fresh local owner grants, and request result. Before it, no partial active project is visible. After it, restart and exact replay return the same project rather than importing again. Ambiguous commit/publication outcomes are not reported as guaranteed no-effect failures.

SQLite's atomic-commit protocol has explicit operating-system and storage assumptions; it does not atomically commit unrelated media files. Pulsar therefore orders verified object durability before the project transaction. The rollback-journal description is not a claim about the engine's WAL implementation. [SQLite atomic commit](https://www.sqlite.org/atomiccommit.html)

Keep parsing, hashing, copying, and filesystem enumeration outside the database transaction. The final transaction binds all active state and the durable outcome together. [SQLite transactions](https://www.sqlite.org/lang_transaction.html)

## Required evidence

- Genuine multi-revision export imported into a different actor's fresh project, with source project still present and CAS hits exercised.
- Original receipt bytes unchanged; original actors and jobs cannot authorize or execute anything locally.
- Imported undo/redo, protected editing, candidate rebase/commit, local editing and re-export, and second clone remain coherent.
- Invalid inner bytes reject even with a valid outer upload hash and an existing valid CAS object.
- Exact request replay, changed-intent rejection, cancellation, interrupted precommit work, and lost postcommit acknowledgement.
- Deterministic failure points before/after object publication and SQL visibility; no partial active project and no deletion of preexisting shared objects.
- Separate generated large-data streaming evidence; no inference from a small codec roundtrip.

Finite lifecycle checks, implementation-correspondence tests, and process crash tests remain separate. They do not prove neural accuracy, arbitrary filesystem failure behavior, OS/runtime correctness, or physical safety. GUI/platform qualification remains P1d. P1b commit b4c038a's required external review returned Transport closed, not approval.

## Upload connection contention and bounded recovery

An exactly bound, authenticated upload handshake encountering an occupied
connection guard returns PackageUploadConnectionBusy with retryable=true.
This is not a generic ResourceExhausted retry policy. Wrong authority,
generation, content binding, and nonretryable errors remain terminal.

The shared helper preserves operation, lease, generation, accepted prefix,
buffered replay bytes, and the original absolute transfer deadline. Selected
engineering defaults are at most 20 retries, 50 ms backoff, and a two-second
contention budget intersected with the original deadline. Short connection
and handshake deadlines are scoped to admission; they do not replace the
successful transfer's original lifetime or later per-chunk timeout.

Wire version and frame structure are unchanged. The error code is additive:
older strict clients fail safely on the unknown code and do not gain automatic
recovery without updating.

A deterministic actual-process test must observe the real engine contention
reply before releasing the held original connection. The private test proxy
forwards exact header/payload bytes and uses a barrier rather than sleep-based
assumptions. The old opaque handshake failure remains unattributed.

## Historical acceptance boundary at c9f4951

Nine confirmed import defects have regression-backed repairs. A final-source
component campaign passes 494 Rust tests and export/import lifecycle models.
The combined qualification entry point still fails because the existing
unanchored ort:: scanner matches import-module names. Its correction is pending
approval; the scanner is not weakened or bypassed in this checkpoint.
External review remains unavailable. Commit/push is not phase or release
acceptance. See PULSAR_PORTABLE_PROJECT_P1C_STATUS.md and p1c-final.json.

## Scanner closure and current acceptance boundary

The user approved correcting the source scanner rather than weakening the
native-runtime boundary. Matching now requires whole Unicode Rust identifiers,
supports raw identifiers and path-separator whitespace, and retains exact
dispatcher-name checks. One pure helper is shared by the scanner and its
36 regression cases. Cargo dependency checks are unchanged.

Independent data-URL mutation QA killed the old unanchored expression and
allow-all/reject-all substitutions through assertions. The guard remains
lexical, not a Rust parser or transitive isolation proof.

The complete import qualification entry point now passes: 104 architecture
checks, 517 Rust test executions, 36 scanner tests, lifecycle models, actual
worker evidence, and large streaming fixtures. Prior failed-gate artifacts
remain historical. External review is still unavailable, P1d is unfinished,
and no neural, physical, cross-platform, release, or Good-phase completion
is implied. See PULSAR_PORTABLE_PROJECT_P1C_STATUS.md and the new
p1c-scanner-closure.json evidence.
