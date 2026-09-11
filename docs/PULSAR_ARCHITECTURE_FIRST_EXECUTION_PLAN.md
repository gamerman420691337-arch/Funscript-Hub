# Pulsar Architecture-First Execution Plan

Version: 1.1
Date: 2026-09-11
Status: Architecture Phase A1-A6 structurally complete; Phase B next; release qualification pending
Decision source: ARCH-041, direct product-owner instruction on 2026-09-11

## 1. Authority and changed order

The [Pulsar Software Specification](PULSAR_SOFTWARE_SPECIFICATION.md) remains the product contract. The [architecture record](PULSAR_ARCHITECTURE_DECISIONS.md) defines the selected four-library architecture and ARCH-001 through ARCH-041. The [major roadmap](PULSAR_MAJOR_ROADMAP.md) retains all M1-M8 scope and acceptance obligations.

ARCH-041 supersedes the previous M1-before-architecture execution order:

1. **Phase A: make the architecture right.** Move production paths into the final ownership/dependency structure, scaffold every required interface family, and demonstrate real composed paths.
2. **Phase B: make it good.** Solve functional and technical challenges using explicit research protocols, implement evidence-supported choices, and close correctness defects.
3. **Phase C: make it solid.** Complete adversarial hardening, formal correspondence, platform/hardware qualification and product release gates.

M1-M8 are now scope and acceptance workstreams, not a mandatory chronological sequence. No historical regression, product requirement, compatibility promise or qualification gate disappears because delivery order changes.

"Final architecture" means the agreed dependency and authority structure is present and enforced for enabled production paths. It does not mean empty crates, frozen algorithm internals, complete model accuracy or qualified physical behavior. A later interface change requires an ADR explaining evidence and compatibility impact rather than an unrecorded exception.

Version 1.0 established the delivery sequence without implementation claims. Phase A has now been executed; its scoped results and limitations are recorded in [PULSAR_PHASE_A_STATUS.md](PULSAR_PHASE_A_STATUS.md). Remaining milestone descriptions are obligations, not completed work. No release, neural-quality or physical qualification is implied.

## 2. Difficulty bands

| Band | Difficulty | Typical work | Estimation rule |
| --- | --- | --- | --- |
| D1 | Mechanical / bounded | Modern names, module moves, composition roots, manifests and compatibility aliases. | Estimate after one source inventory; do not confuse many moved lines with research difficulty. |
| D2 | Deterministic correctness | Checked units/geometry, DSP invariants, frame association, codecs and pure transformations. | Bound by characterized behavior, explicit contracts and independent expected results. |
| D3 | Stateful / cross-platform assurance | Durable transactions, concurrency, authority, isolation, resource admission, recovery and packaging. | Split by failure domain and observable transition; include adversarial and platform evidence. |
| D4 | Research / externally constrained | Neural accuracy, calibrated review flags, six-axis inference, vendor performance, physical stopping and statistical qualification. | Use bounded experiments and hardware/data budgets; do not invent calendar certainty before feasibility evidence. |

The hardest work is not the crate layout. It is obtaining trustworthy motion and calibrated error review, meeting end-to-end performance on qualified hardware, and establishing bounded device behavior. Those challenges remain after structural integration.

## 3. Target structure and module ownership

The selected layout is now a workspace with four production libraries and one role-dispatch executable. The [migration ledger](PULSAR_PHASE_A_MIGRATION.md) distinguishes enabled paths from unsupported implementation and qualification gaps; this is not yet a portable release package.

```text
pulsar executable / composition root
  client role -> pulsar-clients
  engine role -> pulsar-engine
  worker role -> isolated adapters using protocol/core contracts

crates/pulsar-core
  domain identities, units, time, geometry and evidence kinds
  pure perception/tracking/DSP/motion kernels
  project/playback state transitions and invariant predicates

crates/pulsar-protocol
  command/query/event schemas and typed errors
  wire framing, compatibility and checked domain conversion
  worker/artifact/buffer interface contracts

crates/pulsar-engine
  per-user authority and lifecycle
  projects, transactions, revision history and provenance
  jobs, attempts, immutable artifacts and resource admission
  isolated media/inference/plugin/driver adapters and effect brokers
  device sessions, qualification admission and diagnostic orchestration

crates/pulsar-clients
  CLI
  PulsarDesktop GUI
  independent preview state and editing gestures
  approval/assistant presentation
```

