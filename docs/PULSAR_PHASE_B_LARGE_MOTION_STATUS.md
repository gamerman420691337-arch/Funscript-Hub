# Pulsar Good Phase: large-motion and startup checkpoint

Date: 2026-09-11
Status: **IN PROGRESS. Good Phase B1-B6 is not complete.**
Authority: [software specification](PULSAR_SOFTWARE_SPECIFICATION.md), [architecture-first execution plan](PULSAR_ARCHITECTURE_FIRST_EXECUTION_PLAN.md), and [major roadmap](PULSAR_MAJOR_ROADMAP.md).

The large-motion path is implemented and its local structural, contract, process, model and declared GUI checks passed. This does not qualify resource peaks, realtime behavior, neural accuracy, release platforms or physical devices. No whole B milestone is marked complete and Solid phase has not started.

## Delivered contracts

- Immutable motion objects and compact project/candidate descriptors replace inline programs on wire v2. The former 512 KiB publication/1 MiB inline-response blocker is removed without thinning motion.
- Secured, authorization-scoped bulk leases carry bounded chunks. Aggregate admission, expiry, revocation, restart fencing, exact retry and durable outcomes are enforced.
- Values-only uploads cannot assert observed evidence, provenance, model qualification or worker authority. Engine reconciliation assigns authored ancestry and preserves unchanged evidence and exact interpolation lineage.
- Candidate finalization never commits. Existing expected-revision, protection, rebase, undo/redo and sole engine commit authority remain.
- Shared CLI/GUI hydration validates descriptor binding and complete digest before use, with a bounded cache. The editor preserves explicit gaps and refuses misleading continuous rendering for nonmonotonic drag drafts.
- Startup uses the bounded connector and one ten-second budget. Busy sockets do not trigger replacement; a verified held instance lock is only an advisory reason to await an initializing owner.

Details and tradeoffs: [ADR 0008](adr/0008-motion-artifacts-and-transfer-leases.md). This supersedes ADR 0007's unimplemented status for the transport slice, not the product specification.

## Confirmed corrections

| Finding | Disposition |
| --- | --- |
| Distant authored edits rejected by whole-track protection | RED to GREEN. Exact span/anchor protection is used only for engine-backed authored candidates; ordinary generated replacement policy is unchanged. |
| Transfer commands and ordinary mutations could reuse request IDs | RED to GREEN across Begin, Finish and normal mutations; one durable identity namespace. |
| Fresh-ID alias of completed Finish was not journaled | RED to GREEN, including restart. Identity and successful response now commit atomically. |
| Abandon replay was not durable | Authenticated durable outcome/replay added and independently checked. |
| Expired idle leases retained reservations until another request | Periodic and dispatch/admission sweep added. Nominal cadence is not a hard real-time bound. |
| Fallible actor lookup could strand the finalizing flag | Fallible preparation moved before marking finalization active. |
| Unsorted in-flight drag could draw through an explicit gap | Actual-source probe RED to GREEN; nonmonotonic draft emits no misleading continuous spans. |
| Launcher blocked before reaching bounded client code | Kernel EAGAIN saturation and actual CLI timeout reproduced; bounded connector now used by startup. |
| Competing launcher failed before the original owner published its socket | Controlled lock-before-listener regression reproduced premature exit around 50 ms; bounded owner wait now passes. |
| Backlog test fixtures exceeded ordinary descriptor limits | Separate fixture defect. Small listen backlog now proves saturation with two connections and runs under a 1,024-descriptor limit. |

The first backlog fixture treated a short scheduling timeout as saturation; that was inconclusive and was not used to justify a patch. Formatter filename headers and an environment-stripped test insertion were tooling mistakes repaired before final gates. An initial export test assumed floor-millisecond rounding; only that oracle was corrected to the existing checked nearest-half-up contract.

## Final tested subject

Source fingerprint: `8b12bd2343e9c10b16485f517bc8a28af33b0a2b323ce625bb45965bae05bc1f`.

Linux debug GUI/CLI binary SHA-256: `1700a4d9d485add4320d6a270d23903c41d45055633f56ae375b8f34d29cc81a`.

| Gate | Result | Scope |
| --- | --- | --- |
| Complete motion-transfer qualification script under `ulimit -n 1024` | PASS, exit 0; **305 Rust tests, 0 failed, 0 ignored** | Workspace/all-target structural and build checks, unit/contracts and real secured-process tests. Full 64,010-byte log preserved. |
| Existing lifecycle TLC model | PASS; 273 distinct states, depth 9 | Existing bounded lifecycle abstraction; one authorization mutant killed. |
| Transfer TLC model | PASS; 852 distinct states, depth 13 | Four named authorization/epoch/digest/revision guard-removal mutants killed. |
| Large secured-process scenarios | 7/7 within final whole gate | 54,000 stroke actions and 324,000 six-axis actions through transfer, revisions, rebase, protection, history, restart and axis exports. |
| New startup cases | 2/2 within final whole gate | Actual saturated endpoint and controlled initializing-owner window; finite parent deadlines. |
| Read-only instance-lock tests | 5/5 within final whole gate | No-create/unheld/held and unsafe state/lock type, permission, symlink and FIFO handling. |
| Independent review | Scoped findings closed | Core author's own tests distinguished from independent storage/client/engine/launcher review; no remaining confirmed production finding in the reviewed slice. |
| Final binary GUI smoke | PASS for declared steps | Saved large project reopened at revision 3, dense Yaw shown, review warning retained, exact CLI/GUI export parity. |
| Kani | NOT RUN | No bounded Rust proof is claimed. |
| External lamu review tool | UNAVAILABLE | Earlier `review_diff` returned `Transport closed`; not a pass. No commit was made. |

