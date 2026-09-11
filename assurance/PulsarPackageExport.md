# Portable export lifecycle model

This is a finite design model, not proof of the engine implementation.

## Bound and state separation

The checked configuration has one operation, revisions 0 through 2, two process epochs, two authorization generations, and at most two chunk admissions. Expected revision stays fixed while current revision can change before and after capture. Historical readyRecorded remains true after release; present bundle availability and operation phase are separate.

PublishBytes creates a complete file; SyncNamespace records the abstract durability boundary. RecordReady commits the receipt and releases source holds. This abstraction assumes successful file data synchronization before namespace durability; it does not implement fsync or storage recovery.

Revocation fences publication even if a later grant returns. A chunk admitted before revocation may complete afterward. WriteFinish advances the abstract cursor; WriteFailure does not. Read/write holds prevent release cleanup from removing bytes still in use. Exact previous-chunk replay and arbitrary partial byte counts are delegated to protocol tests, not proven by this model.

The model separates historical Ready from post-restart epoch verification. Current-generation verification must finish before lease issuance; matching generation/epoch fields alone is insufficient. One verifier slot remains occupied until finish or abandonment. Client-presented read epochs may be stale, so admission checks them independently of the server's verified lease. It does not model a large hash inside a control handler; real control-response timing must be tested.

## Run

```sh
TLA2TOOLS_JAR=/path/to/tla2tools.jar node scripts/qualify-package-model.mjs
```

The runner checks the safe model and twelve intentionally faulty configurations. A faulty configuration passes the campaign only when TLC reports the expected invariant failure. Parse errors, timeouts, missing tools, unexpected invariant failures, and nonzero safe-model exits fail the campaign. A mutant requires the expected invariant diagnostic and normal TLC invariant-failure exit 12; a signal or missing exit code cannot count as a killed mutant.

## Required implementation correspondence

| Model boundary | Concrete evidence required |
| --- | --- |
| Capture and SnapshotExact | Stale expected revision rejects; edits after capture do not alter the exported revision/catalog. |
| SourceRetention | Concurrent source eviction cannot remove captured objects; cancellation does not release live worker reservations. |
| PublishBytes / SyncNamespace / RecordReady | Data and directory durability precede Ready acknowledgement; crash/failure windows preserve honest outcomes. |
| Publication authority | Revocation during streaming prevents Ready, including later regrant and late completion. |
| Restart / verification / LeaseIsVerified | Old leases reject; Begin responds Pending without blocking on full hash; corrupt/missing artifact never gets a lease. |
| AdmitChunk / AuthorizedAdmission | Exact session/project/operation/range binding and current authority are rechecked after reads. Already-admitted bytes are not retractable. |
| WriteFinish / WriteFailure | Successful full write advances cursor; truncation poisons the connection; exact last replay does not double-advance. |
| ReadRetainsBundle / Release | Release invalidates new reads, preserves active I/O holds, and eventually removes unreferenced bytes without losing tombstones. |
| Replay / NoReplayResurrection | Same request returns the recorded outcome; changed intent rejects; cancelled/interrupted/released requests cannot restart. |

One-operation checks do not prove global admission caps, cross-project alias isolation, atomic capture implementation, complete artifact enumeration, parsing, hashes, filesystem durability, or fairness. The absence of counterexamples in this bound is not an unbounded correctness claim. Test results and outstanding correspondence gaps belong in the P1b status artifact.