Allowed dependency direction: clients and engine depend on protocol/core; protocol depends on core; core depends on neither clients nor engine, GUI, I/O or inference runtimes. Third-party/native extension code must not execute in the engine/GUI address space. Workers propose artifacts and cannot commit project state.

| Required part | Structural home | Architecture milestone | Functional/research milestone | Solid/qualification milestone |
| --- | --- | --- | --- | --- |
| CLI/orchestration and GUI | Clients over identical engine interfaces; executable dispatch only. | A1/A3/A6 | B6 | C1/C3 |
| Media input/parser | Core contracts plus isolated engine-managed import/decode adapters. | A2/A4/A5 | B2/B4 | C1/C2 |
| Media viewer | Core association predicates, engine preview analysis, client viewport/playhead. | A2/A4 | B2/B6 | C1/C3 |
| Media editor | Core edit algebra, engine transactions/history, client gesture previews. | A2/A3/A6 | B6 | C1/C3 |
| General DSP | Core pure kernels with explicit units and provenance. | A2/A4 | B2/B4 | C1/C3 |
| Optical flow / Lucas-Kanade / tracking | Core state and kernels; budgeted worker execution. | A2/A4/A5 | B2 | C1/C3 |
| Neural runtime and model parsers | Protocol tensor contracts and isolated native worker adapters. | A4/A5 | B3 | C1/C2 |
| Model zoo and execution profiles | Engine-managed artifact identities, manifests and qualification records. | A2/A5 | B3/B4 | C2/C3 |
| Local assistant | Client proposals; engine capability/approval broker and resource limits. | A3/A5/A6 | B3/B6 | C1/C3 |
| Device profiles, drivers and playback | Core adaptation/admission predicates and engine-owned sessions/brokers. | A2/A5/A6 | B5 | C1/C2/C3 |
| Hardware validation doctor | Explicit configuration/evidence interfaces; read-only probes by default. | A5/A6 | B3/B5 | C2/C3 |
| Internal assurance suite | Kernel/interface harnesses, lifecycle models and correspondence fixtures. | A1-A6 | B1-B6 | C1-C3 |
| Portable artifacts and lineage | Engine artifact interfaces; build/distribution manifests and provenance audit. | A1/A5 | B3/B6 | C2/C3 |

## 4. Phase A: architecture first

Phase A performs real integration, not algorithm optimization. Existing behavioral defects are tracked explicitly; extraction does not certify them acceptable. Data loss, authority bypass, unsafe physical effects and migration-created regressions cannot be deferred behind an "architecture only" label. Disable an unqualified/unsafe path if its minimum guarantees cannot yet be supplied.

### A1. Workspace, ownership and migration inventory

Difficulty: D1 with D3 integration exposure.
Dependencies: none beyond establishing the selected current baseline without overwriting unrelated work.

Deliverables:

- Map existing production entry points, modules, effects and duplicated paths to the four target libraries and process roles.
- Establish the workspace/composition layout and `PulsarDesktop` naming, preserving attribution and documented CLI aliases.
- Record source/model lineage questions without claiming clean-room origin or changing licenses.
- Add architecture dependency rules and a migration inventory identifying each transitional adapter, owner, allowed behavior and removal milestone.
- Retain existing reproduced defects as baseline observations, not extraction regressions or silently accepted behavior.

Exit evidence: each enabled production path has an owner; dependency enforcement exists; compatibility entry points still resolve; no renamed symbol or new crate is counted as functional improvement by itself.

Existing workstreams: M1 baseline/lineage, M2 shared structure, M6 compatibility.

### A2. Checked domain and interface vocabulary

Difficulty: D2.
Dependencies: A1 ownership map.

Deliverables:

- Distinct source version/placement/frame, entity/target/output, revision/job/attempt/artifact, grant/session and physical-device identities.
- Rational source timestamps, checked project nanoseconds, separate monotonic deadlines and explicit coordinate-space transforms.
- Checked neutral-axis values, unavailable/predicted/inferred/synthesized evidence, compact provenance references and protected-region semantics.
- Pure state transitions for edits, candidates, attempts and playback admission; no I/O or runtime initialization inside core.
- Explicit interface inventories for media, inference, tracking, motion, persistence, jobs, exports, authority and devices.

