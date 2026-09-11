# Pulsar Good Phase: implementation checkpoint

Date: 2026-09-11
Status: **IN PROGRESS. Phase B is not complete.**

> Later checkpoint: [large-motion and startup status](PULSAR_PHASE_B_LARGE_MOTION_STATUS.md). The evidence and open transport blocker below are historical; the later checkpoint records the implemented replacement and remaining phase qualification.
Authority: [software specification](PULSAR_SOFTWARE_SPECIFICATION.md), [architecture-first execution plan](PULSAR_ARCHITECTURE_FIRST_EXECUTION_PLAN.md), and [major roadmap](PULSAR_MAJOR_ROADMAP.md).

This checkpoint records implemented and tested work, not Preview, Stable, performance, model-accuracy, platform, or physical-device qualification. No completion keyword, commit, physical actuation, private-media upload, or paid/cloud experiment is implied. The specification and all existing B1-B6 exit criteria remain unchanged.

## 1. Milestone state

| Milestone | Delivered in this increment | Required work still open |
| --- | --- | --- |
| B1: research baseline and defect map | Reproducible metric/artifact harnesses, independent synthetic oracles, held-out protocol artifacts, defect inventory, historical-20 source inventory and current-contract correspondence. | Permitted real corpus and curated references, the five-human-hour annotation/review pilot, category coverage and unresolved statistical/acceptance decisions. Historical correspondence is not execution of the original legacy test suite. |
| B2: perception, stroke and review flags | Checked detector decoding, clipping/NMS, stable target identity, independent forward/backward flow, conservative target/background relative motion, explicit gaps and evidence labels, synthetic regression/mutation checks. | Real-media detector/tracker comparisons, projection and occlusion behavior, calibrated material-error review recall and measured human burden. A synthetic moving box does not qualify a real detector. |
| B3: local inference and performance | Runtime/provider/device research and identified candidate constraints; existing isolated CPU inference adapter remains available only as unqualified configuration. | Licensed bundled model/runtime selection, real offline assistant inference, actual per-vendor execution and equivalent fallback qualification, reference CPU/GPU end-to-end throughput and memory/cold/warm measurements. No GPU performance claim. |
| B4: modalities and six axes | Checked deterministic text/pattern generation, still-image-plus-prompt pattern path, audio envelope/onset synthesis, six-axis neutral pattern outputs and selected-axis export. | Semantic text/image/audio models; full stereo, VR180/360, mixed and live implementations; observed/inferred six-axis reconstruction and declared evaluations. Current image bytes are decoded and bound but do not semantically condition motion. |
| B5: live and devices | Research/qualification constraints and existing fail-closed authority scaffolding retained. | Real Handy/Handy 2 online and offline BLE drivers; live scheduling/adaptation integration; configuration-bound stop, reconnect, controller transfer, command-age and latency evidence; separately authorized hardware work and human acceptance. |
| B6: product integration | Revision-bound funscript import/candidates, protected selective merge, review-preserving rebase/history, durable compact lineage, automatic GUI diagnostics, source-kind preview guards, CLI/GUI synthesis and selected-axis export parity. | Large motion artifact transport; consistent portable project packages; explicit human review resolution; bounded real local assistant/plugin execution; remaining functional adapters and failure-recovery scenarios. |

**No B milestone is promoted to whole-milestone completion by this checkpoint.** Unavailable external evidence and unfinished engineering are different categories; neither is a pass.

## 2. Implemented contracts and corrections

- Detector contracts identify output shape, class semantics and coordinate space; invalid or non-finite geometry cannot silently become a tracking box.
- Tracking maintains entity identity across intentional frame sampling. Skipped frames do not fabricate absence observations. Forward/backward flow is independently evaluated, and target/background support is distinguished.
- Stroke generation preserves actual supported displacement instead of inventing full-range movement. Missing targets, scene cuts and inadequate support remain explicit gaps/reviews. Initial anchors and creative outputs are labelled synthesized.
- Text generation uses a strict deterministic pattern grammar, not an LLM. All six neutral axes are supported by that pattern path. No claim of semantic understanding or measured creative quality is made.
- Still-image input requires a prompt, is decoded, and participates in the immutable dependency binding. Its current synthesis remains prompt-pattern based; this limitation is disclosed in review flags.
- Audio RMS envelopes and threshold-onset pulses retain an explicit source-clock receipt. The UI's Beats mode currently means onset pulses, not qualified musical beat detection. Discontinuous or too-coarse decoded clocks fail explicitly instead of collapsing gaps.
- Funscript import produces an immutable, revision-bound candidate. Selective merge respects protected anchors and retains conservative interpolation-influence review coverage, including ramps adjacent to an inserted singleton.
- Engine commits retain revision lineage and unresolved review snapshots. Undo/redo create fresh revisions and record explicit restoration ancestry; they do not infer ancestry from program equality. Review overflow rejects the transaction rather than dropping flags.
- Legacy lineage that cannot be established stays explicitly unknown. Review flags are not implicitly resolved by commit, merge, export or redo.
- Source kinds are engine-owned. Generation-input and funscript sources are not offered to the video preview. Legacy wire defaults and trusted stored-job migration remain explicit.
- GUI diagnostics refresh after accepted project snapshots and reject mismatched project/revision/candidate reports. Sidebar warnings lead to read-only diagnostics.
- Selected-axis export checks the expected revision, preserves neutral timing, strips internal provenance from standard funscript bytes, and retains a local export receipt.

