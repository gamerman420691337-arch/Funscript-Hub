# Pulsar local protocol v1

This document describes the wire contract implemented in this crate. A declared
command is not evidence that an engine build supports it: unsupported operations
must return `unsupported`, never a successful placeholder. Capability discovery
reports implementation availability separately from qualification.

## Dependency and authority boundary

`pulsar-protocol -> pulsar-core`; neither crate depends on clients or engine.
The protocol owns bounded DTOs and transport framing, not effects or authority.
GUI, CLI and explicitly paired third-party clients use the same commands.
No command accepts an actor identity, arbitrary shell/SQL, a raw pointer, or an
ambient device handle. An authenticated engine session determines the actor.

## Connection and framing

The engine creates an owner-only Unix socket directory/socket or a Windows named
pipe with a current-user DACL. The client never removes or replaces the endpoint.
`interprocess` supplies the cross-platform byte stream; endpoint security remains
an engine obligation. Unix/macOS/Windows execution qualification is separate from
the existence of conditional platform support.

Each control frame is a four-byte big-endian unsigned byte length, followed by
exactly that many UTF-8 JSON bytes. Allowed lengths are 1 through 1,048,576.
Headers are checked before allocation. Invalid UTF-8, truncated frames, unknown
fields/operations and trailing JSON are errors. The receiver closes a malformed
connection rather than guessing where the next frame begins.

`LocalClient` applies one wall-clock deadline across all partial reads and writes
for a call. A peer cannot extend it indefinitely with a byte-at-a-time response.
Generic `read_message`/`write_message` require their connection owner to supply an
equivalent deadline. Frames, tensor payloads and model bytes never travel inside
control JSON. A separate artifact channel has independently bounded chunks no
larger than 262,144 bytes and a validated, authorized total-length descriptor.

## Requests, responses and revisions

`Request` fields are `version`, `request_id`, `session`, `project`,
`auth_token`, `expected_revision`, and `command`. `session` is a public identity;
the separate secret `auth_token` authenticates it. Pairing returns both. Session
IDs in grant/revocation events do not authenticate requests. Owner credentials
must not be reused as third-party grants. Diagnostic formatting redacts tokens
and command arguments; raw serialized requests and pairing responses must never
be logged. Commands use the discriminator `operation`
and the optional payload `arguments`, with snake_case operation names.

```json
{"version":1,"request_id":"req-1","session":null,"project":null,"expected_revision":null,"command":{"operation":"capabilities"}}
```

Responses contain matching `version` and `request_id` and a Rust/Serde externally
tagged `result`: either `{"Ok": {"kind": "...", "value": ...}}` or
`{"Err": {"code": "...", "message": "...", "retryable": false}}`.
Unknown transport outcomes are not proof that a command did not commit.

Project edits, undo/redo, rebases and candidate commits require an explicit
expected revision. The engine checks grants, protected regions, revision and
request identity within its authoritative transition. Retries reuse the same
request ID and exact request contents; changed contents produce request conflict.
`request_fingerprint` returns exact typed JSON bytes excluding the request ID,
including the authentication token. The engine hashes these bytes before durable
dedupe storage and never stores or logs this plaintext fingerprint. It is not a
cryptographic artifact hash. Dedupe storage is scoped to authenticated
identity; client-selected IDs do not confer authority.

Durable commits may be replayed from recorded outcomes. Unknown native/device or
external effects are not blindly repeated. Disconnecting does not revoke grants
or cancel offline work. Explicit revocation cancels solely authorized work and
withdraws device authority; it does not reverse previously committed edits.

## Schemas and command families

The `Serialize`/`Deserialize` DTO definitions in `src/lib.rs` and `src/worker.rs`
are the executable v1 schema. Checked domain deserialization lives in core.
`Request::validate` enforces envelope and operation constraints, not privileges.