Exit evidence: enabled callers cross checked interfaces; invalid values fail explicitly; missing observations/axes cannot silently become observed motion/zero values; core forbids unsafe code and forbidden dependencies.

Existing workstreams: M1 geometry/time, M2 domain, M4 axes.

### A3. One authoritative engine and one real vertical path

Difficulty: D3.
Dependencies: A1/A2.

Deliverables:

- Per-user engine discovery/instance exclusion and secured versioned local transport.
- Paired/scoped clients, typed commands/queries/events/errors, expected revisions and request identities.
- Bundled SQLite project transactions, durable acknowledgement, actor-labelled undo/redo and protected-edit enforcement.
- Minimal real path shared by CLI and GUI: import an immutable input, start work, receive a revision-bound candidate, commit through the engine and export neutral motion.
- Cancellation/revocation/reconnect semantics with explicit ordering; no worker or client direct project writes.

Exit evidence: both clients exercise the same engine operation; accepted edits have durable recorded outcomes; stale/unauthorized commits reject; the vertical path is not a fake-success stub. Advanced model quality is not an A3 gate.

Existing workstreams: M2 projects/API/client parity.

### A4. Media, perception, DSP and viewer integration

Difficulty: D2/D3.
Dependencies: A2 and the A3 vertical path.

Deliverables:

- Route existing import/decode, detector/flow/tracker, motion-processing and export implementations through their assigned interfaces.
- Replace duplicated orchestration with one shared execution path; preserve behavior provenance while identifying known algorithm defects.
- Carry source/frame/transform/request identities to client observation rendering, with explicit unavailable and prediction states.
- Separate preview playheads, transient edit gestures and engine-owned physical playback.
- Scaffold all required modality and six-axis interface families; unsupported implementations report unsupported and remain visible delivery gaps.

Exit evidence: actual observations can reach the viewer through the intended path; seeks/cuts/transforms have explicit invalidation contracts; no production caller bypasses the shared path merely to retain an old implementation. Detector quality and full bug closure remain B work.

Existing workstreams: M1 consumer paths, M2 orchestration, M4 modality interfaces.

### A5. Workers, artifacts, extension roles and resources

Difficulty: D3.
Dependencies: A2/A3; integrate with A4.

Deliverables:

- Isolated worker roles with immutable manifests, distinct attempts, bounded messages/buffers and atomic artifact/checkpoint publication.
- Snapshot/pinning/eviction interfaces preserving active dependencies, original media, committed motion and compact lineage.
- Runtime/model/plugin/driver role manifests and capability brokers, with no engine/GUI in-process extension loading.
- Aggregate CPU/RAM/VRAM/storage admission, bounded retries and separate host-call deadlines.
- Explicit interfaces for qualified fallback, strict pins, custom unqualified runs and interrupted-job recovery.
- Assistant action proposals use the same scoped engine interfaces, not shell access or direct GUI/project-memory mutation.

Exit evidence: a real worker path obeys budgets and authority; malformed/late responses fail closed; unsupported confinement or backend paths stay disabled unless the selected exact-build human exception applies. Empty scheduler or sandbox interfaces are insufficient.

Existing workstreams: M2 jobs/artifacts, M3 runtime/resources, M5 driver workers, M6 packaging prerequisites.

### A6. Whole-architecture closure

Difficulty: D3.
Dependencies: A1-A5.

Deliverables:

- Complete the interface inventory and ownership graph for every required logical module.
- Route all enabled GUI/CLI/batch/project/device effects through the designated engine authority and workers.
- Install device admission/controller/session and assistant approval interfaces without enabling unqualified physical capabilities.
- Remove duplicate production authority paths. Keep only explicitly scoped compatibility/implementation adapters with removal milestones.
- Establish lifecycle models, correspondence-test entry points and invariant harnesses alongside their implementations.
- Publish a capability ledger separating implemented, unsupported, unqualified and disabled capabilities.

Exit evidence: the architecture gate below is met. A6 is structural integration, not Preview readiness.

Existing workstreams: M2 integration plus structural prerequisites across M1-M6.

### Phase A exit gate

All required criteria must have subject-bound evidence when Phase A is executed:

1. Four-library dependency direction and core restrictions are enforced, not merely documented.
2. Every enabled production path has a module owner and uses its designated interface; no client/worker direct project commits or ambient device effects remain.
3. GUI and CLI complete the same real import -> worker -> candidate -> durable commit -> neutral export path.
4. Checked identity, time, coordinate, revision and evidence types cross that path without silent semantic conversion.
5. Authentication, expected revisions, revocation/cancellation fencing and durable outcomes are integrated for enabled operations.
6. Workers and bulk artifacts are isolated, bounded and ownership-scoped; unsupported enforcement is explicit.
7. Viewer observations are associated with displayed context; unavailable evidence is not represented by a decorative tracking box.
8. Device and assistant authority interfaces are present; unqualified or unsupported effects cannot be activated accidentally.
9. Remaining adapters have named owners, bounded responsibilities and removal milestones; duplicated authoritative behavior is gone.
10. Known defects and unimplemented capabilities remain in the ledger with later milestones. Structural integration does not mark them resolved.

## 5. Phase B: make it good through evidence-led research

All B work starts after A6. Independent B tracks may run concurrently once their prerequisites and resource/data budgets are established. Deterministic bugs that invalidate a research baseline must be corrected before trusting measurements; there is no reason to conduct deep research on an already demonstrated divide-by-zero.

### B1. Research baseline, defect map and evaluation protocol

Difficulty: D2/D4.
Dependencies: A6.

Deliverables:

- Reproduce the historical 20 regression cases against the migrated, selected baseline; record fixed, still failing, changed and no-longer-applicable cases separately.
- Map major flaws and security hypotheses to interfaces, violated invariants, triggers, reachability, consequences and evidence.
- Run the authorized five-human-hour annotation/review pilot when explicitly executing that work; preserve independent expected results and record achieved coverage.
- Define held-out splits, category taxonomy, uncertainty reporting and proposed statistical release rules; request human decisions for unapproved budgets or product thresholds.
- Freeze a useful baseline for each research question and create its experiment protocol before tuning.

Exit evidence: reproducible defect/benchmark inventory, independent oracles and explicit unresolved acceptance definitions. A five-hour pilot is not release qualification.

Existing workstreams: M1 regression evidence, M3 benchmark foundation, M7/M8 evaluation design.

### B2. Stroke, perception, tracking and truthful review flags

Difficulty: D4 research plus D2 correctness.
Dependencies: B1.

Deliverables:

- Correct detector shape/coordinate validation, clipping, track identity/aging/cuts, box propagation and forward/backward-flow handling.
- Preserve constant input, shallow motion, direction, reversal timing, pauses and plateaus; remove invented full-range movement.
- Distinguish camera/target motion, occlusion, missing evidence and genuine synthesis.
- Investigate tracking/fusion/uncertainty alternatives using controlled comparisons and independent references.
- Connect confidence/review behavior to materially incorrect sections and actual human review burden, not only score thresholds.
- Exercise moving targets, no detections, seeks, cuts, resizing, projections and late results in the real GUI path.

Exit evidence: M1 correctness cases resolved with independent regression coverage; real-media results separately reported from synthetic fixtures; quality and review-flag claims supported on their declared evaluation subjects. Whole-release accuracy remains a C3 gate.

Existing workstreams: M1, M3 quality, M4 projection/evidence semantics.

### B3. Local inference, model zoo and performance

Difficulty: D4.
Dependencies: B1 and stable B2 measurement interfaces; B2/B3 may iterate together.

Deliverables:

- Compare model weights, preprocessing/postprocessing, quantization, runtimes and per-vendor execution profiles.
- Measure end-to-end CPU/GPU throughput, peak RAM/VRAM, preparation/extraction effects and cold/warm behavior.
- Support the bundled offline assistant and generation without compromising live/control reserves.
- Qualify equivalent fallback within the requested tier; retain strict backend pins and prominent custom-run status.
- Audit model/runtime redistribution and artifact provenance before packaging a selected candidate.

Exit evidence: selected profiles have reproducible quality/speed/memory evidence on identified configurations. Unsupported hardware remains explicitly unqualified, not silently CPU-backed while claiming acceleration.

Existing workstreams: M3 inference/performance, M6 vendor feasibility.

### B4. All modalities, synthesis and six axes

Difficulty: D4.
Dependencies: B1 plus usable B2/B3 profiles.

Deliverables:

