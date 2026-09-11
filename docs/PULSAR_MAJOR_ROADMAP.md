# Pulsar Major Roadmap

Version: 1.2
Date: 2026-09-11
Status: Approved implementation direction; milestones require evidence before closure

## Authority and scope

The [Pulsar Software Specification](PULSAR_SOFTWARE_SPECIFICATION.md) is the product contract. Its confirmed requirements and Appendix D decision sources take precedence over this roadmap, implementation notes, README feature claims, and historical plans. Design defaults and open choices remain labeled as such.

The [Architecture Decisions and Design Continuation](PULSAR_ARCHITECTURE_DECISIONS.md) records ARCH-001 through ARCH-041, distinguishes submitted decisions from selected technical contracts, and consolidates the source map, module responsibilities, interfaces, and assurance obligations. Architecture discussion is complete. Implementation, regression closure, performance evidence, and release/hardware qualification remain incomplete. The architecture record supplements this roadmap without superseding the software specification's product requirements.

## Authoritative execution order: architecture first

ARCH-041 supersedes the earlier M1-first delivery order. The [Architecture-First Execution Plan](PULSAR_ARCHITECTURE_FIRST_EXECUTION_PLAN.md) is authoritative for sequencing; the software specification remains authoritative for product requirements. M1-M8 below now identify scope and acceptance workstreams, not a strict chronological implementation order. Their defects, compatibility requirements, numerical gates, and qualification obligations remain binding.

1. **Phase A: architecture scaffolding.** Establish the final four-library shape, shared authority interfaces, enforced dependency directions, and representative real input-to-result vertical paths. Move supported implementations behind those boundaries. Fix only correctness defects that block the migration; keep the remaining historical M1 defects explicitly open. Unimplemented or unqualified capabilities must be disabled or reported as unsupported, not represented by hollow success-returning stubs.
2. **Phase B: make it good.** Complete functional behavior and correctness through the new architecture, including the full M1 regression and GUI tracking work. Address difficult numerical, model, runtime, and device-integration questions with deep research, falsifiable experiments, and ADR-recorded scientific reasoning. Preserve unsuccessful or inconclusive evidence rather than treating research as qualification.
3. **Phase C: make it solid.** Complete hardening and acceptance evidence across M1-M8: critical formal gates, fault/recovery and adversarial testing, performance and real-media qualification, physical device qualification, compatibility, and portable release evidence. Earlier green paths or research results do not substitute for these gates.

Phase A exits on real composed behavior and enforced ownership boundaries, not directory structure, type declarations, or placeholder APIs alone. It must demonstrate that supported client paths use the shared authority interfaces and that dependency directions are enforced. Architecture completion does not qualify unsupported models, devices, backends, or platforms.

This roadmap authorizes staged implementation of that contract. It does not assert that current models, hardware paths, benchmarks, or packages satisfy it. The product owner requested that the roadmap be committed first, followed by milestone planning, the first milestone, debugging, and substantial correctness review.

Implementation starts from upstream `7a4f5671c14731b238b64c4827294ce029928dda`, selected by the product owner in a separate worktree. The older `434b830c2c7d004d04830d299e8c5159d43b047e` checkout and its divergent history remain preserved. This is a pinned starting point, not a permanent claim about the latest upstream revision. Existing upstream licensing is not changed by this roadmap.

## Product outcome

Deliver portable, offline-first multimodal funscript generation, tracking, editing, local assistance, and synchronized device playback. Stroke fidelity has priority; six-axis support remains required. Quality and speed must be measured against curated references, not inferred from model names or a working UI.

Core ships with all required Fast input families and the local assistant usable offline from first launch. Complete includes every advertised quality tier. Windows, Linux, and Apple Silicon macOS are launch targets, with NVIDIA, AMD, Apple, and Intel acceleration required on qualified hardware. Other ARM64 and Intel Mac priorities remain as specified.

## Implementation principles

1. Correctness before optimization: preserve pauses, shallow motion, timestamps, target identity, and authored work before chasing throughput.
2. Shared semantics: GUI, CLI, batch, and live paths use common domain contracts; fixes must reach real consumers rather than unused helpers.
3. Real inference: placeholder wrappers, pending hashes, fabricated detections, and silent model substitution do not qualify a capability.
4. Honest uncertainty: best-effort output remains distinguishable from observed truth and carries actionable review information.
5. Safe adaptation: retain the device-neutral master, preserve timing, and constrain device-adapted amplitude/dynamics.
6. Evidence-bound closure: tests, real-media observations, hardware qualification, and release metrics are distinct forms of evidence.
7. Preserve user work: no silent replacement of existing scripts, protected edits, or repository history.
8. Changes remain bounded: introduce the smallest useful shared contracts, not an unvalidated wholesale rewrite.

