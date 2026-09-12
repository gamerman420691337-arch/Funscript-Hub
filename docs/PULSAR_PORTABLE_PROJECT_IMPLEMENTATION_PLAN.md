# Portable-project implementation plan

Status: **IN PROGRESS. P1a foundation and P1b retained export/scoped download are implemented and locally exercised on Linux. P1b is committed as b4c038a. P1c clone import has nine repaired defects and a passing 494-test component campaign, including worker and large-file cases; the combined architecture gate is blocked by a confirmed scanner false positive, and external review remains unavailable. P1d complete user-facing/platform qualification remains unfinished.**

See [P1c checkpoint and blockers](PULSAR_PORTABLE_PROJECT_P1C_STATUS.md),
[ADR 0011](adr/0011-portable-project-clone-import.md),
[P1b status and evidence](PULSAR_PORTABLE_PROJECT_P1B_STATUS.md), and the historical
[P1a foundation](PULSAR_PORTABLE_PROJECT_P1A_STATUS.md).
This does not claim a complete portable export/import workflow or completion
of the Good phase.

This is the next bounded B6 work package, not a replacement for the software
specification, architecture decisions, or B1-B6 acceptance criteria. It does not
claim portable executable distributions, model-pack redistribution, or platform
qualification.

## 1. Next executable vertical slice

**B6-P1: export one consistent editable project, move the resulting file to a
fresh engine data root, import it as a new project, then inspect, edit, undo/redo,
and export its neutral motion without the original machine.**

Include all persisted revisions, the retained undo/redo branch, protected
regions, durable candidates, exact compact evidence and review state, source
identities, and source bytes. Treat interrupted/running work as historical
evidence only. Do not restore executable jobs, permissions, clients, or devices.

The first format is a versioned, uncompressed, length-framed single-file
container. It carries typed logical project records and content-addressed
objects, not a SQLite database and not arbitrary archive paths. This avoids a
general archive extractor and keeps the editable-project operation independent
of engine-wide disaster recovery.

Minimum successful demonstration: a two-axis project with a media source,
imported script, authored edit, protected interval, candidate merge, undo and
branched history exports at revision R; another engine imports it under a new
ProjectId; all promised state compares exactly through the recorded identity
mapping; subsequent authorized editing succeeds. A smoke test alone does not
close the failure/security gates below.

## 2. Observed implementation seams

Source was inspected read-only for this proposal. These are source facts, not
new qualification claims.

| Existing file / symbol | What is available | Implication |
| --- | --- | --- |
| `crates/pulsar-engine/Cargo.toml` | `rusqlite 0.32` with `bundled`. | No user database installation is needed. |
| `crates/pulsar-engine/src/authority.rs`: `Engine::open` | One file-backed database, WAL, synchronous FULL, engine mutex, durable request identities, per-project grants. | A package must select one project's rows, not copy the global engine database. |
| `authority.rs`: `project_snapshot`, `candidate_snapshot` | Public snapshots are compact motion descriptors plus limited metadata. | `GetSnapshot` alone is not a complete native project export. |
| `authority.rs`: `export` | Neutral export has authorization, revision/replay checks, non-clobber publication, pending/unknown-outcome receipts. | Reuse these invariants, not the in-memory funscript byte writer for large packages. |
| `crates/pulsar-engine/src/motion_state.rs` | `motion:<sha256>` SQL references, descriptor catalog, preserved `legacy_motion_bytes`. | Include the complete referenced motion set and preserve migration recovery bytes. |
| `crates/pulsar-engine/src/motion_artifacts.rs` | Durable immutable programs, verified opens, full digest and length, 64 MiB / 1M action / 1M gap limits per program. | Reuse this storage seam; do not decode every revision into memory simultaneously. |
| `crates/pulsar-engine/src/lineage.rs` | Explicit revision and restoration edges, history positions, review flags, unknown/conservative state, candidate contributions. | Equal motion bytes do not establish equal history or provenance. |
| `crates/pulsar-engine/src/transfers.rs` | Authenticated epoch-bound leases, separate bulk channel, authored-edit receipt and lineage tables. Current grants derive from upload/download direction; leases require a project. | Package transfers need explicit purpose-sensitive authority, binding, admission and limits. They cannot masquerade as motion download or authored edit. |
| `crates/pulsar-protocol/src/bulk.rs` | 256 KiB bounded chunks, absolute per-operation/transfer deadlines, exact offsets and authenticated handshakes. | Reuse chunk framing and connection poisoning rules. |
| `crates/pulsar-engine/src/artifacts.rs` | Immutable snapshots with Linux COW/copy fallback; digest checks; non-clobber exports. | Source paths are local locators, not portable identity. Large output needs streaming publication. |
| `crates/pulsar-engine/src/source_kinds.rs` | Engine-assigned media / generation-input / funscript purpose. | Preserve source kinds explicitly; do not infer them from archive filenames. |
| `crates/pulsar-protocol/src/worker.rs` | Worker manifests and dependency identities; generated output supplies artifact/receipt references and compact lineage identities. | Historical provenance must remain data, never an imported executable worker request. |
| `crates/pulsar-clients/src/session.rs`, `lib.rs`, `cli.rs` | Shared `EngineApi` / `SessionClient`; project CLI currently create/open. | One package helper serves CLI and desktop, without a second persistence implementation. |