- Complete required video/projection, audio, text, still-image-plus-prompt, mixed and live-input interface implementations according to the specification.
- Keep faithful reconstruction and creative synthesis separate, including evaluation methods.
- Complete the neutral six-axis program while exposing unobservable/inferred motion honestly.
- Preserve the stroke-only scope of offline speed floors; preserve the required GPU fastest-preset live six-axis capability.
- Establish proposed and approved quality conventions for creative, Fast/Core, Maximum and secondary-axis outputs without inventing prior user approvals.

Exit evidence: each required input/output family has real functional evidence, provenance and a declared evaluation method. Missing modalities block their required release scope.

Existing workstreams: M4, M3 model coverage, M5 live-input prerequisites.

### B5. Live scheduling, profile adaptation and device integration

Difficulty: D4 with high-consequence D3 state.
Dependencies: B2/B3 and the relevant B4 live-output path.

Deliverables:

- Integrate neutral -> adaptation -> admission -> encoding -> transport, preserving timing and bounding amplitude.
- Implement required Handy/Handy 2 online/offline modes and the selected device qualification flow.
- Exercise controller transfer/loss, revision switching, command age, reconnect, stop/disarm and unknown physical state.
- Measure capture-to-command latency separately from physical response; investigate sustained backlog and non-preemptible workloads.
- Establish configuration-bound qualification and stopping evidence through explicitly authorized physical work; human acceptance remains separate.

Exit evidence: device capabilities have exact driver/device/firmware/transport evidence, no implicit rearm, and separately approved headless eligibility. A mocked stop does not qualify a physical device.

Existing workstreams: M5; M4 live six-axis integration.

### B6. Product integration and functional defect closure

Difficulty: D3.
Dependencies: B2-B5 as applicable; independent editor/assistant usability work may proceed after B1.

Deliverables:

- Complete editor gestures, protected regions, actor-labelled history, candidate review/rebase, project packaging and diagnostics.
- Complete the bounded local assistant and plugin workflows, including per-invocation native approval and the separately risky standing mode.
- Remove phase-B transitional implementations when their replacement gates pass.
- Close remaining functional defects across GUI/CLI parity, state, worker/resource behavior and user-facing error recovery.
- Re-run affected comparisons when integration changes model inputs, transformations, scheduling or output behavior.

Exit evidence: required functional scenarios succeed through production interfaces; remaining defects are classified and cannot be hidden by a successful demo. Phase B functional demonstration does not certify adversarial robustness or release readiness.

Existing workstreams: M1-M5 functional completion, M6 export/update preparation.

## 6. Phase C: make it solid

Assurance harnesses begin in Phase A; Phase C completes their adversarial depth and release qualification. Deferring all tests, security design or proof correspondence until Phase C is explicitly not this plan.

### C1. Correctness, security and adversarial hardening

Difficulty: D3.
Dependencies: relevant B implementations.

Deliverables:

- Complete KAT, property/metamorphic/differential, parser/protocol/manifest fuzzing and mutation campaigns.
- Exercise crash/disk-full/OOM/timeout/revocation/reconnect/duplicate-request and artifact-tampering failures.
- Complete bounded Kani obligations and TLA+/TLC lifecycle models with implementation-correspondence evidence.
- Audit native/assistant/plugin effects, buffer isolation, dependency identity and stale authority across actual platform implementations.
- Resolve release-blocking defects; bind residual limitations to explicit assumptions and capability exclusions without silently reducing promised release scope.

Exit evidence: critical declared invariants have required proof/test evidence; adversarial findings are resolved or remain explicit blockers. Model checking does not establish neural accuracy, foreign-runtime correctness or physical stopping.

Existing workstreams: M2-M6 hardening; mandatory M7 formal prerequisites.

### C2. Portable release and platform/hardware qualification

Difficulty: D3/D4.
Dependencies: B3/B5/B6 and applicable C1 gates.

Deliverables:

- Qualify offline Core/complete packages, verified first-use extraction, interrupted update/rollback and consistent project migration/export.
- Exercise clean supported machines without user-installed developer/inference tools or mandatory first-run model downloads.
- Qualify required OS/architecture/GPU-vendor configurations and document actual OS/driver prerequisites.
- Complete signing/distribution and source/model/runtime attribution/redistribution evidence.
- Bind device records and approvals to exact configurations; requalify changed artifacts and firmware where required.

