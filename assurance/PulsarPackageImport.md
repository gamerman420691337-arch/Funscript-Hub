# Clone import lifecycle model and correspondence

This finite model checks one import operation, two chunks, one process restart, and one local project result. It abstracts complete bytes, graph validation, and SQL transaction boundaries rather than implementing them. Passing the model is not proof of Rust, SQLite, filesystem, parser, origin mapping, or neural correctness.

## Separated boundaries

Uploading, immutable sealing, strict validation, durable original recovery, object publication/synchronization, atomic project visibility, and acknowledgement are distinct transitions. Sealing synchronizes only the provisional staging file; it does not establish crash-recoverable directory-entry durability. Full validation precedes durable original-container publication. The original container must become durable before other imported objects or active project state can be published. Source-origin authority is never imported; the creator grant is assigned only with complete active rows and the durable request result.

An existing content-addressed object cannot replace validation of newly uploaded bytes. Invalid content and invalid graph structure are independent possible inputs. Cancellation preserves a live worker's reservation until exit. Restart interrupts precommit work but preserves a completed project/result. Cleanup must not delete preexisting shared objects.

The model represents authentication invalidation as an abstract loss of creation authority. Existing project-scoped grant revocation is not automatically equivalent to revoking an unrelated projectless import. Concrete cancellation/authentication checks must match the actual API authority.

## Campaign

```sh
TLA2TOOLS_JAR=/path/to/tla2tools.jar node scripts/qualify-package-import-model.mjs
```

The safe configuration and sixteen expected negative configurations are checked. Mutants remove complete-byte, content, graph, CAS-hit, original-recovery sync, object sync, authorization, atomic-row, fresh-identity, inert-authority, reservation, shared-object, replay, terminal-replay, request-binding, and acknowledgement safeguards. A mutant requires the exact expected invariant and normal TLC failure exit 12. Parse failures, signals, missing tools, and surviving expected mutants fail the campaign.

The request-binding mutant substitutes another input identity under the accepted request, carries that identity through sealing, and commits the wrong result. The invariant compares the visible result with the accepted intent; it is not an invariant over an otherwise unused marker. These identities abstract exact payload/request equality and do not prove hashing or serialization.

Recovery synchronization and object synchronization have separate mutants. Removing the recovery-publication ordering safeguard must violate RecoveryBeforeObjectPublication; removing object synchronization must invalidate durable visibility. Neither mutant establishes actual operating-system flush behavior.

## Concrete correspondence obligations

| Boundary | Required engine/client evidence |
| --- | --- |
| Full-byte validation | Correct CAS content already present, but caller uploads corrupted bytes with valid outer hash; import rejects. |
| Seal/validation | Truncation, bad canonical format, mismatched version, invalid object/hash/graph and trailing bytes publish no active project. |
| Immutable recovery | Exact original container preserved before migration; concurrent chunks cannot alter a sealed input. |
| Fresh identity/inert authority | Clone into another actor while original project exists; active IDs differ, archive actors/jobs/request IDs have no local authority. |
| Complete visibility | Inject failure before/after durable object publication and SQL commit; observe either no active project or one complete editable project. |
| Cancellation/resources | Cancel active work; no later publication, no early live-worker reservation release, eventual cleanup remains accounted. |
| Crash/replay | Interrupted precommit import does not auto-publish; lost postcommit acknowledgement and restart return the same project. |
| Shared-object cleanup | Import failure cannot remove previously committed sources/motion/evidence shared by other projects. |
| Editing/re-export | Preserved history/protection/candidates support editing and a second clone; original bytes and origin mappings remain coherent. |

The last row is intentionally outside this small lifecycle state space. Depth/metadata caps, exact canonical bytes, transitive origin mapping, aggregate quotas (including canceled in-flight unsealed writes), cross-session isolation, upload-generation/acknowledgement races, upload cursor replay, and real filesystem durability need separate implementation tests. This is a safety model without a fairness or wall-clock completion proof.