## Milestone sequence

| Milestone | Purpose | Required work and exit evidence |
| --- | --- | --- |
| M1: Baseline and stroke correctness foundation | Establish trustworthy geometry, tracking state, and motion extraction before larger model changes. | Pin baseline; reproduce current defects; fix bounding-box coordinate/decoder/consumer errors where demonstrated; test identity/reacquisition boundaries and confidence math; prevent invented flat-signal motion and unintended amplitude expansion; record real-media validation separately. |
| M2: Shared generation and editable projects | Make one generation contract serve actual application entry points. | Align GUI/CLI/batch settings and behavior; protected edits, selective regeneration, undoable versions, recoverable exports, truthful progress/cancellation, and explicit uncertainty propagation. |
| M3: Real local inference and reference performance | Select real pretrained models/runtimes from evidence and close measured gaps. | Model/runtime comparison; real model manifests and execution provenance; CPU/reference-GPU stroke speed measurements; bounded memory; targeted fine-tuning only where justified; local assistant actions and memory handoff. |
| M4: Full multimodal and six-axis generation | Complete required inputs without confusing synthesis with measured physical motion. | All media families, synthetic text/image authoring, faithful audio/video precedence, VR/projection handling, neutral six-axis tracks, secondary-axis provenance, and separate quality evaluation. |
| M5: Live scheduling and device qualification | Deliver synchronized output with explicit transport and failure behavior. | GPU fastest-preset live six-axis, CPU best-effort frame dropping, shared-clock synchronization, device profiles, Handy/Handy 2 online and offline BLE, upload consent, disarming, and stale-command handling. |
| M6: Portable releases and vendor coverage | Make the product install-and-use without user-managed inference dependencies. | Core/Complete artifacts, first-launch extraction, offline pack import, compatible updates, 16 GB baseline qualification, all required GPU vendors including Maximum, and supported OS/architecture matrices. |
| M7: Public Preview qualification | Meet the Preview contract without borrowing Stable claims. | Applicable stroke speed gates; per-category Default precision/recall >=80% with 90% target; material-error flag recall >=95%; measured human effort; explicit experimental label for unqualified six-axis playback. |
| M8: Stable and comparative qualification | Establish release-quality evidence across the full promised scope. | Per-category Default precision/recall >=95%; <=5 minutes editing and <=10 minutes total human effort for >=95% of 30-minute clips per category; physical six-axis qualification; resolved creative/secondary-axis criteria; reproducible comparisons before SOTA claims. |

Milestones are dependency-oriented, not calendar promises. M1 deterministic engineering can begin before the human annotation pilot is complete; that does not mark data qualification complete. Internal stroke-first work does not reduce launch modalities or device scope.

Detailed work packages belong in `PULSAR_MILESTONES.md` and evidence receipts under `docs/evidence/`. A planned milestone is not completed merely because its document or source files exist.

## First milestone: bounding-box and motion correctness

M1 must address the complete chain from detector output to motion, not only the drawn rectangle:

1. Establish supported tensor layouts, coordinate units, preprocessing transforms, and the canonical box representation. Reject malformed/non-finite boxes and avoid scale/letterbox/crop mistakes.
2. Trace that representation through detection selection, overlap/association, point seeding, motion estimation, and visible overlays where present.
3. Exercise portrait/landscape inputs, borders, degenerate boxes, detector-order changes, competing targets, occlusion, and scene transitions at the appropriate seam.
4. Check confidence and forward/backward consistency against actual directional flow rather than mathematical labels.
5. Preserve stillness and relative excursion through event extraction; missing observations must not silently become confident full-range strokes.
6. Add deterministic regressions for each confirmed fix and exercise the original consumer path. Record baseline failures separately from new regressions.
7. Run focused debugging passes, a broader relevant suite, and mandatory per-commit external review. Verify review findings before fixing them.
8. Record what remains unqualified, especially real model behavior, UI overlay alignment, six-axis physical interpretation, and private-media cases not supplied for testing.