Important gaps to close within P1:

1. A project-scoped artifact-closure resolver must identify receipt/evidence
   bytes referenced by candidates and authored edits. There is no demonstrated
   universal portable lineage catalog. Resolve only trusted engine-owned
   locations, hash every referenced object, and fail on unresolved required
   compact evidence. Do not search arbitrary paths embedded in imported data.
2. The existing transfer broker has motion-sized assumptions, including
   five-minute lease lifetime and a 4096-entry replay-chunk map. Multi-GiB media
   must not silently hit those limits or require unbounded chunk bookkeeping.
3. Source imports/pins can change independently of the motion revision.
   Revision R is necessary but not sufficient to identify the whole snapshot:
   capture the source/candidate/history set and an event cursor in the same
   database snapshot, and return its content digest.
4. Legacy and worker metadata may contain absolute paths. Preserve archival
   evidence bytes where required, but never activate those paths after import.

## 3. Chosen engineering defaults, not new user decisions

- Export current project state only, with an explicit expected revision.
  Historical-point-in-time project export is deferred because the source and
  candidate catalogs are not fully revisioned.
- Include all project media, generation-input and funscript snapshot bytes.
  If a required snapshot is evicted, report the missing content identity and
  fail self-contained export. Never silently follow the mutable original.
- Preserve identities of model/runtime dependencies, but do not bundle or
  install executable models/runtimes in this slice. Model-pack availability
  and exact regeneration are reported separately from editable-project
  completeness. Existing redistribution and qualification requirements remain.
- Import is clone-only: allocate a fresh local ProjectId and collision-safe
  local candidate identities. Never overwrite or merge an existing project.
  Preserve source content identities and numeric revision/history structure;
  retain a durable origin-to-local identity map.
- Preserve exact compact evidence bytes and historical actor labels as
  attributed archival data. They grant no local identity, trust, qualification,
  assistant permission, or execution authority.
- Exclude caches and heavy traces by default. Exclude authentication tokens,
  session/grant tables, device connections/approvals, live playback state,
  transfer credentials and executable queue state unconditionally.
- The bundle is explicitly unencrypted. The UI warns that it includes media,
  prompts, labels and private lineage. Encryption is a later format capability,
  not something the initial format pretends to provide.
- Unknown schema versions or unsupported required fields fail closed without
  changing the original package or the destination engine.
- Initial format ceilings are technical admission defaults: 16 MiB manifest,
  100,000 logical records/object descriptors, checked u64 total lengths, and a
  configurable package-byte ceiling proposed at 64 GiB. Per-program existing
  limits still apply. These ceilings require sizing tests and may be adjusted;
  rejection must be explicit, never truncation or omitted history.

No blocking product question was found for this slice. Defaults above remain
proposals until implementation review. This plan does not reopen the settled
one-engine, immutable-input, provenance, platform or original-media policies.

## 4. Data closure and identity contract

A versioned `PortableProjectManifest` records origin ProjectId, captured
revision, event cursor, schema/codec versions, completeness status, object
descriptors and the expected logical-record counts. Canonical ordering makes
the captured snapshot identity deterministic. Export-operation/request IDs and
destination paths belong in a separate local receipt, not the deterministic
manifest.