Relevant decision records: [perception/tracking](adr/0002-perception-tracking.md), [runtime/device research](adr/0004-runtime-device-research.md), [audio source clock](adr/0005-audio-source-clock-admission.md), [revision/review lineage](adr/0006-revision-review-lineage.md), and [proposed large-motion artifacts](adr/0007-large-motion-artifacts.md).

## 3. Current tested subject and evidence

Gate source fingerprint:
`350ae1e1f8e02cc2ddb42b2764b369dea125b8a0259cc75319ede20d841257cd`.

Linux debug GUI/CLI binary SHA-256:
`cf115884e571c0ecc0cc2ab4ada5c2b864e47c2db0d1543fa17554b9b7bf23d8`.

| Check | Result | Scope and limits |
| --- | --- | --- |
| `scripts/qualify-architecture.sh` | PASS, exit 0 | Four-library/core-purity structure checks, workspace all-target checks and 209 Rust tests passed; no ignored tests. This is not the full B exit gate. |
| TLC lifecycle model | PASS | 888 generated states, 273 distinct states, depth 9; existing finite lifecycle model only. |
| TLC authorization-negative mutation | KILLED | Deliberate mutant violates NoUnauthorizedCommit. This is an expected negative result, not a production invariant failure. |
| Kani | NOT RUN | cargo-kani unavailable. No bounded Rust proof result is claimed. |
| Benchmark metric harness | PASS: 299 checks | Includes 20 golden cases, 256 time-translation cases and five killed metric mutants. Synthetic evidence only. |
| Artifact/metric harness | PASS: 528 checks | 512 exhaustive matching cases, four matching/identity boundaries and 12 artifact pipeline cases. Synthetic evidence only. |
| Perception mutation probes | Two killed | Isolated probes documented in the perception ADR, not comprehensive mutation coverage. |
| Independent B6 review | Scoped findings closed | Exact merge boundaries, review visibility, explicit history ancestry and interpolation influence coverage independently rechecked. Not a complete security audit. |
| Isolated real GUI/CLI smoke | PASS for declared steps | CLI-generated Yaw project opened in GUI; synthetic source non-previewable; automatic unresolved-review warning; undo revision 2, redo revision 3; Doctor review at revision 3; byte-identical GUI/CLI Yaw funscript exports. |
| Real corpus / human pilot | NOT RUN | Zero permitted benchmark subjects or human sessions supplied. |

CLI export revision 1 and GUI export revision 3 after undo/redo share SHA-256
`bb05b459b324c78815a0cc9d7da847709ebdf02da65ba11a303151d5d45d7153`.
The revision change is local provenance; standard funscript motion bytes remain identical.

One automated GUI entry returned Forbidden before slower select-all replacement succeeded in the same running GUI/engine. Read-only inspection found the correct grant. Earlier typed bytes were not captured, so exact cause is unproven; no authorization patch or weakened check was applied. This is not a confirmed product auth defect.

Evidence: [gate log](evidence/phase-b/gates-good-increment.log), [evidence manifest](evidence/phase-b/good-increment.json), [GUI project](evidence/phase-b/closeout-open-retry.png), [undo](evidence/phase-b/closeout-undo.png), [redo diagnostics](evidence/phase-b/closeout-doctor.png), [GUI export](evidence/phase-b/closeout-export.png).

Earlier `pre-review-*` and `source-kind-final-*` evidence files describe intermediate binaries. Their names do not establish current-source or whole-phase qualification.

## 4. Immediate engineering priority: large program transport

The engine's 512 KiB candidate/program publication limit and inline 1 MiB control channel cannot carry a full-detail reference 30-minute, 30 fps output. At 54,000 actions, a conservative serialized program already exceeds 2.48 MB.

This is a source-bound limit analysis, not an executed 30-minute benchmark. Some sparse outputs may fit; full-detail reference output does not. Raising only the publication limit fails at downstream inline query/edit surfaces. Silent lossy decimation is not an acceptable workaround.

[ADR 0007](adr/0007-large-motion-artifacts.md) is **proposed and unimplemented**. The next vertical slice must provide immutable, authorization-scoped bulk program transfer plus bounded descriptors for candidate/project queries and edit submission. It must cover revision/identity/digest checks, lease release/revocation, failure atomicity, and CLI/GUI parity before the reference-length performance campaign.

## 5. Inputs and external qualification still needed

- Local permitted benchmark clips, hand-curated funscript references, review annotations and category labels. The requested folder location remains unanswered; no personal media was substituted.
- Human annotation/review sessions and dispositions. Agent-authored fixtures cannot replace the five-human-hour pilot or user acceptance.
- Explicit resource windows for model/runtime experiments on identified hardware. The existing occupied GPU workload was not disturbed.
- Licensed model/runtime artifacts and redistribution evidence before official bundling.
- Supported vendor/platform execution environments, including reference CPU/GPU hardware.
- Separate permission and equipment for physical Handy/Handy 2 qualification. No physical device was actuated.
- Human approval of unresolved evaluation conventions and configuration-bound headless acceptance where required.

These inputs do not excuse remaining software work listed above. Good phase remains active and incomplete; Solid phase has not started.


## 6. Next implementation work package

[Large motion artifact transport](PULSAR_LARGE_PROGRAM_IMPLEMENTATION_PLAN.md) records the dependency-ordered protocol/core/engine/client changes, upload trust rules, lifecycle invariants and acceptance matrix. It remains proposed and unimplemented; writing the work package is not completion of the transport gate.