"Bounding boxes solved" requires both geometric/consumer correctness and evidence on representative real media. Passing synthetic tests alone establishes only the cases they exercise. A model-quality issue must not be hidden by clamping coordinates or turning up thresholds.

No new detector architecture, precision gain, six-axis ground truth, or realtime promise is assumed merely to close M1. If a defect spans multiple milestones, record the remaining obligation rather than expanding M1 indefinitely or calling it complete prematurely.

## Debugging and correctness gates

| Gate | Required record |
| --- | --- |
| Baseline | Exact commit, branch/worktree, build configuration, relevant existing failures, and source paths actually exercised. |
| Reproduction | Small deterministic failing case or captured original artifact with an explicit expected result. If unavailable, label the suspected defect unconfirmed. |
| Regression | Test reaches the same production seam; it fails on the baseline and passes with the fix where a controlled comparison is feasible. |
| Consumer integration | Changed behavior reaches the GUI/CLI/batch/live consumer within scope; unused helpers do not establish repair. |
| Broader correctness | Relevant suite results and major pass findings, with failures classified rather than omitted. |
| External review | Every new commit reviewed using `review_commit` with automatic reviewer selection; real findings fixed and re-reviewed, false positives recorded. |
| Evidence | Commands, exit status, fixtures, tested configurations, limitations, and immutable commit references recorded without private source media or credentials. |
| Closure | Every milestone exit condition satisfied or explicitly still open. "Partial", "blocked", and "qualified" are not interchangeable. |

Real device movement is not a default test action. Physical playback validation requires the appropriate device setup and explicit operator control. Offline generation and non-actuating simulations should be used for initial correctness work.

## Data and model work

The initial labeling/review pilot has a five-human-hour budget total. It is an annotation-method trial with limited coverage, not five hours of video, a complete training dataset, or release qualification.

Benchmark pretrained candidates first. Targeted fine-tuning may address measured gaps, using separate permitted training data. Keep tuning and held-out qualification sources separate and preserve source/protocol identities.

Model choice, quantization, inference backend, and final memory budgets remain evidence-driven decisions. This roadmap does not authorize paid cloud training, uploading private media, or unbounded GPU workloads.

## Performance and qualification boundaries

- Fast stroke-only execution must exceed 1x realtime on every qualified CPU/GPU path. Reference GPU Fast retains its stronger >=10x floor.
- CPU Default stroke-only retains >=0.5x; reference GPU Default retains >=5x. Reference workload is 30-minute 1080p/30-fps video, measured end-to-end.
- Offline six-axis generation and Maximum may be slower. GPU fastest-preset live six-axis remains required; CPU six-axis live has the explicit best-effort exception.
- Responsive live target is 200 ms capture-to-command; optional buffered mode adds up to one second of playback delay. Physical response, synchronization error, and throughput are separate measurements.
- Material-error thresholds remain >100 ms reversal deviation and >20% reference stroke-travel error. Flag recall and pre-edit stroke accuracy are separate gates.
- Other input/hardware envelopes, Fast/Maximum quality floors, creative scoring, and secondary-axis metrics must be specified and qualified; unspecified metrics are not automatically passed.

## Architecture-to-milestone crosswalk

These contracts strengthen the existing M1-M8 acceptance scope; they do not change workstream purposes, relax numerical gates, or reduce platform, modality, offline, or device requirements. The M1-M8 mapping identifies responsibility and required evidence, not chronological precedence; the architecture-first phases above govern execution order. The software specification remains authoritative for product requirements. The architecture record supplies the detailed contract and decision provenance.