| Include | Required representation |
| --- | --- |
| Project head | Name, captured revision, current motion descriptor, protection, history cursor. |
| All persisted revisions | Numeric revision, actor attribution, operation/label, motion reference and protection, including abandoned branches. |
| Retained edit states and history lineage | Positions, revision associations, motion/protection, explicit restoration edges. |
| All durable candidates at capture | Base revision, motion, review state, committed revision and archival origin. Uncommitted proposals remain proposals. |
| Revision lineage | Exact parent/restored-from/candidate associations, contribution spans and unknown/conservative review state. |
| Authored-edit lineage | `edit_candidate_lineage` metadata and referenced receipts/input evidence needed to interpret authored versus inherited motion. |
| Generated provenance | Required receipt bytes, dependency identities, source/attempt associations and historical manifests as inert evidence. |
| Source catalog | Version, kind, label, content identity, availability and pin intent; restored paths are engine-generated. |
| Motion objects | Every distinct object referenced by project, revisions, edit states and candidates. |
| Migration recovery | Exact applicable `legacy_motion_bytes`; preserved opaque historical bytes remain separate from current canonical motion. |
| Receipts | Project-scoped neutral/adaptation/export history as inert receipt data, excluding bearer credentials and active request-replay authority. |

The imported operational graph may use new local IDs, but the archived graph and
raw evidence retain their exact original bytes/identities. Record all
translations explicitly. Do not rewrite an opaque receipt and keep its old
digest. Do not turn absent ancestry into inferred ancestry.

Generated job/attempt associations must remain inspectable through an archival
origin record rather than being erased or resurrected as entries eligible for
`ResumeJob`. The destination's event stream and request-ID namespaces are new;
the archived source cursor is historical metadata, not a reconnect cursor.

Distinguish three independent statuses: editable-project completeness, source
media availability, and exact generation-dependency availability. A complete
editable project is not a neural reproducibility or physical-safety claim.

## 5. Deep module and public interface

New engine module `project_packages.rs` owns capture, closure validation,
streaming container I/O, recoverable publication and import translation.
Private format and store helpers may live beneath it; callers should not
coordinate tables, file layout, graph repair or archive extraction themselves.

Proposed control operations, named provisionally:

| Operation | Contract |
| --- | --- |
| `StartProjectExport` | Existing project + expected revision + idempotent request; requires explicit `PackageProject` scope. Returns bounded package-operation metadata, never package bytes. |
| `ProjectPackageStatus` | Authenticated operation owner; reports capturing/streaming/validating/ready/failed/cancelled and typed failure. No paths or credentials in normal progress. |
| `BeginProjectPackageDownload` | Ready immutable export; creates a purpose-bound bulk lease and returns digest/length/captured identity. |
| `BeginProjectImport` | Paired creator supplies declared digest/length; engine reserves a fresh destination identity and bounded staging, without publishing a project. No client-chosen engine path. |
| `FinishProjectImport` | Exact complete upload; asynchronously validates closure, publishes objects, then atomically creates the new project and local owner grants. Returns operation metadata; status yields the resulting snapshot. |
| Existing transfer status/abandon | Reuse semantics only after adding an explicit package purpose and correct grant/binding checks. |

`PackageProject` is a separate project scope because ordinary neutral
`Export` must not start disclosing source media and private lineage. Existing
third-party grants do not gain it automatically. The authenticated creator of a
new project receives its owner capabilities through the ordinary owner policy.

Import uses the current paired-client ability to create a project, not an
implicit permission to read engine filesystem paths. Its provisional identity
is a reservation bound to the authenticated session, not an existing project
with ambient grants. The transfer descriptor must distinguish that binding
from an existing-project motion lease; do not weaken `requires_project` or
normal grant checks to make an absent project pass.

Client-facing helpers should remain small:
`export_project(project, expected_revision, destination)` and
`import_project(source)`, with progress/cancellation callbacks. The client
opens its explicitly selected file and uses the same authenticated bulk
protocol for CLI and desktop. It verifies the final package digest and
publishes an export file without clobbering an existing file.

A package-operation lifecycle is necessary for multi-GiB input. Do not hold a
control connection, main database mutex, GUI thread, or live-control resource
reservation while hashing/copying media. Do not force package operations into
the generation-only jobs schema using fake media sources or worker attempts.

## 6. Snapshot, publication and import ordering

### Export

1. Authenticate, require `PackageProject`, validate expected revision and
   request identity, and admit metadata/storage/resource budgets.
2. Under a documented lock order, establish an engine-owned read transaction,
   capture the head/source/candidate/history closure and event cursor, and
   acquire explicit artifact read holds against eviction.
3. Copy bounded metadata to private staging from that one snapshot. Finish the
   read transaction promptly; do not keep the engine's database mutex across
   media transfer. A dedicated read connection may be used, but its snapshot
   establishment and artifact holds must be coordinated with eviction.
4. Stream exact required objects from verified, no-follow regular-file handles.
   Validate digest and length during the stream. Recheck authority and absolute
   operation budget between bounded units; cancellation releases held resources.
