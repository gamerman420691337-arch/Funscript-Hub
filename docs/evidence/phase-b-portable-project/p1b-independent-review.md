# P1b scoped review and regression disposition

This is a record of bounded first-read reviews and owner verification. It is not an external commit approval or an independent second review of repaired source.

## Client boundary

Independent review confirmed hidden Start/Begin request identities after lost acknowledgements, fresh per-RPC timeouts exceeding an overall wait deadline, and an unconditional retained-copy claim. Repairs provide caller-supplied identities, bounded same-ID transport replay, absolute control deadlines, and a statement that this helper did not request release rather than asserting current remote availability.

The new lost-Begin test exposed a retry-classification defect: TransportError did not expose its nested I/O error through Error::source. Explicit typed matching repaired the actual EOF path. A reuse test separately reproduced stale verification progress (512 bytes rather than zero); resetting progress under exclusive ownership repaired it.

Final owner client gate: 53 passing tests. Protocol owner gate: 98 passing tests, including five absolute-deadline real-socket cases. These owner gates precede the final combined source qualification recorded in P1b status.

## Storage boundary

A failed staging unlink left 52 bytes after its reservation ended, but accounting reported zero. A deterministic failed-unlink fixture reproduced that defect. All private pending bytes are now charged, conservatively alongside live reservations.

A cleanup pass that removed a file and then failed reported ordinary Unavailable without treating the partial effect as uncertain. A second deterministic regression required UnknownOutcome; directory synchronization is now attempted after cleanup effects, including later-error paths.

Both findings had RED then GREEN evidence. Final owner storage gate: 17 passing tests. FD anchoring, metadata retention guards, fixed-buffer verification, and no-clobber publication were included in the bounded first read; no additional confirmed defect was found in that scope.

## Engine authority and resource lifecycle

The following defects were confirmed and repaired through deterministic internal interleavings or failure injection:

- A committed revoke followed by regrant could admit an old lease before an advisory callback. Durable authorization generations now fence leases, verification, and pending requests.
- A draining verifier's slot could be overwritten by a replacement. The old worker retains its slot until exit.
- Final Begin admission did not recheck the pending request's captured generation after unlocked I/O. The final durable transaction now checks it again.
- A delayed broad revoke callback cancelled fresh regranted work. Advisory cleanup now targets older generations.
- Panicking export/verifier thread creation happened after admission and could leak slots/holds. Fallible spawn and explicit outcome cleanup cover both cases.
- Filesystem enumeration held the package runtime mutex while another path could hold the database waiting for that mutex. Enumeration now occurs outside the runtime mutex using a conservative reservation snapshot and artifact gate. This last repair has a source lock-order argument; do not label it a measured blocked-enumeration regression unless a separate fixture records that observation.

The first generation race had 8 passing tests and 1 failure before repair. The later four race/spawn cases had 12 passing tests and 4 failures before repair. Final focused owner lifecycle gate: 16 passing tests. The internal scheduler/barrier tests are not claims that a black-box process test reproduced the same scheduling window.

## Model review

A first model incorrectly allowed a lease while verification was still Verifying because epoch equality remained true. Strengthening the invariant first produced a TLC counterexample (normal invariant-failure exit 12); adding a completion guard repaired it. Current-generation authority and process epoch are separate, and a verifier retains its slot until finish or abandonment.

Runner review also found that a signalled child with a null exit status could be classified as a killed mutant after printing the expected diagnostic. Classification now requires normal TLC invariant-failure exit 12, no signal/error, and the exact expected invariant. Synthetic exit-classification checks run before the model campaign.

After adding the completion guard, deleting one redundant epoch check no longer produced a counterexample. The runner correctly failed that campaign; its log is retained. The epoch-negative case now removes admission's independent stale client-request epoch fence instead of pretending that a redundant-check deletion proves something.

Final finite model: 4,088 distinct states, 15,885 generated states, and twelve expected safeguard-negative counterexamples. These bounds do not prove implementation correctness, global caps, full closure enumeration, arbitrary partial writes, or physical/filesystem behavior.

## Process-fixture corrections

Initial process-test failures included incorrect assumptions, not product defects:

- New creators intentionally receive PackageProject. A legacy-equivalent negative fixture requires a real two-manager revoke/regrant sequence; Grant is additive.
- Cancelled acknowledges publication fencing, not synchronous worker/hold teardown. The correct test requires bounded eventual eviction while accepting only the specific temporary held-source rejection.
- Terminal/revoked leases may report typed Unavailable; exact scenarios now use the declared terminal semantics.
- A full shared /tmp caused ResourceExhausted and linker SIGBUS. Those runs are infrastructure failures, not passes.

The large fixture contains generated sparse marker data and verifies complete bytes. It does not qualify a real detector, import semantics, neural quality, physical hardware, or reference-device performance.

## External review status

No commit was requested or created for this slice. Earlier required commit-review attempts returned Transport closed; no external PASS is implied by this record. Final combined commands, identities, metrics, and remaining limitations are recorded separately in the P1b status/evidence.
