# Phase A lifecycle model and correspondence obligations

This finite model is an executable design artifact, not proof of the Rust engine or physical safety. `Arm` abstracts a successful qualification/admission predicate; physical transport behavior, timing bounds, storage hardware and untrusted native runtimes are outside this model.

The model bounds revisions to three and reconnect epochs to two in the accompanying configuration. It models snapshot candidates, durable edit publication, cancellation/revocation fencing and disarmed reconnect. The `NoUnauthorizedCommit` counter records authorization at each edit/commit transition. Removing the Edit authorization guard must violate this invariant in the mutation sanity check. Concrete implementation tests independently attempt unauthorized transitions; model checking alone does not establish implementation correspondence.

Run with an installed development TLC tool:
```sh
java -cp /path/to/tla2tools.jar tlc2.TLC -config PulsarLifecycle.cfg PulsarLifecycle.tla
```

Required correspondence fixtures use the actual engine/protocol:

| Model transition | Concrete observation required |
| --- | --- |
| Start/Finish | Engine-created immutable job manifest produces a candidate bound to its input revision. |
| Edit/Commit | Authorized expected-revision transaction records state and request outcome atomically before acknowledgement. |
| Commit after Edit | Stale candidate rejected until explicit rebase and fresh commit. |
| Cancel/Revoke then Finish | Delayed worker completion cannot become an authorized candidate/commit. |
| Duplicate request | Same request and payload return recorded outcome; changed payload under reused identity rejects. |
| Reconnect | Snapshot/session resynchronization; physical state remains disarmed and old authority is not replayed. |

Phase A requires models and real correspondence-test entry points. Required critical formal results gate Preview; no TLC/Kani success is asserted by checking these files into the workspace.