5. Fsync the complete private bundle, publish immutable package identity, fsync
   its directory, then record the ready receipt. Only then admit downloads.
6. Bulk download rechecks purpose/grant/session/epoch on each chunk. Revocation
   prevents further disclosure but cannot retract bytes already delivered.
   Disconnect is not revocation; explicit retry/status semantics apply.

SQLite WAL read transactions provide stable snapshots on separate connections;
they do not make a later filesystem walk consistent with the database. Artifact
holds are therefore a distinct requirement.
[SQLite isolation documentation](https://www.sqlite.org/isolation.html)

### Import

1. Create an engine-generated private staging file and provisional destination
   identity. Admit declared total storage plus validation overhead before
   accepting bytes; stream through bounded chunks.
2. Reject unknown format/version, duplicate keys/IDs, excess records, trailing
   bytes, unexpected object counts, digest/length mismatches, missing required
   objects, invalid typed values, broken references, cycles where forbidden,
   inconsistent revision/history state, and over-budget input.
3. Container entries have typed roles, digests and lengths, not filenames.
   The importer chooses every local object path. Reject link/device/directory
   entry types, archive SQL, executable hooks and undeclared sections.
4. Preserve the exact original import file in a separately documented recovery
   object before any format migration. Use immutable copy/COW where available,
   never overwrite the sole supplied copy, and account for retained bytes.
5. Publish validated immutable motion/source/evidence objects and fsync them.
   Validate existing same-digest objects before reuse. SQL failure may leave
   complete unreferenced objects; it must not leave partial referenced objects.
6. In one durable transaction, create the new project, translated graph,
   imported-origin map, history/review data, receipt and only locally assigned
   owner grants. Recheck authorization/cancellation immediately before commit.
   Publish no operational worker/device/session state.
7. Commit is the authority linearization point. Replaying the same authenticated
   request returns the same imported project, including after restart. A new
   explicit import request may create a separate clone, not overwrite the first.

Import-format migrations and engine database migrations are different. Never
execute SQL from a package or directly attach its schema to the live store.
SQLite documents additional restrictions needed for untrusted SQL/databases;
the initial format avoids accepting either.
[SQLite untrusted-input guidance](https://www.sqlite.org/security.html)

An engine-wide recovery backup is a separate future operation. SQLite's backup
interface can produce a database snapshot but would also include other
projects and credentials here, and does not capture external motion/media
objects by itself.
[SQLite backup documentation](https://www.sqlite.org/backup.html)

## 7. Exact implementation file plan

| File | Scoped change |
| --- | --- |
| NEW `crates/pulsar-protocol/src/project_package.rs` | Manifest/record/descriptor types, format limits, operation state, provisional import binding; typed validation. |
| `crates/pulsar-protocol/src/lib.rs` | Register types, explicit commands/responses/scope/capabilities; command validation, debug redaction and version/compatibility treatment. |
| `crates/pulsar-protocol/src/motion.rs`, `bulk.rs` | Purpose-specific transfer binding/limits without relaxing motion limits or framing. |
| NEW `crates/pulsar-engine/src/project_packages.rs` | Deep package implementation and bounded private lifecycle. |
| `crates/pulsar-engine/src/authority.rs` | Registration, dispatch, scoped grants, reservation ownership, durable package/import receipts, and startup recovery. |
| `crates/pulsar-engine/src/transfers.rs` | Purpose-sensitive authority, provisional-import binding, bounded replay strategy, package-range limits and cancellation. |
| `crates/pulsar-engine/src/resources.rs` | Only if existing reservation primitives cannot express artifact read holds or aggregate package staging; no duplicated scheduler. |
| `crates/pulsar-engine/src/artifacts.rs` | Streaming non-clobber publication and shared verified file-handle helpers as needed; preserve original-media policy. |
| `crates/pulsar-engine/src/motion_state.rs`, `lineage.rs` | Narrow internal capture/import helpers so package code does not duplicate motion/reference/history semantics. |
| NEW `crates/pulsar-clients/src/project_package.rs` | Shared streaming import/export helper; client-selected file handling and progress. |
| `crates/pulsar-clients/src/session.rs`, `lib.rs` | Shared authenticated helper registration and response handling. |
| `crates/pulsar-clients/src/cli.rs` | Add `project export` / `project import`; preserve create/open and existing aliases. |
| `crates/pulsar-clients/src/desktop.rs` | Thin import/export actions and honest privacy/completeness/progress UI. Read its current UI seam before editing. |
| NEW `crates/pulsar-engine/tests/project_packages.rs` | Production-interface integration/fault tests. |
| NEW `crates/pulsar-protocol/tests/project_package.rs` | Manifest/framing/validation KAT and hostile-format tests. |
| NEW `crates/pulsar-clients/tests/project_package.rs` | Shared helper, CLI/desktop request parity and non-clobber file tests. |

Read the exact newly affected files once before implementing. The source
investigation did not inspect `resources.rs` or `desktop.rs`; their entries
are conditional integration seams, not assertions that a suitable helper
already exists. No broad engine rewrite is required by this package.

## 8. Execution milestones within P1

1. **P1a: contract and closure KAT, D2.** Freeze typed record/identity mapping,
   container bytes and privacy exclusions. Build a real source-backed project
   fixture and enumerate its complete SQL/object closure. The KAT must fail
   when a revision edge, review unknown bit, evidence receipt or original
   recovery cell is dropped.
2. **P1b: consistent export, D3.** Add scoped operation lifecycle, capture and
   artifact holds, streaming immutable bundle publication and client download.
   Concurrent edit/source/candidate changes must yield one captured set, not a
   mixture. Do not call this portable-project completion before import works.
3. **P1c: clone import and recoverable publication, D3/D4.** Add bounded decoder,
   inert provenance, identity mapping, exact recovery bytes and atomic new
   project publication. Exercise restart and ambiguous acknowledgement.
4. **P1d: shared product surface and qualification, D3.** CLI and desktop use
   the same helper. Move a real package to an unrelated data root, edit it and
   round-trip it again. Run the complete gates below.

Do not start compression, encrypted containers, external relinking, historical
catalog reconstruction, cloud backup, model installation or portable executable
packaging in this slice. Those remain later explicitly tracked B6/M6 work.

## 9. Required gates

| Gate | Evidence required before P1 is complete |
| --- | --- |
| Exact closure KAT | Compare all revisions, retained history, candidates, protection, source kinds, conservative/unknown review state and compact evidence, not only final action values. |
| Large motion/media | Existing 54k-stroke and 324k-six-axis programs survive; a streamed fixture larger than 1 GiB proves the old replay-map assumption is gone. Record peak memory and bounded chunk size. |
| Concurrent snapshot | Inject edits, source import, pin/eviction and candidate completion around capture. Assert a coherent captured digest/cursor/set and no source disappearance while held. |
| Authority | Neutral Export or Read alone cannot package; another session/project cannot use a lease; changed credentials, scope revocation, wrong purpose/epoch and cross-request reuse fail closed. |
| Imported trust | Fake qualified flags, actor IDs, job manifests, model paths, grant tables, scripts and device state never establish local authority or execute. |
| Parser/graph attacks | Truncation at every framing edge, overflowing lengths, duplicate objects/IDs, invalid time/axis values, malformed graph edges, missing evidence and undeclared trailing data reject atomically. |
| Filesystem attacks | Symlinks, hardlink tricks, nonregular objects, path traversal strings, replaced parent directories and preexisting destination files never escape the engine/client-owned destination seam. |
| Failure ordering | Crash/fault injection during capture, streaming, object fsync, publication, SQL commit and response. Result is old state, complete new state or an explicitly recoverable pending outcome; never partial visible history. |
| Replay/restart | Same request after lost acknowledgement/restart returns the same capture/import result; a changed fingerprint conflicts; interrupted leases cannot resurrect under a new epoch. |
| Privacy | Package contains only selected-project closure. Seed other-project secrets, tokens and device grants and assert none are present; ordinary funscript remains stripped. |
| Original/recovery policy | Originals and sole package copies keep their bytes through failures; legacy recovery cells remain exact; absent media never gets silently replaced by changed bytes. |
| Client parity | Same requests/outcomes through shared CLI/desktop helper; cancel/disconnect distinctions, non-clobber file publication and visible missing dependencies. |
| Portability claim | Filesystem-safe relocation on the currently enabled target first. Windows/macOS path, handle, fsync and rename behavior require their own later platform gates; Linux passing is not cross-platform qualification. |

Use KAT, property/metamorphic cases, fuzz input bounds, targeted mutation of
digest/lineage/authority guards and fault injection through the real module
interface. Broader release campaigns remain Phase C. This document records no
new test result and authorizes no physical device action.

## 10. Completion wording

P1 is complete only when export/import and all promised project state survive
the production-interface gates. Report exact current-source results and any
unsupported imported feature. Do not label the Good phase complete or say its
completion keyword based on this proposal or this package alone.
