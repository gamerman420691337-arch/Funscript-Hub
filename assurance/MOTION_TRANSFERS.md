# Motion Transfer Model and Implementation Correspondence

This finite model covers a single upload lease/project with bounded byte counts, revisions and engine epochs. It is a design-checking artifact, not proof of Rust code, filesystem durability, cryptographic behavior, actual memory accounting, transport deadlines, download authorization or physical behavior.

The model represents digest verification and durable candidate/commit publication as abstract atomic steps. Publication failure, rejected requests, duplicate acknowledgments and chunk replay that has no additional effect are stuttering steps. Concrete failure injection and idempotency tests must independently establish those implementation properties.

Disconnect changes connection state without revoking authority or freeing a live lease. Revocation aborts unfinished transfers; restart invalidates unfinished leases while retaining finalized candidates and durable revisions. Expiry/abandon releases the abstract reservation. Finalization publishes a candidate, never a project revision.

## Running

```sh
TLA2TOOLS_JAR=/path/to/tla2tools.jar node scripts/check-motion-transfers-model.mjs
```

The runner bounds the Java heap, enforces a timeout, checks the baseline and requires four deliberate guard-removal mutants to violate the corresponding invariant. Unavailable TLC is NOT RUN with exit 2, never a pass.

## Required implementation correspondence

| Model action or invariant | Concrete observation required |
| --- | --- |
| Begin / ReservationMatchesLifecycle | Aggregate memory/storage admission precedes upload staging; refusal leaves no lease or revision mutation. |
| AppendChunk / NoUnauthorizedEffects | Actual bulk endpoint rejects wrong actor/project/grant or stolen leases. |
| NoStaleEpochEffects / Restart | Old-epoch leases cannot write after restart; finalized candidates and revisions remain recoverable. |
| VerifyDigest / NoBadDigestPublication | Truncated or incorrect-hash uploads never publish candidates; malformed edit values cannot forge evidence or lineage. |
| Finalize | Checked engine-owned candidate is bound to captured project/base revision and finalized request replay is idempotent. |
| Commit / NoStaleCommit | Concurrent edit makes candidate commit stale; explicit rebase produces a fresh binding. |
| DurableAcknowledgement | Actual crash/restart and storage-fault probes establish object-before-SQL publication and atomic revision/outcome persistence. |
| Disconnect / Reconnect | Interrupted connections preserve authorized work until expiry or revocation; prefix/retry semantics remain explicit. |
| Revoke / AbandonOrExpire | Subsequent chunks and finalization reject; staging reservations release only after active writers are fenced. |

Root process fixtures are planned as large_stroke_artifact_roundtrip_rebase_history_restart_and_export, large_six_axis_artifact_roundtrip_preserves_every_action, and large_transfer_authority_integrity_and_restart_fences. Passing these fixtures establishes only the cases actually asserted; their names are not blanket proof of all rows.

No Kani, full mutation campaign, full fault-injection campaign, Preview or whole Good-phase completion is implied.