[Full gate log](evidence/phase-b-large-motion/final-gate.log), [manifest](evidence/phase-b-large-motion/increment.json), [source/test hashes](evidence/phase-b-large-motion/source-hashes.sha256), [independent review](evidence/phase-b-large-motion/independent-review.md), and [model correspondence limits](../assurance/MOTION_TRANSFERS.md).

Earlier metric/artifact campaigns remain historical synthetic evidence. They are not added to the 305-test count and were not requalified as real-media results here.

## Large-program and GUI evidence

The actual storage tests cover a 3,169,071-byte stroke object and a 19,014,348-byte six-axis object. Observed process-test control replies were 613 bytes for stroke and 869 bytes for six axes, with 256 KiB maximum chunks. These are fixture measurements, not universal maximum-response or throughput claims.

A separate 30-minute deterministic Yaw program contained **180,001 actions** in a **13,919,716-byte** motion object. Its generation is explicitly not local AI inference. Before the extra launcher repair, GUI undo created revision 2 and redo revision 3; reopening retained revision 3 and unresolved `constrained_pattern_parser_not_local_ai` review. The final binary reopened that saved project and exported byte-identical standard funscript output.

Export SHA-256: `ee7f29f641fe50c8f3ded8c23501d51b253d52b8edd9ac2758a7a40918cdafb1`.

[Final reopened timeline](evidence/phase-b-large-motion/final-reopened.png), [final export](evidence/phase-b-large-motion/final-export.png), [earlier undo](evidence/phase-b-large-motion/pre-launcher-undo.png), and [earlier detailed review](evidence/phase-b-large-motion/pre-launcher-doctor-revision3.png).

The earlier private Xvfb exited for an unproven reason; only owned QA processes were restarted. An early GUI launch exposed the initialization-window symptom, subsequently covered by a deterministic process regression. Earlier automated typing did not produce the assumed filename suffix: the acknowledged extensionless path exported correctly. Exact initial input-event cause is unproven. Final slower entry with a post-typing wait exported the intended `final.funscript` path. Dense UI input/frame latency remains unqualified. A tool-output serialization error occurred after that successful final export; saved exit 0, output and image were recovered without repeating the export.

All QA used an isolated private display and software rendering. No user desktop, private media, shared GPU workload or physical device was used. Owned QA sessions were stopped.

## Remaining phase work and next slice

| Area | Still required |
| --- | --- |
| B1 | Permitted real corpus, curated references/categories, five-human-hour pilot, and remaining statistical/acceptance decisions. Zero human sessions or real benchmark subjects supplied. |
| B2 | Real detector/tracker/VR/cut/occlusion comparisons, calibrated material-error review recall and human editing burden. Synthetic boxes do not qualify real detection. |
| B3 | Licensed bundled runtimes/models/tools, real offline assistant inference, vendor-equivalent fallback, reference CPU/GPU throughput and RAM/VRAM/cold/warm campaigns. |
| B4 | Semantic text/image/audio, full stereo/VR180/360/mixed/live support, and observed/inferred six-axis reconstruction. Deterministic patterns remain explicitly synthesized. |
| B5 | Real Handy/Handy 2 online/offline BLE, live scheduling/adaptation, stopping/reconnect/controller/latency evidence and explicit human hardware acceptance. |
| B6 | Portable project packages, explicit human review resolution, bounded real assistant/plugin execution, dense timeline usability and remaining adapters/recovery cases. |
| Cross-cutting | Measured validation/commit/merge latency and memory peaks, OS/vendor qualification, formal implementation correspondence and remaining fuzz/mutation/fault campaigns. |

Existing commit/merge/history object reads can still occur inside authority transactions. The 1 GiB validation reservation and other configured budgets are not evidence of measured worst-case memory or control latency. There is no nonblocking/live-deadline claim for the entire authority path.

Next executable proposal: [portable-project implementation plan](PULSAR_PORTABLE_PROJECT_IMPLEMENTATION_PLAN.md). B6-P1 is a project-only consistent snapshot, versioned bundle, fresh-project import, then edit/history/neutral-export round trip. It must not restore sessions, grants, jobs or device authority. That plan is **PROPOSED**, not implemented, and preserves the original media/provenance and qualification boundaries.

Remaining engineering is not excused by missing external inputs. No phase-completion keyword, flawless-program claim, Preview/Stable promotion, commit or physical qualification is implied.
