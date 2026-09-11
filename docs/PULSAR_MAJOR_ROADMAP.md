# Pulsar Major Roadmap

Version: 1.0
Date: 2026-09-10
Status: Approved implementation direction; milestones require evidence before closure

## Authority and scope

The [Pulsar Software Specification](PULSAR_SOFTWARE_SPECIFICATION.md) is the product contract. Its confirmed requirements and Appendix D decision sources take precedence over this roadmap, implementation notes, README feature claims, and historical plans. Design defaults and open choices remain labeled as such.

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

## Decisions to resolve during implementation

| Item | Resolution point |
| --- | --- |
| Exact model/runtime and per-vendor backend selection | M3 comparison, then M6 platform qualification. |
| Curated pilot sources, annotations, statistical protocol, and expanded data budget | Pilot before model tuning or release qualification; preserve the five-hour initial budget. |
| Concrete box/flow/timestamp contracts and verified consumer paths | M1/M2; derive from actual code and reproducible defects. |
| Native project schema and recovery/version retention | M2. |
| Canonical six-axis coordinates and reference device | M4/M5; physical qualification before Stable. |
| Handy firmware/API/BLE coverage and external-player synchronization | M5. |
| Core size, peak memory, minimum OS versions, signing, extraction/update feasibility | M3/M6; escalate conflicts with confirmed portability requirements. |
| Other ARM64 release milestone and later Intel Mac support | Explicit support-matrix decision; no silent launch-scope expansion or reduction. |

Escalate only genuine conflicts with confirmed requirements or missing human-owned resources/decisions. Do not restart an open-ended requirements interview for ordinary engineering choices.

## Completion and reporting

Each milestone report must state: implemented scope, reproduced/fixed defects, exact checks, external-review result, unresolved requirements, and next dependency. A blocked real-media or hardware gate stays visible even when code-level work passes.

Roadmap approval is not permission to erase repository history, change licenses, publish releases, push to a protected branch, or claim target accuracy/performance without evidence.