| Architecture area | Milestone mapping | Required acceptance obligation |
| --- | --- | --- |
| 1. Modules and modern names | M1 bounded seams; M2 shared architecture; M6 packaging | Introduce `pulsar-core`, `pulsar-protocol`, `pulsar-engine`, and `pulsar-clients` behind one executable's client/engine/worker roles. Core forbids unsafe code and GUI/runtime dependencies. GUI and CLI share engine APIs. Rename `FunGenApp` to `PulsarDesktop` while preserving attribution and completing the lineage audit; a rename is not evidence of clean-room origin. |
| 2. Domain identities and units | M1 geometry/time association; M2 project identities; M4 multimodal/six-axis contracts | Distinguish source versions/placements, frames, tracks, targets, outputs, revisions, jobs, attempts, and physical devices. Use rational source time, checked integer-nanosecond project time, and separate monotonic deadlines. Identify coordinate spaces and observation status explicitly. First release has one neutral program per project with up to six axes. |
| 3. Viewer and tracking | M1 actual observation rendering; M2 preview lifecycle; M4 projection coverage | Consume real observations only when source, frame, transforms, and seek/request generation match. Request lightweight analysis when absent; hide stale boxes and label user-defined regions separately. Test positive and absent detections, seeks, cuts, resizing, projections, and late delivery. Client previews remain independent of device playback. |
| 4. Projects and persistence | M2 authority and durability; M6 portable snapshots/migrations | Bundle SQLite. Only the engine commits authorized, revision-checked, protected-region-aware transactions. Gestures commit atomically; actor-labelled undo/redo creates fresh revisions. Reject stale commits and require explicit rebase. Snapshot sources immutably using copy-on-write or copies; test recovery, consistent exports, and non-destructive migrations. |
| 5. Engine, jobs, and resources | M2 lifecycle; M3 execution budgets; M5 live priority | One engine per OS user owns scheduling and devices. Jobs use immutable inputs; attempts identify exact dependencies. Resume only eligible validated checkpoints. Qualified automatic fallback creates a new attempt; explicit backend pins remain strict. Bound retries and aggregate CPU/RAM/VRAM/storage use; protect live/control deadlines and reject infeasible admission. |
| 6. Local API and authority | M2 protocol/grants; M5 controller lifecycle | Version and bound length-framed JSON over secured local sockets/named pipes; separate control and bulk transfers. Specify typed errors, idempotency, revision checks, reconnect/resynchronization, pairing, scoped grants, and revocation. Revocation cancels solely authorized work; disconnection alone does not. Shared buffers are immutable and authorization-scoped; no arbitrary shell, SQL, or ambient device access. |
| 7. Provenance, privacy, and disposal | M2 lineage/storage; M4 output association; M6 export behavior | Preserve compact lineage through opaque references; heavy traces are opt-in. Evict only eligible unpinned snapshots after active users release them, never committed motion, compact lineage, or original media as cache cleanup. Strip internal provenance from standard exports/device payloads while retaining local adaptation/export receipts. |
| 8. Plugins, models, and runtimes | M2 grant enforcement; M3 execution contracts; M6 platform confinement | Use bounded Wasmtime/WASM processing/editing extensions and isolated native runtime/driver workers with manifests, exact artifact identities, brokered effects, validated buffers/tensors, and host-call deadlines. Mark custom unqualified models/backends clearly. Unsandboxed exceptions require exact-build approval; assistant invocation requires individual approval by default, with a separately user-controlled, explicitly risky standing grant. Changed bytes or revocation invalidate that grant; the assistant cannot authorize exceptions itself. |
| 9. Device admission and qualification | M5 device/live evidence; M7 Preview classification; M8 Stable physical qualification | Separate neutral motion, profile adaptation, admission, encoding, and transport. Preserve timing while constraining amplitude; pin playback to an admitted revision and require controller authority/re-admission for changes. Accept official qualification records or explicit human acceptance after required local qualification. Headless use additionally requires a qualified stopping bound and per-configuration approval. Reconnect disarmed; a sent stop command is not proof of physical stopping. |
| 10. Assurance | M1 regressions; M2 critical models/kernels; M3-M6 affected-surface evidence; M7/M8 release gates | Use Kani for bounded critical Rust kernels and TLA+/TLC for lifecycle models, plus implementation-correspondence tests. Cover authority, revisions, revocation/cancellation, ownership, replay, arithmetic, and frame/evidence association. Combine KAT, property/metamorphic/differential, fuzz, mutation, fault-injection, real-media, and hardware evidence. Critical formal gates apply before Preview; fast PR gates do not replace deeper release qualification. |
| 11. Migration and compatibility | M1-M2 migration scope; M3-M6 adapters/packaging; M7-M8 compatibility qualification | Establish the final architecture in Phase A, moving supported behavior incrementally behind shared contracts rather than maintaining independent GUI/CLI/batch authority. Give transitional adapters explicit removal milestones. Preserve documented CLI syntax/aliases and non-destructive project imports through the first Stable major series; preserve standard funscript interoperability permanently, not historical bugs. Retain the agreed bundled offline and platform/vendor scope. |

## M1 acceptance addendum: reproduce, repair, and establish real consumers

