# ADR 0009: Portable project contracts and capture boundaries

Status: Accepted for the internal P1a foundation; the user-facing package lifecycle remains unimplemented.

## Context

A neutral funscript export is intentionally smaller than an editable project. A portable project must preserve historical edits, abandoned branches, protection, candidates, sources, and compact evidence without copying engine-wide credentials or executable authority.

The product decision is an offline, self-contained editable project export and a clone import with fresh local identity. The implementation sequence and initial format limits are technical defaults, not new user requirements. The software specification and architecture decisions retain authority.

## Decision

Use a versioned, uncompressed, length-framed file containing typed logical records and content-addressed objects. Do not export the engine SQLite database or accept package-provided SQL, schema, scripts, archive paths, or executable installation hooks.

Capture the project head at an explicit expected revision and capture its source, candidate, protection, history, and evidence catalog in the same SQL snapshot. Revision equality alone is insufficient because several catalogs can change without a motion revision. Historical ancestry and restoration edges are explicit records; equal motion bytes are not ancestry evidence.

A capture plan is not a ready bundle. P1a describes a consistent logical closure and validates its referenced records. P1b must establish artifact retention, bounded admission, cancellation and revocation, streamed materialization, verification, and durable publication before any operation can claim a self-contained export. Do not hold the authority database mutex while streaming or hashing bulk objects.

Retain raw evidence bytes under their original digest. Parsed summaries, reserialized JSON, or reconstructed receipts cannot replace missing originals under an old identity. Resolve required evidence through explicit project-scoped references, never arbitrary filesystem searches. Missing required bytes must produce an explicit failure, not a silently weakened export.

Resolve references according to their declared schema and role. A configuration checksum is not automatically a separately stored artifact. A worker receipt can name the original candidate bytes even when the motion store has a different canonical serialization; preserve the original attempt object under the receipt's digest.

Keep filesystem resolver locations engine-private. Package object entries identify role, content digest, and checked length, not extraction paths. Generic Read/Write bounds do not enforce wall-clock deadlines; the eventual operation owner must supply deadline and cancellation control. Decoder callbacks may write only provisional staging data; no success or active project publication precedes full framing, graph, digest, length, and final end-of-stream validation.

Origin actor labels, original project identifiers, dependency manifests, and historical job/attempt evidence are archival claims. Future import needs an explicit origin-to-local mapping and fresh local authority. It must never turn archived jobs into runnable queue state or import grants, tokens, sessions, device acceptance, live playback, or credentials.

Keep edit completeness, media availability, and exact-generation dependency availability separate. P1 does not bundle or install executable runtime/model packs. Initial packages are unencrypted and require a clear media/prompt/lineage disclosure warning before eventual user-facing export.

## Selected initial engineering limits

The typed manifest is capped at 16 MiB and 100,000 combined logical/object entries. The default file budget is 64 GiB with checked arithmetic. Private SQL capture accounts for at most 256 MiB of raw metadata and retains a 1 GiB ResourcePool memory reservation. These are selected implementation limits, not measured peak-memory or throughput results. Existing motion-object limits remain unchanged.

Exact legacy ownership must be applied before accounting, and worker primary source identities must agree with the captured catalog. Executable dependency identities do not provide an exemption for source bytes. Candidate/revision/job indexes avoid repeated full graph scans without changing canonical record order.

## Rejected shortcuts

- A global SQLite backup includes unrelated projects and engine authority and omits external objects.
- Serializing only the current public project snapshot loses native history and evidence.
- Looking at the same revision before and after collection does not make non-revision catalogs consistent.
- Reading a changed original media path does not recover the retained source snapshot.
- A successful encoder/decoder round trip alone is not an independent known-answer test.
- A decoder that emits object bytes has not admitted those bytes as a project.

## Research basis

SQLite documents stable WAL read transactions across connections, but not independent isolation for operations on the same connection. This supports one explicit capture transaction and a defined lock boundary; it does not establish retention of external objects. [SQLite isolation](https://www.sqlite.org/isolation.html)

SQLite separately discusses defenses for untrusted SQL and database files. The choice to transport typed records rather than a live database is a Pulsar attack-surface decision, not a claim that SQLite is unsafe. [SQLite security guidance](https://www.sqlite.org/security.html)

Rust readers may return short reads or interruptions. A bounded reader limits consumption but does not prove a full frame, exact source length, digest correctness, or absence of trailing data. The codec therefore needs explicit structural checks and streaming known-answer/adversarial cases. [Rust Read documentation](https://doc.rust-lang.org/std/io/trait.Read.html)

## Qualification boundary

P1a evidence must distinguish typed-contract validation, SQL closure capture, stream framing tests, and real durable engine fixtures. None alone proves relocation, import atomicity, disk-full recovery, multi-GiB peak memory, nonblocking control, cross-platform durability, or user-facing export/import parity.

P1b covers consistent materialization and scoped export. P1c covers clone import and durable publication/recovery. P1d covers shared CLI/GUI behavior and relocated editable-project qualification. The Good phase remains incomplete.