| Family | Commands | Results |
| --- | --- | --- |
| Project | CreateProject, OpenProject, GetSnapshot, ImportSource, PinSource, EvictSource | Project, Source or acknowledgment |
| Edit | ApplyEdit, Undo, Redo | New project revision |
| Generation | Generate, JobStatus, CancelJob, ResumeJob | Job |
| Candidate | GetCandidate, RebaseCandidate, CommitCandidate | Candidate or project |
| Viewer | PreviewFrame | Frame artifact and frame-associated observations |
| Export | Export | Exported path and pinned revision |
| Authority | Pair, Grant, Revoke | Session or acknowledgment |
| State | Capabilities, Events | Explicit capabilities, cursor page |
| Device | Arm, Stop, SwitchPlaybackRevision | Qualified engine result or unsupported |

`ImportSource` scope separately controls filesystem import, including model paths
on generation/preview requests. `ManageProtection` separately controls protected
regions and undo operations that change them. Neither permission follows from an
ordinary `Edit` grant.

The initial engine admits inline serialized motion programs up to 512 KiB, leaving
room inside the 1 MiB control envelope. Larger programs require an artifact-backed
or paginated editing surface and must be rejected before candidate publication or
commit with `resource_exhausted` until that surface is available. Bulk frame and
artifact codecs do not by themselves implement large-project editing. The engine
must also bound source lists, event pages and other snapshot metadata; a bounded
program does not bound the entire response.

Events carry a monotone durable cursor, project identity and revision. A lost or
expired cursor requires snapshot resynchronization; clients must not infer a
complete history from a gap. Volatile preview/progress telemetry is not durable
edit history. A server enforces page limits and project grants before exposing
events. A time-based seek returns the actual decoded rational timestamp and frame
ordinal, not a guessed constant-rate index, and echoes `requested_source_time`.
Its source version, placement,
transforms, seek generation and request generation must match the request. The
returned context contains the actual frame identity; observations must match that
exact context. `matches_request` requires exact frame identity;
`matches_seek_request` separately validates seek freshness and the requested-time
echo. Passing the seek check does not assert that the guessed requested frame was
displayed. Clients adopt the returned actual context for the displayed frame rather than
relabelling old observations. Absent/pending analysis never supplies a fabricated
detection. Independent viewer state does not move engine-owned physical playback.

## Workers, extensions and artifacts

Workers receive immutable source/tool identities, project/base revision,
job/attempt identity, private output directory and finite resource budgets.
Generation returns a candidate artifact, never an instruction to commit it.
Preview returns a frame artifact and explicitly available/pending/unavailable
analysis. Source timestamps remain rational values distinct from project time.

Artifact identities use lowercase SHA-256 and exact byte length. Descriptor
validation is not byte verification. The engine and worker must verify actual
bytes, source snapshots and dependency pins. Checkpoints require exact source,
configuration and dependency matches; fallback creates a new attempt.

Tensor validation checks rank, nonzero dimensions, contiguous size arithmetic,
offset bounds and admission limits. Arbitrary mutable buffers and strides are
not v1 interfaces. Artifact transfer grants are project/session scoped. Reusing
already disclosed buffers for another project's data is forbidden; revocation
cannot erase bytes a recipient already received.

Extension manifests declare roles, immutable build identity, ABI, requested
scopes, budgets and confinement requirements. Manifest declarations cannot grant
authority, accept driver qualification or approve unsandboxed execution.
Unsandboxed exact-build loading approval and assistant invocation approval remain
separate. Assistant calls require approval each time unless a human separately
enables the explicitly risky, scoped, exact-build standing grant. Changed bytes
or revocation invalidate approvals. No sandbox guarantee applies to the native
code admitted through an unsandboxed exception.

## Compatibility and evidence

Version mismatch fails explicitly. Public wire compatibility does not guarantee
internal Rust ABI compatibility. Documented CLI aliases/import compatibility and
standard funscript interoperability retain roadmap authority.

Unit tests cover bounded framing, malformed/truncated input, partial I/O, tensor
overflow, artifact ranges and layout validation. They do not establish OS-level
confinement, neural quality, crash durability or physical stopping safety.

Transport API reference: [interprocess local sockets](https://docs.rs/interprocess/latest/interprocess/local_socket/index.html).