M1 retains the baseline, bounding-box, tracking, and stroke-correctness scope, but no longer precedes architecture scaffolding. Phase A establishes the final four-library and shared-engine shape, fixing only migration-blocking correctness defects. The remaining M1 regression cases and acceptance obligations below stay open and carry into Phase B; Phase C supplies the corresponding hardening and qualification evidence. Do not discard historical defects, apply cached patches blindly, or mistake architecture migration for correctness closure.

- Re-establish the selected upstream baseline without overwriting the preserved checkout, local roadmap/specification work, or unrelated changes. Identify the source revision associated with every new result.
- Reproduce the 20 historically observed targeted regression failures against that selected baseline. The historical count and cached candidate patches are leads, not current acceptance evidence; disposition cases that no longer reproduce rather than applying old patches blindly.
- Repair demonstrated detector tensor/shape, finite-value, extent, class, clipping, and coordinate faults; tracker identity, aging, cuts, and box propagation; point-tracker forward/backward-flow behavior; and equal-area frame-dimension changes. Bind each correction to the relevant public or internal contract.
- Repair stroke extraction without inventing full-range movement or reversals. Preserve pauses, shallow relative amplitude, reversal timing, and plateau boundaries. Distinguish invalid input and missing evidence from a legitimate stationary result.
- Connect actual tracking observations to GUI rendering. A fixed analysis region or script-derived cursor must not impersonate a detected box. Exercise a moving target and no detections, then seek/cut/resize/projection changes and late results, with explicit source/frame/transform/request freshness checks.
- Record real-media detector/tracker behavior separately from synthetic/unit results. A moving synthetic box proves neither real detector quality nor physical device qualification. Do not close M1 solely because the existing test suite is green.

## Resolved architecture and remaining implementation evidence

Architecture choices are no longer an open-ended requirements interview. The selected module split, independent project timeline, engine authority, immutable job/source inputs, persistence approach, local protocol, plugin trust classes, and assurance tools are the implementation direction. Concrete schemas and code must realize those contracts; numerical, platform, model, and physical claims still require evidence.

| Item | Contract already selected | Remaining implementation or qualification work |
| --- | --- | --- |
| Models, runtimes, and per-vendor backends | Typed runtime adapters, exact artifact/dependency identity, qualified equivalent fallback, strict explicit pins, and visibly unqualified custom execution | Compare exact models/runtime builds in M3; demonstrate per-vendor support, memory use, throughput, and quality on the agreed M6 platform matrix. No particular model is qualified by this document. |
| Curated pilot and benchmark | Hand-curated/test-script benchmark direction and the five-hour initial annotation pilot remain binding | Acquire sources, annotations, statistical protocol, and any explicitly approved expanded data budget before tuning or release qualification. Preserve category-level gates and held-out evidence. |
| Box, flow, time, and consumer correctness | Typed coordinate/time identities, explicit observation status, and frame/transform/request freshness | Reproduce M1 faults and demonstrate corrected producer-to-consumer behavior; complete M2 domain/protocol schemas and conversion checks. Historical inspection is not a current pass. |
| Projects, jobs, recovery, and resources | Bundled SQLite, engine-only durable revision transactions, immutable snapshots, validated resumability, and bounded scheduling | Implement concrete schema/migrations, retention and recovery behavior, budget admission, checkpoint validation, and failure injection in M2/M3; prove consistent portable snapshots in M6. |
| Six-axis semantics and physical reference | One neutral program with typed axes/coordinate spaces, explicit transforms, and separate device-profile adaptation | Complete canonical axis conversion/convention evidence in M4; select the physical reference device and obtain M5/M8 qualification. Preserve the specification's Preview experimental versus Stable physical-qualification distinction. |
| Handy and other device integrations | Qualified admission, controller ownership, disarmed reconnect, explicit online/headless authorization, and human-owned local qualification acceptance | Demonstrate firmware/API/BLE coverage, stop bounds, external-player synchronization, and the approved device/configuration matrix in M5. Sending commands or passing transport tests alone is insufficient. |
| Plugin confinement and assistant exceptions | WASM/native role separation, brokered effects, exact-build unsandboxed approval, and individually approved assistant exception calls unless a user grants explicitly risky standing scope | Implement and test each platform's confinement, revocation, changed-artifact invalidation, host-call deadlines, and failure boundaries in M2/M3/M6. A separate process alone does not establish a security sandbox. |
| Assurance and release gates | Kani bounded kernels, TLA+/TLC lifecycle models, implementation correspondence, and layered KAT/property/fuzz/mutation/differential/fault-injection evidence | Define concrete harnesses and bounds, exercise the relevant implementations, disposition mutation survivors, and satisfy critical formal gates before M7 Preview. Neural quality, foreign runtimes, and physical safety remain separate evidence obligations. |
| Portable packages and support matrix | Bundled offline core/runtimes/models; agreed launch platform/vendor scope remains unchanged | Establish core size, peak memory, minimum OS versions, signing, extraction/update/rollback feasibility, and qualified execution in M3/M6. Escalate conflicts rather than silently weakening portability. |
| Other ARM64 and Intel Mac | Previously agreed priority and launch exclusions remain binding | Assign explicit later support milestones; do not silently expand or reduce launch scope. Platform qualification precedes a support claim. |