Exit evidence: candidate-bound platform matrix and physical records, not compilation/provider-detection claims. Unsupported release promises remain blockers for that required release scope.

Existing workstreams: M5 physical qualification, M6 portability.

### C3. Preview, Stable and comparative qualification

Difficulty: D4.
Dependencies: C1/C2 and the applicable product gates.

Deliverables:

- Close the M7 Preview gate using the specification's quality/performance scope plus critical formal obligations.
- Close the M8 Stable gate using independent held-out evaluation, per-category human-effort evidence and required physical qualification.
- Keep flags' material-error recall distinct from generation precision/recall and review burden.
- Report Fast/Core, Default, Maximum, creative and secondary-axis evidence separately according to their approved criteria.
- Make SOTA claims only from dated, reproducible comparisons with disclosed inputs, hardware and metrics.

Exit evidence: required candidate-bound release report, no unresolved release-blocking defects and no missing required qualification. Preview success does not automatically establish Stable or comparative superiority.

Existing workstreams: M7/M8.

## 7. Research and ADR discipline

Research follows Phase A and uses stable measurement interfaces. Research may refine internals freely; evidence-driven changes to shared contracts require an explicit ADR and compatibility review.

Every D4 investigation follows this loop:

1. Define the research question, falsifiable hypothesis and contract it must satisfy.
2. Review primary papers, official documentation and relevant source implementations; record versions, limitations and provenance.
3. Freeze a baseline, independent oracle, dataset split and candidate artifact identities.
4. Predeclare metrics, tolerances, resource/time/data budgets and stopping conditions before comparing candidates.
5. Run bounded local experiments; preserve raw/derived results, scripts, seeds, exact dependencies and failure cases.
6. Compare quality, throughput, memory, human effort and portability jointly; reject improvements that silently violate another confirmed requirement.
7. Write the decision ADR with evidence links, rejected alternatives, tradeoffs and any unresolved assumptions.
8. Implement the chosen result, add regression coverage, and repeat affected evaluation through production interfaces.

Negative results and "no candidate met the contract" are valid scientific outcomes, not permission to lower the contract. External compute spending, training budgets, private-data disclosure and physical actuation require their own authorization.

ADRs record decisions and reference the science. Experiment records retain protocols and results; an accepted ADR is not a passed experiment or a qualification receipt. Maintain both so negative findings and reproduction details survive later summaries.

Recommended durable records, to be created with the corresponding work:

| Record | Required content |
| --- | --- |
| Decision ADR | Status, decision source, question, constraints, alternatives, chosen option, evidence references, consequences, interface impact, validation obligations and supersession. |
| Experiment protocol | Hypothesis, baseline/candidates, exact artifacts, data split, independent oracle, metrics/tolerances, budget and stop conditions. |
| Experiment result | Environment, source/build identities, commands, seeds, measurements, uncertainty, negative results, artifacts and reproduction instructions. |
| Qualification receipt | Exact subject/configuration, required gate, tool/protocol version, result, limitations, invalidation rules and required human acceptance. |
| Defect record | Requirement/interface, invariant, trigger, reachability, impact, evidence status, fix milestone, regression case and closure evidence. |

Document directories and record naming must use the repository's existing conventions when implementation begins; this planning step does not invent completed ADRs, experiments or acceptance.

## 8. Bugs are tracked throughout, not postponed blindly

Use explicit defect evidence states:

- Historically reproduced: demonstrated against an older recorded baseline.
- Currently reproduced: demonstrated against the identified current implementation.
- Source-supported: code reasoning establishes the behavior under stated preconditions.
- Hypothesis: investigation required; not a confirmed vulnerability.
- Specification/acceptance gap: behavior or required evidence lacks a settled measurable definition.
- Qualified closed: fix and required regression/integration evidence match the actual subject.

Phase A fixes migration blockers, data-loss/authority hazards and unsafe enabled effects; otherwise disable the path and retain the blocker. Phase B fixes deterministic defects that contaminate research before measurements are trusted and closes functional defects alongside research. Phase C closes adversarial, release and qualification defects.

The previously observed 20 cases remain an explicit M1 inventory: eight detector-decoding cases, four tracker cases, three point-tracker cases, four stroke-processing cases and one flow-dimension case. The stationary GUI region/observation issue is additional historical source evidence. These are not fresh failure counts for the migrated/current checkout.

## 9. Contract-to-execution crosswalk

| Architecture contract | Architecture placement | Good/function/research | Solid/qualification |
| --- | --- | --- | --- |
| CONTRACT-01 Modules and modern names | A1/A6 | B6 adapter retirement | C1 dependency/compatibility audit |
| CONTRACT-02 Identities, units and evidence | A2/A4 | B2/B4 algorithms | C1 arithmetic/association evidence |
| CONTRACT-03 Viewer and tracking | A4 | B2/B6 | C1/C3 real-media/UI evidence |
| CONTRACT-04 Projects and persistence | A3/A5 | B6 | C1/C2 crash/migration/export evidence |
| CONTRACT-05 Engine, jobs and resources | A3/A5 | B3/B5/B6 | C1/C2 stress/recovery qualification |
| CONTRACT-06 Local interface and authority | A3/A5/A6 | B6 | C1 protocol/revocation/replay campaigns |
| CONTRACT-07 Provenance, privacy and disposal | A2/A5 | B1/B3/B6 | C1/C2 retention/export/lineage evidence |
| CONTRACT-08 Plugins, models and runtimes | A5/A6 | B3/B4/B6 | C1/C2 confinement/vendor/distribution evidence |
| CONTRACT-09 Device admission and qualification | A2/A5/A6 | B5 | C1/C2/C3 physical/release evidence |
| CONTRACT-10 Assurance | A1-A6 harnesses/models | B1-B6 independent functional evidence | C1-C3 required deep/formal/release gates |
| CONTRACT-11 Migration and compatibility | A1/A4/A6 | B6 | C2/C3 public compatibility/release gates |

## 10. Status vocabulary and next executable milestone

Use **structurally integrated -> functionally demonstrated -> qualified**. These states are not interchangeable and are recorded per capability/configuration.

Immediate next milestone: **B1, research baseline, defect map and evaluation protocol**. A1-A6 structural integration has passed the scoped local gates in [PULSAR_PHASE_A_STATUS.md](PULSAR_PHASE_A_STATUS.md). Historical regression reproduction, functional completion and deep research now proceed on the shared architecture; none is silently counted as already complete.

This document changes execution order, not product scope. Architecture discussion and Phase A structural integration are complete. Good-phase functionality/research, full bug closure, adversarial hardening and release qualification remain unfinished.


## Good-phase implementation checkpoint: 2026-09-11

Phase B remains **IN PROGRESS**. See [PULSAR_PHASE_B_STATUS.md](PULSAR_PHASE_B_STATUS.md) for the B1-B6 delivered/open crosswalk, current-source gate receipt, real GUI/CLI smoke evidence, research limitations, and next engineering priority. This checkpoint does not mark any whole B milestone complete, start Solid phase, reduce specification scope, or substitute synthetic fixtures for real-media/human/hardware qualification.

Current increment: 209 Rust tests and architecture checks pass; 827 metric/artifact checks pass; the finite TLC lifecycle model and its negative mutation check pass. Kani was not run. GUI/CLI selected-axis export parity, fresh-revision undo/redo, and restored review visibility were exercised. Large program bulk transport, full inference/modalities/devices, portable project packaging, and human/real-media qualification remain open.


## Large-motion and startup checkpoint: 2026-09-11

See [PULSAR_PHASE_B_LARGE_MOTION_STATUS.md](PULSAR_PHASE_B_LARGE_MOTION_STATUS.md) and [ADR 0008](adr/0008-motion-artifacts-and-transfer-leases.md). Immutable motion transport, values-only edit proposals, durable request/replay fixes and bounded startup are implemented. Final local gate under a 1,024-descriptor limit passed 305 Rust tests plus bounded lifecycle/model mutation checks; final binary GUI reopen/export parity passed for a 180,001-action synthesized project. Source fingerprint: `8b12bd2343e9c10b16485f517bc8a28af33b0a2b323ce625bb45965bae05bc1f`.

This replaces the earlier unimplemented large-motion blocker status only. Good Phase B remains IN PROGRESS; no whole B milestone, Preview/Stable, real-media quality, reference-hardware speed, physical stopping or full memory/control-latency qualification is implied. [Portable-project work](PULSAR_PORTABLE_PROJECT_IMPLEMENTATION_PLAN.md) is the next proposed B6 slice, not an implemented feature. The software specification remains authoritative.