Escalate genuine conflicts with confirmed requirements or missing human-owned resources/decisions. Ordinary engineering detail must implement the settled contracts, not reopen the interview or silently substitute a weaker design.

### Consolidation status, 2026-09-11

This revision is documentation-only architecture-first sequencing consolidation under ARCH-041. It does not implement the four-library split, fix the bounding box, run or pass tests, establish hardware/model/platform qualification, or imply a commit. Architecture discussion is complete; implementation and acceptance evidence remain outstanding. The architecture-first execution plan changes delivery order, not the software specification's scope or the M1-M8 acceptance obligations.

## Completion and reporting

Each milestone report must state: implemented scope, reproduced/fixed defects, exact checks, external-review result, unresolved requirements, and next dependency. A blocked real-media or hardware gate stays visible even when code-level work passes.

Roadmap approval is not permission to erase repository history, change licenses, publish releases, push to a protected branch, or claim target accuracy/performance without evidence.

## Architecture Phase execution update (2026-09-11)

A1-A6 structural integration is complete for the enabled Linux architecture path;
Phase B starts with B1. See [Phase A completion record](PULSAR_PHASE_A_STATUS.md),
[migration/capability ledger](PULSAR_PHASE_A_MIGRATION.md) and
[implementation ADR](adr/0001-phase-a-boundaries.md) for exact evidence and limits.
This is not Preview/Stable, full feature parity, portable release qualification,
real-media accuracy or physical-device qualification. All M1-M8 product scope and
remaining B/C obligations are retained. No commit or upstream publication is implied.


## Good-phase implementation checkpoint: 2026-09-11

Phase B remains **IN PROGRESS**. See [PULSAR_PHASE_B_STATUS.md](PULSAR_PHASE_B_STATUS.md) for the B1-B6 delivered/open crosswalk, current-source gate receipt, real GUI/CLI smoke evidence, research limitations, and next engineering priority. This checkpoint does not mark any whole B milestone complete, start Solid phase, reduce specification scope, or substitute synthetic fixtures for real-media/human/hardware qualification.

Current increment: 209 Rust tests and architecture checks pass; 827 metric/artifact checks pass; the finite TLC lifecycle model and its negative mutation check pass. Kani was not run. GUI/CLI selected-axis export parity, fresh-revision undo/redo, and restored review visibility were exercised. Large program bulk transport, full inference/modalities/devices, portable project packaging, and human/real-media qualification remain open.


## Large-motion and startup checkpoint: 2026-09-11

See [PULSAR_PHASE_B_LARGE_MOTION_STATUS.md](PULSAR_PHASE_B_LARGE_MOTION_STATUS.md) and [ADR 0008](adr/0008-motion-artifacts-and-transfer-leases.md). Immutable motion transport, values-only edit proposals, durable request/replay fixes and bounded startup are implemented. Final local gate under a 1,024-descriptor limit passed 305 Rust tests plus bounded lifecycle/model mutation checks; final binary GUI reopen/export parity passed for a 180,001-action synthesized project. Source fingerprint: `8b12bd2343e9c10b16485f517bc8a28af33b0a2b323ce625bb45965bae05bc1f`.

This replaces the earlier unimplemented large-motion blocker status only. Good Phase B remains IN PROGRESS; no whole B milestone, Preview/Stable, real-media quality, reference-hardware speed, physical stopping or full memory/control-latency qualification is implied. [Portable-project work](PULSAR_PORTABLE_PROJECT_IMPLEMENTATION_PLAN.md) is the next proposed B6 slice, not an implemented feature. The software specification remains authoritative.
