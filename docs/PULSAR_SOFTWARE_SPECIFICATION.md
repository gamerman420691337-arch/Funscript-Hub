# Pulsar Software Specification

Version: 0.2  
Date: 2026-09-10  
Status: Conversation-derived product specification and delivery baseline  
Scope: Initial public Preview/Beta and subsequent Stable release

## 1. Purpose and authority

Pulsar is a local-first, multimodal motion-generation, tracking, editing, and device-playback application. Its ambition is state-of-the-art quality, speed, compatibility, and usability, demonstrated through reproducible evaluation rather than model names or feature lists.

This document consolidates the product-owner interview. It records requirements, not evidence that the current implementation satisfies them. It does not authorize a license change, cloud spending, publication of private data, or modification of existing user scripts.

Requirement status is explicit:

- **C - Confirmed:** A decision accepted during the interview. These requirements define the agreed product contract.
- **D - Design default:** A proposed engineering interpretation that makes the contract implementable. These are not additional product-owner approvals. They may change without weakening a confirmed requirement.
- **V - Validation/open:** A question requiring measurement, compatibility investigation, or a remaining policy decision. It must not be represented as an implemented or qualified capability.

For confirmed requirements, MUST denotes a requirement and MAY denotes an explicitly permitted behavior. For design defaults, SHOULD denotes the proposed implementation baseline.

### 1.1 Decisions that supersede earlier, broader statements

1. Offline generation speed floors apply to **stroke-only** output, not full six-axis generation. Six-axis generation may be slower.
2. Launch requires GPU-accelerated six-axis live output on the **fastest preset**. Higher-quality six-axis live presets are optional.
3. CPU-only six-axis live operation is best effort, without the GPU live latency requirement. It may skip analysis frames to prevent growing backlog.
4. Preview/Beta has an **80% tracking accuracy floor and 90% target**. Stable has a **95% floor**. Accuracy means stroke precision and recall separately, not one blended score.
5. The human-effort limits are Stable gates. Preview may exceed them if measured effort is reported honestly.
6. Creative generation is evaluated using a creative rubric, not exact agreement with one reference script.
7. The optional smaller distribution is an **offline-capable Core**, not a launcher that requires a first-run model download. Core includes all input modalities at its bundled Fast tier.
8. Six-axis device playback may be hardware-unqualified and labeled experimental in Preview. Stable requires physical qualification.

## 2. Product goals and non-claims

| ID | Status | Requirement |
| --- | --- | --- |
| GOAL-01 | C | Deliver excellent funscript generation and tracking across supported media, with competitive quality and speed. |
| GOAL-02 | C | Support local AI, including a bundled natural-language assistant, without mandatory cloud inference. |
| GOAL-03 | C | Provide portable distributions that run without user-managed development tools, inference installations, or mandatory model downloads for their advertised offline capabilities. |
| GOAL-04 | C | Support six-axis motion, with stroke as the most important axis. |
| GOAL-05 | C | Preserve meaningful timing, pauses, shallow movements, and relative amplitude rather than maximizing intensity automatically. |
| GOAL-06 | C | Attempt best-effort generation when evidence is uncertain, while identifying sections needing review. |
| GOAL-07 | C | Establish quality using curated tests and hand-curated references. |
| GOAL-08 | D | Reserve SOTA claims for dated, reproducible comparisons against relevant competing systems on disclosed hardware, inputs, and metrics. |

Neither a model registry entry, a backend compile flag, a generated file, nor an API wrapper proves model availability, accelerated execution, accurate tracking, or device compatibility.

## 3. Terminology

| Term | Meaning |
| --- | --- |
| Input modality | A source family, such as video, audio, text, or an image with a prompt. It is not a device connection method or an output axis. |
| Stroke | The primary linear motion channel and highest-priority quality target. |
| Six-axis / 6DoF | Three translational and three rotational motion channels. Exact coordinate conventions and device mappings require explicit definition. |
| Device-neutral master | Motion expressed independently of one device's travel, speed, and other physical limits. |
| Observed | Motion derived from observable source evidence, with its measurement uncertainty retained. |
| Inferred | Estimated motion where evidence is incomplete, ambiguous, or indirect. It must not be presented as directly observed. |
| Synthetic | Motion intentionally generated from prompts, presets, patterns, or creative interpretation rather than recovered as physical motion from media. |
| Faithful mode | Generation intended to follow source motion without silently reinterpreting it for intensity or musical rhythm. |
| Creative mode | Explicitly requested synthesis or reinterpretation, evaluated against intent and constraints. |
| Quality tier | A generation cost/quality family: Fast, Default, or Maximum. Exact preset names and counts are not yet fixed. |
| Core / Complete | Distribution editions, not quality tiers or release maturity levels. |
| Preview / Stable | Release qualification levels, independent of distribution edition. |
| Realtime factor | Source media duration divided by end-to-end processing wall time. Higher is faster. |
| Review section | A localized interval independently assessable as requiring human review, with reason and confidence information. |

## 4. Input and generation scope

### 4.1 Required input families

| ID | Status | Input | Required behavior |
| --- | --- | --- | --- |
| IN-01 | C | Ordinary 2D video | Recover motion; support automatic target selection and optional user override. |
| IN-02 | C | Stereo, VR180, and 360 video | Support these input families explicitly; do not assume ordinary 1080p performance numbers cover them. |
| IN-03 | C | Audio-only | Generate motion appropriate to the selected task or creative intent; do not claim visually observed motion exists. |
| IN-04 | C | Audio plus video | Use multimodal evidence under the faithful/creative precedence rules. |
| IN-05 | C | Still image plus text prompt | Require the accompanying prompt; produce explicitly synthetic motion. |
| IN-06 | C | Text prompt | Generate synthetic motion using premade motions, presets, patterns, and local AI interpretation. |
| IN-07 | C | Existing scripts | Support use in editing, transformation, or generation workflows while preserving user-authored work. |
| IN-08 | C | Live camera and streams | Support synchronized generation and device output under the live requirements. |

IN-09 **[C]** Core's bundled Fast tier MUST cover every required input family offline without adding packs. A still image still requires its accompanying prompt.

IN-10 **[D]** The release SHOULD publish a finite qualification matrix for containers, codecs, stream schemes, stereo layouts, projections, frame rates, resolutions, and script variants. "All modalities" does not establish compatibility with every possible encoding, inaccessible source, or protected stream.

IN-11 **[D]** An unsupported or undecodable source SHOULD produce a clear failure or explicit partial-result state, not a successful-looking script derived from missing frames.

### 4.2 Automatic operation and source precedence

| ID | Status | Requirement |
| --- | --- | --- |
| GEN-01 | C | Video generation MUST work without mandatory manual target selection. The normal workflow is select media and generate. |
| GEN-02 | C | Clicking a target or providing a prompt MAY override automatic selection. |
| GEN-03 | C | Ambiguity MUST trigger review information, while generation attempts a best-effort result rather than stopping routinely for user input. |
| GEN-04 | C | In faithful video mode, clearly visible motion MUST take precedence over conflicting audio. Audio may assist uncertain sections but must not silently retime clear visual motion. |
| GEN-05 | C | Beat-driven or prompt-driven reinterpretation of visible motion requires an explicit creative mode or instruction. |
| GEN-06 | C | Observed, inferred, and synthetic output MUST remain distinguishable. |
| GEN-07 | C | Faithful generation MUST preserve pauses and relative/shallow motion. Expanding every stroke to full range is not the default. Intensity expansion is opt-in. |
| GEN-08 | D | Tracking SHOULD maintain target identity within a scene, account for camera motion and scene transitions, and flag unresolved identity changes. |
| GEN-09 | D | Global image movement, bounding-box size, and aspect ratio SHOULD NOT be treated as proof of physical translation or rotation. Any such inference needs explicit provenance and qualification. |

Best-effort generation does not authorize fabricated confidence or indefinite physical playback after a connection failure.

## 5. Motion representation and device adaptation

### 5.1 Master output

MOT-01 **[C]** Pulsar MUST support six-axis motion generation, with stroke receiving the highest quality priority.

MOT-02 **[C]** The normal funscript export MUST remain device-neutral. Device-specific playback profiles and optional adapted exports are separate from the master.

MOT-03 **[C]** Device adaptation MUST preserve timing and adjust amplitude to respect device limits. It must not normalize each shallow movement independently to full travel.

MOT-04 **[C]** Mapping MUST account for device-specific travel and applicable velocity, acceleration, jerk, and coupled-axis constraints. Profiles require qualification rather than assumed universal values.

MOT-05 **[D]** The master SHOULD remain unchanged by device adaptation. If adaptation substantially changes or cannot reproduce requested motion, the application should report that limitation.

Illustrative relative mapping: master positions 20 to 40 within a device's usable normalized range of 10 to 90 become 26 to 42. They do not become 10 to 90. This illustrates range mapping, not a complete dynamics limiter.

### 5.2 Interchange and project information

MOT-06 **[D]** Standard stroke `.funscript` output SHOULD remain compatible with external consumers. Multi-axis companion naming, axis conventions, and interchange behavior must be qualified explicitly.

MOT-07 **[D]** A native project or associated sidecar SHOULD preserve information that ordinary playback exports do not reliably retain: source references, all motion tracks, confidence, provenance, review sections, protected edits, generation settings, and model/runtime versions.

MOT-08 **[V]** Exact axis coordinates, sign conventions, companion-file naming, and secondary-axis quality thresholds remain unresolved. Stroke metrics must not be presented as six-axis accuracy evidence.

Vibration and suction were discussed as separate channels but were not approved as mandatory launch outputs. They are not implicitly included in the six-axis requirement.

## 6. Editions, packaging, and offline behavior

### 6.1 Distribution editions

| Edition | Status | Required contents and behavior |
| --- | --- | --- |
| Core | C | Portable application, bundled inference runtimes, baseline Fast models for all input modalities, local assistant, editing/export, and offline playback. It works before optional packs are installed. |
| Complete | C | The same core application plus models/packs needed for every advertised quality tier and local assistant. Advertised capabilities work offline after extraction. |
| Additional packs | C | Higher-quality or additional compatible packs can be obtained through optional downloads or offline file import. |

PKG-01 **[C]** Complete is the recommended/default full-offline distribution. An optional smaller Core distribution is also required.

PKG-02 **[C]** A Complete download of 20 GB or more is acceptable. No hard maximum was approved. Core's download-size ceiling remains open.

PKG-03 **[C]** A small application core and larger, versioned runtime/model assets are acceptable. Assets MAY be extracted on first launch and loaded on demand.

PKG-04 **[C]** Users MUST NOT need to install Cargo, Python packages, a separate inference service, or similar development/runtime dependencies for advertised bundled functionality. GPU drivers remain system prerequisites.

PKG-05 **[C]** First-launch extraction MUST NOT be replaced by a mandatory network download for Core's baseline capabilities or Complete's advertised installed tiers.

PKG-06 **[C]** Component updates should avoid redownloading unchanged large packs. Exact update, rollback, and compatibility policies are design work, not already implemented behavior.

### 6.2 Platform artifacts

| Platform | Status | Artifact target |
| --- | --- | --- |
| Windows | C | A single executable distribution with first-launch extraction permitted. |
| Linux | C | A single AppImage distribution with extraction permitted. |
| macOS | D | A native `.app` distributed through a `.dmg`. macOS support is confirmed; this particular packaging format was a proposed default. |

PKG-07 **[V]** Large single-file delivery, signing/notarization, archive/file-size limits, extraction space, removable storage, and standard-OS dependencies must be checked before promising final artifacts. This specification does not establish that a 20 GB self-extracting executable meets every platform's signing and distribution constraints.

PKG-08 **[D]** Portable operation SHOULD support a configurable application-owned data root containing installed packs and portable settings. Relocation should not require absolute paths to the original machine. OS-managed drivers, permissions, and caches remain distinct from application portability.

PKG-09 **[D]** Extraction and pack installation SHOULD be transactional, interruption-safe, and integrity-checked. An incomplete or incompatible pack should not replace a working installation.

## 7. Platform, architecture, and hardware support

### 7.1 Architecture priorities

| Priority | Target | Status |
| --- | --- | --- |
| 1 | Windows and Linux x86-64 | C: primary launch targets. |
| 2 | macOS Apple Silicon | C: macOS launch target. |
| 3 | Windows/Linux ARM64 | C: next architectural priority. V: exact release milestone was not fixed. |
| 4 | Intel macOS | C: not required at launch; later support. |

Minimum OS versions and distribution-specific Linux qualification remain open. Deferring Intel Macs does not defer Intel GPU support on other supported systems.

### 7.2 GPU coverage

HW-01 **[C]** NVIDIA, AMD, Apple, and Intel GPU support are launch requirements, in that priority order.

HW-02 **[C]** Fast, Default, and Maximum MUST support genuine GPU acceleration across all four vendors on qualified hardware. Merely detecting a provider or compiling a GPU feature is insufficient.

HW-03 **[D]** The application SHOULD identify the actual active execution path and disclose CPU fallback, unsupported operations, or model substitutions. A CPU-only fallback must not be labeled GPU acceleration.

HW-04 **[V]** The exact AMD, Apple, and Intel reference devices and backend/runtime combinations have not been selected. NVIDIA speed targets must not be generalized to every vendor without measurement.

### 7.3 Hardware floor and references

| Item | Status | Contract |
| --- | --- | --- |
| System memory | C | Minimum supported system: 16 GB RAM. |
| CPU operation | C | CPU-only operation is supported; GPU acceleration is optional for general application use. |
| Reference CPU | C | Modern 8-core Intel 10th-generation class or faster. Exact SKU/configuration must be recorded for qualification. |
| Reference GPU | C | NVIDIA 3060-class performance with 8 GB VRAM. Exact board/configuration must be recorded; do not silently substitute a different memory configuration. |
| Upper Default / Maximum | C | Stronger GPUs, such as a 4090-class device, are recommended. |
| CPU Maximum | C | Available with longer processing times; the Default CPU speed floor does not apply to Maximum. |
| Per-tier memory eligibility | V | Exact peak RAM/VRAM requirements, particularly for Maximum, must be qualified. The interview did not establish that every model fits every 8 GB GPU. |

### 7.4 Memory sharing and responsiveness

MEM-01 **[C]** If an assistant request arrives during offline generation and the two models cannot fit in memory together, Pulsar MUST pause/checkpoint offline generation, make memory available for the assistant, and resume generation afterward.

MEM-02 **[C]** Live device playback MUST remain protected from that resource handoff. The offline-generation pause policy is not permission to stall live control.

MEM-03 **[D]** Model loading SHOULD be budgeted and demand-driven. Download size does not equal resident memory use. Budgets should account for weights, activations, decoder buffers, UI, assistant, runtimes, and GPU allocations without double-counting unified memory.

MEM-04 **[V]** A 12 GB whole-application memory budget on a 16 GB system was suggested, but not approved as a hard number. Peak RAM/VRAM caps, startup deadlines, and assistant response-time targets remain to be set through qualification.

## 8. Performance requirements

### 8.1 Reference offline workload

The agreed reference workload is a 30-minute, 1080p, 30-fps video. Performance is end-to-end, including decoding, inference, processing, and export. These figures are targets, not measured results.

`realtime_factor = source_duration_seconds / processing_wall_time_seconds`

| ID | Status | Workload | Minimum speed | Time for reference video |
| --- | --- | --- | --- | --- |
| PERF-01 | C | CPU Fast, stroke-only | Strictly greater than 1x realtime | Less than 30 minutes |
| PERF-02 | C | CPU Default, stroke-only | At least 0.5x realtime | At most 60 minutes |
| PERF-03 | C | Reference GPU Fast, stroke-only | At least 10x realtime | At most 3 minutes |
| PERF-04 | C | Reference GPU Default, stroke-only | At least 5x realtime | At most 6 minutes |
| PERF-05 | C | Maximum | May be slower than realtime | No fixed speed floor approved |
| PERF-06 | C | Offline six-axis generation | May be slower than stroke-only generation | Stroke-only floors do not apply |

CPU Default is expected around 0.5x to 1x realtime; faster performance is welcome, not a violation. The CPU half-realtime floor applies to Default, not Maximum. No 2x CPU Fast requirement was approved.

PERF-07 **[D]** Qualification SHOULD time an uninterrupted generation request through completed export and report cold/warm model-load conditions. First-launch installation/extraction should be reported separately. User-triggered pauses and assistant handoffs must remain visible in real job elapsed time and ETA.

PERF-08 **[D]** Stroke-only and six-axis performance SHOULD be reported separately. All benchmark reports should disclose media, preset, model/runtime versions, hardware, drivers, and actual acceleration path.

PERF-09 **[V]** VR/high-resolution video, other input families, upper-Default presets, exact non-NVIDIA GPU reference devices/workloads, and long-duration scaling need separate qualification envelopes. This does not waive the common Fast stroke-only floor in PERF-10.

PERF-10 **[C]** Every qualified CPU/GPU Fast execution path MUST exceed 1x realtime for stroke-only workloads within its published hardware/input envelope, including AMD, Apple, and Intel GPU paths. The reference-GPU Fast requirement remains at least 10x; the common floor does not replace that stronger requirement. Six-axis generation remains exempt under PERF-06. An out-of-envelope workload must not be represented as meeting the qualified Fast guarantee.

### 8.2 Live operation

| ID | Status | Requirement |
| --- | --- | --- |
| LIVE-01 | C | Support camera/stream input with synchronized device output. |
| LIVE-02 | C | Responsive live target: 200 ms capture-to-command, including decoding, inference, and dispatch. Physical device response is measured separately. |
| LIVE-03 | C | Offer an optional buffered-quality mode allowing up to one second of added media playback delay. |
| LIVE-04 | C | When Pulsar controls media, synchronize audio, video, and motion using a shared presentation clock and device lag compensation. |
| LIVE-05 | C | Launch requires GPU live six-axis output on the fastest preset. Higher-quality live six-axis presets are optional. |
| LIVE-06 | C | CPU live six-axis output is best effort and may be much slower; it has no agreed live latency gate. |
| LIVE-07 | C | CPU live six-axis analysis may skip frames, use a bounded latest-frame queue, and lower analysis cadence rather than accumulate growing backlog. Show actual delay. |

LIVE-08 **[D]** Analysis timestamps, presentation timing, and device scheduling SHOULD remain distinct. Skipping analysis frames is not permission to relabel old observations as current measurements or replay stale commands.

LIVE-09 **[V]** The statistical definition of the 200 ms target, jitter limits, externally controlled player synchronization, and maximum valid command age require qualification. A percentile was not agreed for live latency; the 95th-percentile agreement concerns human effort, not automatically latency.

Buffering alone does not repair sustained processing backlog. CPU six-axis concessions do not weaken the GPU fastest-preset live requirement.

## 9. Tracking quality and review gates

### 9.1 Material errors

| ID | Status | Error condition |
| --- | --- | --- |
| QUAL-01 | C | More than 100 ms deviation at a reference stroke reversal is material timing error requiring a review flag. |
| QUAL-02 | C | More than 20% error in reference stroke travel is material amplitude error requiring a review flag. Compare device-neutral motion, not device-adapted output. |
| QUAL-03 | C | Wrong targets, missed strokes, extra/invented strokes, and major timing/amplitude errors are material errors. Missing shallow strokes count separately rather than disappearing into amplitude tolerance. |
| QUAL-04 | C | Review flags must detect at least 95% of independently annotated materially incorrect sections. |

For a nonzero reference excursion, relative amplitude error is:

`abs(generated_excursion - reference_excursion) / reference_excursion`

QUAL-05 **[D]** Stroke matching SHOULD be one-to-one so that duplicates and missing events cannot be hidden. An eligible matched stroke must satisfy the agreed timing and amplitude tolerances. A pause or zero reference excursion should be handled as a pause/event classification, not division by zero or artificial full-range movement.

QUAL-06 **[V]** Exact stroke-event definitions, interval boundaries, annotation uncertainty, near-zero excursion handling, and matching rules must be frozen in the annotation protocol. Tighter phase-relative checks for rapid strokes were suggested, but no additional numeric threshold was approved.

### 9.2 Default-tier release gates

| Metric | Preview/Beta | Stable |
| --- | --- | --- |
| Pre-edit stroke precision | At least 80%; 90% target | At least 95% |
| Pre-edit stroke recall | At least 80%; 90% target | At least 95% |
| Aggregation | Evaluate precision and recall separately in each required benchmark category | Same; overall averages must not hide weak categories |
| Material-error flag recall | At least 95%; no Preview relaxation was approved | At least 95% |
| Active editing per 30-minute video | Measure/report; exceeding five minutes does not block Preview | At most five minutes for at least 95% of videos in each category |
| Total review plus editing per 30-minute video | Measure/report; exceeding ten minutes does not block Preview | At most ten minutes for at least 95% of videos in each category |

QUAL-07 **[C]** Review time is distinct from active editing time. Total human effort includes both; false alarms consume this budget.

QUAL-08 **[D]** Flags SHOULD be localized and actionable, with timestamps, reasons, confidence, and provenance. Flagging an entire video must not be used to manufacture apparently high error-detection recall.

QUAL-09 **[D]** Reports SHOULD include category-level results, difficult cases, missed flags, false alarms, sample counts, and statistical uncertainty. Benchmark sections must be defined independently of the generator.

QUAL-10 **[V]** Exact corpus sizes, category membership, flag-recall aggregation, confidence-interval policy, and the statistical release decision rule remain open. A five-hour pilot cannot establish release-level coverage or a 95th-percentile claim.

QUAL-11 **[V]** These numerical accuracy gates were established for Default. Separate Fast/Core and Maximum quality floors were not agreed. They must not be silently inferred from Default or omitted from release reporting.

QUAL-12 **[D]** Maximum SHOULD demonstrate an improvement or justified tradeoff against Default on paired evaluation. More computation alone is not evidence of better output.

### 9.3 Creative generation

CREATIVE-01 **[C]** Text/image-driven synthetic output MUST use a creative evaluation rubric when multiple motion patterns can satisfy the same prompt. Exact matching to one reference script is not the general quality criterion.

CREATIVE-02 **[C]** The rubric evaluates prompt adherence, timing/pattern constraints, and correction effort. Observed-motion stroke precision/recall remains the appropriate gate for faithful tracking, not a substitute for creative assessment.

CREATIVE-03 **[D]** Deterministic preset and constrained-pattern tests SHOULD still verify exact timing, bounds, pauses, and explicitly requested structures. More open-ended outputs should be assessed against permitted variation.

CREATIVE-04 **[V]** Numeric creative pass thresholds, rater instructions, and evaluation of audio-only or mixed creative tasks remain unresolved. No 80% or 95% creative pass rate was separately approved.

### 9.4 Secondary-axis quality

SIX-01 **[C]** Six-axis support and stroke priority are confirmed. Confidence and observed/inferred/synthetic provenance must remain available for generated axes.

SIX-02 **[V]** Secondary-axis ground truth, error metrics, coordinate calibration, and minimum accuracy thresholds are not established. Stroke success cannot qualify rotations or other translations automatically.

## 10. Data and model-development strategy

| ID | Status | Requirement |
| --- | --- | --- |
| DATA-01 | C | Benchmark pretrained candidates first. Targeted fine-tuning is allowed where measured quality gaps remain. |
| DATA-02 | C | Training data MUST remain separate from held-out evaluation data. |
| DATA-03 | C | Build a small curated training pilot rather than assume an existing labeled training corpus is ready. |
| DATA-04 | C | Initial pilot budget is five human-hours total for labeling and review. |
| DATA-05 | C | The pilot is an annotation-method trial with limited coverage, not training completion or release qualification. |

DATA-06 **[D]** Start the pilot with a bounded stroke-focused video slice and representative failure cases. This is a proposed pilot focus, not a reduction of launch modality scope. Record annotation time, ambiguity, reference consistency, and achieved coverage; stop and reassess at the five-hour budget.

DATA-07 **[D]** Development, tuning, and held-out evaluation SHOULD be separated by source media or other leakage-relevant groups, not adjacent frames from the same source. Keep versioned manifests and content identities for splits and annotations.

DATA-08 **[D]** Model changes SHOULD be driven by development-set failures. Repeatedly training against a supposedly held-out qualification set invalidates the held-out claim and requires a revised evaluation protocol.

DATA-09 **[D]** Separate measurable motion annotations from stylistic preferences in expert-authored scripts. Record uncertain or unobservable intervals instead of inventing physical ground truth.

DATA-10 **[D]** Use data permitted for the intended training and evaluation purposes, with source/provenance records and appropriate access controls. This document grants no permission to upload private media or publish datasets.

DATA-11 **[V]** Source selection, annotator assignment, exact annotation fields, expanded dataset budget, training compute, and cloud-training policy remain to be established. Five human-hours is not a cloud-spending or GPU-training authorization.

MODEL-01 **[D]** Select models and runtimes through an evidence-based comparison covering quality, latency/throughput, memory, artifact size, offline deployment, supported operators, CPU/GPU portability, and redistribution suitability.

MODEL-02 **[V]** No final perception model, local assistant model, quantization, or exclusive inference runtime was selected during the interview. Placeholder weights, guessed artifact sizes, and unimplemented wrappers are not model selections.

## 11. Editing, assistant, and regeneration contract

EDIT-01 **[C]** The bundled local assistant MUST support natural-language generation and editing workflows, including selecting tracking targets, explaining review flags and their reasons/confidence, adjusting generation settings, and making undoable script edits. Core includes these capabilities offline. These are concrete application actions, not merely conversational advice.

EDIT-02 **[C]** Regeneration MUST preserve manually edited regions by default. It regenerates unlocked regions and maintains undoable versions. Replacing protected edits requires an explicit user action.

EDIT-03 **[C]** Users MUST be able to override automatic target selection and act on review flags without requiring a complete project regeneration.

EDIT-04 **[D]** The native project SHOULD preserve source references, master motion, protected regions, review state, operation history, and generation provenance. Playback exports are not substitutes for the editable project.

EDIT-05 **[D]** Assistant operations SHOULD use bounded application actions with validation and undo, not arbitrary shell execution or unrestricted model-generated code. Media content and embedded text are data, not authority to operate devices or upload files.

EDIT-06 **[D]** File writes SHOULD be atomic or recoverable. Batch and single-file generation should never silently truncate existing user scripts. Partial multi-axis regeneration should preserve existing protected channels and report generated/skipped/failed channels accurately.

EDIT-07 **[D]** Cancellation, crashes, decode failures, and interrupted exports SHOULD preserve recoverable work and must not label incomplete output as completed. Exporting a partial result should be an explicit operation with coverage recorded.

EDIT-08 **[V]** Native project format, version migration, autosave/checkpoint cadence, crash-recovery guarantees, and model-version retention policy remain to be specified. Exact byte-for-byte regeneration across backends was not promised.

## 12. Device integration and physical playback

### 12.1 Handy platform

DEV-01 **[C]** The Handy platform, including Handy 2, is required for launch integration and is the identified stroke-device reference family.

DEV-02 **[C]** All supported input modalities MUST be usable in Handy generation/playback workflows. Device output remains limited to the actual device capabilities; this is not a requirement for Handy hardware to provide six physical axes.

DEV-03 **[C]** Both online control through Handy services and optional offline Bluetooth playback MUST be supported. Offline Bluetooth is an available path, not the only permitted path.

DEV-04 **[C]** Online transmission of generated scripts or motion commands requires explicit user opt-in. Source media and prompts remain local under the agreed integration design. Bluetooth failure must not trigger a silent switch to cloud control.

DEV-05 **[D]** Each supported device model/firmware/transport combination SHOULD have a capability profile and tested synchronization, limit handling, and disconnect behavior. API-family compatibility is not sufficient proof of physical playback.

DEV-06 **[V]** Exact Handy variants, firmware floors, API versions, BLE feature coverage, and per-platform transport qualification remain open. The manufacturer's documentation describes BLE integrations as self-supported; do not assume vendor-backed compatibility without testing.

### 12.2 Six-axis devices

DEV-07 **[C]** A specific six-axis reference device has not been selected.

DEV-08 **[C]** Preview MAY ship six-axis generation/export and hardware-unqualified six-axis device playback, provided direct playback is clearly labeled experimental.

DEV-09 **[C]** Stable requires physical six-axis playback qualification. Protocol simulators and exported companion files alone do not establish that qualification.

### 12.3 Disconnects, stale work, and resumption

SAFE-01 **[C]** After source or device disconnection/reconnection, device playback MUST remain disarmed until the user explicitly resumes it.

SAFE-02 **[C]** Stale queued commands MUST NOT restart physical motion on reconnection. Best-effort generation is not permission for surprise movement.

SAFE-03 **[D]** Device scheduling SHOULD be independent of inference workers and validate command timing and profile limits. Stop behavior must be profile-specific; an arbitrary move to zero or midpoint is not a universal safe stop.

SAFE-04 **[D]** Qualification SHOULD document device-side timeout/watchdog behavior and the limits of host control when a connection is lost. The application cannot claim that a software stop command always reaches a disconnected device.

SAFE-05 **[V]** Maximum command age, disconnect detection thresholds, stop transitions, and per-device emergency behavior need physical qualification. They were not assigned numerical values in the interview.

## 13. Architecture baseline

The accepted direction is a Rust product core with bundled local inference runtimes. The product owns generation, tracking orchestration, editing, timing, and adaptation; it is not merely a UI around an external service.

The following decomposition is a **design default**, not a claim about current module completeness:

1. **Media layer:** local files, supported streams/cameras, image/text/script inputs, decoding, timestamps, and modality metadata.
2. **Inference adapters:** actual model loading/execution, capability discovery, provenance, memory accounting, and explicit backend/fallback reporting.
3. **Shared generation engine:** target tracking, multimodal evidence, uncertainty, motion estimation, event extraction, and neutral six-axis output. GUI, CLI, and batch should share semantics and configuration.
4. **Live scheduler:** a latency-bounded scheduling variant using the same domain contracts, with frame dropping where allowed and a presentation clock separate from model execution.
5. **Project/editor layer:** editable master, protected regions, versioned operations, review workflow, and export.
6. **Device layer:** profiles, amplitude/dynamics adaptation, synchronized command scheduling, online/BLE transport, disarming, and reconnection rules.
7. **Packaging/update layer:** platform artifacts, compatible runtime/model packs, offline import, extraction, integrity checks, and rollback.

### 13.1 Proposed logical interfaces

These define responsibilities, not frozen Rust APIs or wire schemas.

| Record/interface | Minimum responsibility |
| --- | --- |
| Media source descriptor | Source identity, timestamps/timebase, duration or live status, modality, projection/layout, and access method. |
| Generation request | Sources, faithful/creative intent, quality preset, requested axes, selected/automatic target, protected regions, and resource policy. |
| Inference observation | Source timestamp, target identity, measured/inferred values, confidence, provenance, and model/backend identity. |
| Motion bundle | Device-neutral tracks, temporal coverage, observed/inferred/synthetic attribution, and review sections. |
| Project state | Source references, motion versions, protected edits, operations, configuration, and artifact provenance. |
| Device profile | Supported axes, usable ranges, qualified dynamics limits, timing compensation, transport capabilities, and failure behavior. |
| Pack manifest | Artifact identity/version/hash, runtime compatibility, model capabilities, supported execution paths, and redistribution metadata. |

External file/transport contracts should be versioned and qualified before they become compatibility promises.

## 14. Privacy, security, and update defaults

| ID | Status | Policy |
| --- | --- | --- |
| SEC-01 | C | Core generation and assistant functionality remain local and available offline. |
| SEC-02 | C | Handy-server script/command transmission is permitted with explicit consent; no automatic BLE-to-cloud fallback. |
| SEC-03 | D | No telemetry or source-media upload by default. Exported logs should avoid source content, prompts, private URLs, and device credentials. |
| SEC-04 | D | Runtime/model packs should have authenticated release metadata, verified integrity, and compatibility checks. Placeholder hashes cannot qualify a release pack. |
| SEC-05 | D | Updates should preserve projects and user edits, install transactionally, and support recovery to a working compatible state. |
| SEC-06 | D | An update or model replacement should not silently rewrite existing generated masters or project history. |
| SEC-07 | V | Signing/notarization infrastructure, update-consent behavior, credential storage in portable mode, rollback retention, and consent persistence need final design. |
| SEC-08 | V | Code/runtime/model/data redistribution permissions must be reviewed before release. No license migration is authorized by this specification. |

## 15. Planned acceptance scenarios

These are required validation scenarios for implementation work. They have not been executed as part of writing this document.

| ID | Scenario | Expected evidence/result |
| --- | --- | --- |
| TEST-01 | First launch of Core on a supported machine with Internet disabled and no developer/inference tools installed | Extraction succeeds; bundled assistant and all Fast input families work without downloads. Record actual OS/driver prerequisites. |
| TEST-02 | First launch of Complete offline | Every advertised installed tier is usable without fetching additional models. |
| TEST-03 | Interrupted extraction, damaged pack, incompatible pack, or interrupted update | Clear failure, no use of corrupted assets, existing working state recoverable. |
| TEST-04 | Reference stroke-only 30-minute video on CPU and reference GPU | Report end-to-end times against PERF-01 through PERF-04, actual backend, preset, cold/warm conditions, and export completion. |
| TEST-05 | Fast/Default/Maximum on each GPU vendor | Evidence of genuine accelerated execution and qualified model output; no compile-flag-only success claim. |
| TEST-06 | Constant signal, pause, shallow strokes, and non-full-range motion | No invented full-range stroke; faithful timing and relative excursion retained. |
| TEST-07 | Target ambiguity, occlusion, camera movement, scene cut, and target change | Best-effort output with appropriate confidence/provenance and localized review flags. |
| TEST-08 | Clear visual motion conflicts with audio rhythm | Faithful output follows visual motion; creative reinterpretation requires explicit intent. |
| TEST-09 | Text and still-image-plus-prompt generation | Local synthetic generation works; missing image prompt is handled clearly; creative rubric and deterministic constraints are assessed separately. |
| TEST-10 | Device profile changes and adapted export | Neutral master remains unchanged; timing preserved; amplitude/dynamics adaptation and limitations reported. |
| TEST-11 | GPU fastest-preset six-axis live session | Measure capture-to-command latency, presentation synchronization, physical device lag separately, and sustained behavior. |
| TEST-12 | CPU six-axis live workload exceeds analysis capacity | Bounded queue, skipped analysis frames, visible actual delay, and no growing stale-command backlog. |
| TEST-13 | Assistant request while offline generation occupies available memory | Recoverable generation pause, assistant response, resumed job, and protected live playback. |
| TEST-14 | Regeneration overlaps manually edited regions | Protected regions preserved unless explicitly unlocked/replaced; operation remains undoable. |
| TEST-15 | Batch job finds an existing stroke file but missing secondary files | Existing work is not silently truncated; per-channel results and progress are accurate. |
| TEST-16 | Cancellation, decode failure, or crash during generation/export | Existing files remain intact; partial work is distinguishable from successful completion and recoverable where supported. |
| TEST-17 | Handy online and offline BLE playback across supported models/firmwares | Qualified synchronization, limits, consent, transport behavior, and disclosed unsupported combinations. |
| TEST-18 | Source/device disconnect and reconnect during playback | Playback remains disarmed; stale commands do not restart motion; explicit user resume required. |
| TEST-19 | Held-out Default evaluation by category | Separate stroke precision/recall, material-error flag recall, false alarms, and human-effort distributions, with no training leakage. |
| TEST-20 | Physical six-axis Stable qualification | Actual multi-axis hardware evidence, not just software simulation or successful export. |
| TEST-21 | Portable relocation and offline pack import | Source/model references and settings follow the selected portability contract without hidden mandatory downloads. |
| TEST-22 | Stroke-only Fast generation on every qualified CPU/GPU execution path, including AMD, Apple, and Intel | Each supported workload in the declared envelope exceeds 1x realtime end-to-end; the reference NVIDIA GPU also meets its stronger 10x floor. Report six-axis results separately without imposing stroke-only floors. |
| TEST-23 | Offline assistant selects a tracking target, explains a flagged interval, changes generation settings, and edits a script | Requested target/settings affect application state, explanations expose recorded flag reasons/confidence, and script edits are undoable while preserving protected regions unless explicitly replaced. No cloud inference is required. |

## 16. Delivery plan and exit conditions

These stages are an implementation roadmap, not claims that work has been completed. Internal stroke-first stages do not reduce the agreed launch modality scope.

### Stage 0: Establish the implementation and evidence baseline

- Reconcile the current repository state with the reviewed snapshot before choosing changes. Preserve unrelated work.
- Map confirmed requirements to real code paths and distinguish working integrations from placeholders.
- Freeze the pilot annotation method, artifact identity conventions, and benchmark reporting format.
- Run the five-human-hour annotation/review pilot and record achieved coverage and limitations.

Exit: a bounded baseline and pilot report, not a release-quality claim.

### Stage 1: Prove the stroke generation path

- Consolidate shared generation behavior across GUI, CLI, and batch.
- Correct unsafe overwrite paths and fabricated motion from flat/shallow signals where still present.
- Compare real pretrained models and runtimes on development cases; use targeted fine-tuning only for measured gaps.
- Establish faithful motion, confidence/review output, CPU/GPU reference measurements, and initial Handy playback evidence.

Exit: a real end-to-end stroke path with traceable output and measured behavior, not named-but-unused model wrappers.

### Stage 2: Complete the application contracts

- Implement protected edits, selective regeneration, undoable versions, recovery, and resource-aware local assistant handoff.
- Implement offline assistant target selection, review-flag explanations, and generation-setting changes through application actions.
- Complete multimodal faithful/creative behavior and Core's offline Fast coverage.
- Implement neutral six-axis generation and device profiles, with explicit secondary-axis inference and limitations.
- Qualify all required GPU vendors and fastest-preset GPU six-axis live behavior; implement CPU best-effort live scheduling.

Exit: feature evidence covering required workflows, with unresolved hardware and quality gaps explicitly marked.

### Stage 3: Package and qualify public Preview/Beta

- Produce portable Core and Complete artifacts with real bundled models/runtimes and offline installation/import behavior.
- Meet applicable confirmed hardware, transport, stroke-speed, and Preview tracking gates.
- Report human effort even when above Stable limits.
- Mark physically unqualified six-axis playback experimental; do not extend that exception to unrelated requirements.

Exit: a Preview qualification report and explicit supported-feature/hardware matrix. Creative, Fast/Core, and secondary-axis evaluation gaps must be resolved or conspicuously disclosed before corresponding quality claims.

### Stage 4: Qualify Stable

- Expand evaluation beyond the discovery pilot with frozen held-out data and category coverage.
- Meet the 95% Default tracking floors, flag recall, and agreed per-category human-effort gates.
- Establish creative pass criteria and independent secondary-axis qualification.
- Complete real six-axis device testing, compatibility qualification, recovery/update evidence, and reproducible release packaging.

Exit: an artifact-backed Stable report. SOTA claims additionally require appropriate competitor comparisons, not just internal threshold passes.

## 17. Remaining decisions and validation tasks

The interview is complete for now. These items should become bounded design/measurement work, not another open-ended feature interview. Escalate only when a finding requires changing a confirmed contract.

| ID | Open item | Resolution path |
| --- | --- | --- |
| OPEN-01 | Exact model weights, quantization, runtimes, and per-vendor execution providers | Model/runtime comparison with quality, speed, memory, packaging, and redistribution evidence. |
| OPEN-02 | Core size, peak RAM/VRAM, startup and assistant timing limits | Measure candidate configurations on the 16 GB baseline; document proposed caps without claiming they were already approved. |
| OPEN-03 | Single-file large-artifact packaging and signing feasibility | Platform-specific packaging investigation; explicitly escalate any conflict with the one-file delivery contract. |
| OPEN-04 | Minimum OS versions and Windows/Linux ARM64 milestone | Publish a proposed support matrix and qualify it; Intel Macs remain excluded from launch. |
| OPEN-05 | AMD/Apple/Intel reference hardware and complete preset mapping | Choose representative hardware and disclose exact configurations; do not reuse NVIDIA numbers as proof. |
| OPEN-06 | Pilot sources, labels, category taxonomy, reference uncertainty, and expanded data budget | Complete the five-hour method trial, then propose the smallest defensible evaluation/training expansion. |
| OPEN-07 | Statistical release decision rule and dataset sizes | Predeclare aggregation, sample counts, uncertainty reporting, and whether gates use point estimates or confidence bounds. |
| OPEN-08 | Fast/Core, Maximum, creative, and secondary-axis quality gates | Define and qualify separately; do not silently extend or evade Default's agreed metrics. |
| OPEN-09 | Six-axis reference device, canonical coordinates, and export conventions | Select hardware/protocol conventions and perform physical qualification before Stable. |
| OPEN-10 | Handy variants/firmware floors, BLE/API coverage, and external player synchronization | Build a concrete device/transport/player matrix and test it. |
| OPEN-11 | Native project schema, recovery guarantees, and retention of old model/pipeline versions | Specify a versioned project and migration contract; preserve user edits and recorded provenance. |
| OPEN-12 | Credential storage, update trust, consent persistence, rollback, and redistribution review | Security/release design and evidence. No implicit cloud-training, private-data publication, or licensing authorization. |
| OPEN-13 | Live jitter, latency percentile, stale-command thresholds, and physical stop behavior | Instrument shared-clock playback and qualify limits per hardware/device profile. |

## Appendix A. Repository review context

The interview included review of upstream commit `7eb6d62b47cae6e8819a5ec685af365cbc684654` in `gamerman420691337-arch/pulsar-fs`. The review read the upstream commit without replacing the local checkout. This document does not assert that it is still the latest upstream commit or that the current working tree matches it.

Reported risks at that reviewed snapshot included:

- Partial multi-axis batch generation could overwrite existing script channels; GUI and CLI handling diverged.
- A flat signal could be converted into a fabricated 0-to-100 stroke, and normalization could destroy relative amplitude.
- SAM decoder paths returned empty results; neural point-tracking integration did not establish a valid end-to-end video tracking path.
- Model registry hashes remained `pending`, and registry descriptions were not evidence of trained/exported/qualified artifacts.
- GUI/CLI/live generation semantics and actual adaptive-model use were inconsistent.
- GPU labeling could follow build features rather than observed execution-provider behavior.
- Distribution still depended on source builds, external media executables, and external model discovery/download rather than the agreed portable offline packages.

These are historical review findings to recheck before implementation, not a new current-checkout audit or a statement that all defects remain unchanged.

Pinned source references:

- [Model registry](https://github.com/gamerman420691337-arch/pulsar-fs/blob/7eb6d62b47cae6e8819a5ec685af365cbc684654/src/neural/model_manager.rs)
- [SAM integration](https://github.com/gamerman420691337-arch/pulsar-fs/blob/7eb6d62b47cae6e8819a5ec685af365cbc684654/src/neural/sam.rs)
- [Point tracker](https://github.com/gamerman420691337-arch/pulsar-fs/blob/7eb6d62b47cae6e8819a5ec685af365cbc684654/src/neural/point_tracker.rs)
- [Signal processing](https://github.com/gamerman420691337-arch/pulsar-fs/blob/7eb6d62b47cae6e8819a5ec685af365cbc684654/src/signal.rs)
- [Batch queue](https://github.com/gamerman420691337-arch/pulsar-fs/blob/7eb6d62b47cae6e8819a5ec685af365cbc684654/src/batch/queue.rs)
- [Funscript export](https://github.com/gamerman420691337-arch/pulsar-fs/blob/7eb6d62b47cae6e8819a5ec685af365cbc684654/src/funscript.rs)

## Appendix B. External integration references

These primary references were consulted during the interview. They are implementation starting points, not evidence that Pulsar's integration has passed qualification.

- [Handy BLE control documentation](https://ohdoki.notion.site/Bluetooth-control-1ee344f4b55d42e5be14d98b001ee504)
- [Handy REST API v3 documentation](https://ohdoki.notion.site/Handy-Rest-API-v3-ea6c47749f854fbcabcc40c729ea6df4)
- [Handy REST API specification](https://www.handyfeeling.com/api/handy-rest/v3/docs/)
- [Handy 2 usage and connection guide](https://www.thehandy.com/handy-2-how-to-guide/)

## Appendix C. Decision ledger

| Area | Final recorded decision |
| --- | --- |
| Primary objective | High-quality, fast, multimodal, broadly compatible, local-AI motion generation and tracking. |
| Distribution | Portable platform artifacts; first-launch extraction allowed. |
| Core | Useful offline immediately; all input modalities at Fast, local assistant, editing/export, offline playback. |
| Complete | All advertised tiers bundled; 20 GB or larger acceptable. |
| Additional models | Optional downloads or offline pack import. |
| Operating systems | Windows, Linux, macOS. |
| Architecture order | x86-64, Apple Silicon, other ARM64, then Intel Macs; Intel Macs not launch-required. |
| GPU vendor order | NVIDIA, AMD, Apple, Intel; all launch-required, including Maximum acceleration. |
| Hardware | 16 GB minimum system RAM; 8-core 10th-gen Intel-class CPU; 3060-class 8 GB reference GPU; stronger GPUs recommended for upper tiers. |
| Stroke speed | Fast >1x on every qualified CPU/GPU path; CPU Default >=0.5x; reference GPU Fast >=10x; reference GPU Default >=5x. |
| Six-axis speed | Offline generation may be slower; stroke-only speed floors do not apply. |
| Live | 200 ms responsive capture-to-command target; optional buffered mode up to one second of added playback delay. |
| Live six-axis | GPU fastest preset required at launch; higher-quality live presets optional; CPU best effort may skip analysis frames. |
| Fidelity | Preserve timing, pauses, shallow motion, and relative amplitude; expansion opt-in. |
| Uncertainty | Best effort plus review flags; observed/inferred/synthetic distinction retained. |
| Automatic selection | Video works without manual target selection; override optional. |
| Multimodal conflict | Clear visual motion wins in faithful mode; audio cannot override it. |
| Still image/text | Image requires prompt; text uses synthetic generation and preset motions/patterns. |
| Output | Six-axis support; stroke most important; neutral master with device profiles and optional adapted exports. |
| Preview tracking | 80% precision and recall floor, 90% target, per category. |
| Stable tracking | 95% precision and recall floor, per category. |
| Material error | >100 ms reversal timing deviation or >20% reference stroke-travel error; wrong/missing/extra strokes also material. |
| Error flags | At least 95% recall of materially incorrect sections. |
| Stable human effort | <=5 minutes active editing and <=10 minutes total review plus editing per 30-minute video for at least 95% of videos in each category. |
| Preview human effort | May exceed Stable budgets; report measured effort. |
| Creative evaluation | Prompt adherence, timing/pattern constraints, correction effort; not one exact reference script. |
| Training strategy | Pretrained comparison first, targeted fine-tuning for measured gaps. |
| Training data | Curate a separate pilot; keep held-out evaluation separate. |
| Pilot budget | Five human-hours total labeling and review; method trial only. |
| Assistant capabilities | Offline natural-language generation/editing, target selection, review-flag explanations, generation-setting changes, and undoable script edits. |
| Assistant contention | Pause/checkpoint offline generation for chat when both models cannot fit; resume afterward; protect live playback. |
| Manual edits | Protected by default during regeneration; replacement explicit; versions undoable. |
| Handy | Platform including Handy 2; online services and optional offline BLE; all input modalities usable. |
| Cloud consent | Generated scripts/commands may use Handy services with explicit opt-in; no silent Bluetooth-to-cloud fallback; source media/prompts stay local. |
| Reconnect | Disarmed until explicit manual resume; stale commands must not restart motion. |
| Six-axis device qualification | Preview may label unqualified playback experimental; Stable requires physical qualification. |

This specification is a durable record of the interview. It is not a claim that the repository, models, benchmarks, or release artifacts already meet the contract.

## Appendix D. Chat-to-requirement traceability

### D.1 Source and interpretation rules

Conversation/task: `01a0855c-2f0e-7b00-8f73-8e3dd1646aa1`.

This matrix maps the product decisions and later clarifications to this specification. Plain-chat sources use stable user-message IDs; menu choices use the question ID, call ID, exact selected label, and response receipt timestamp. Answer excerpts normalize whitespace only. The decision column supplies the preceding question's context, so a short "yes" is not treated as approval of an unrelated topic.

The architecture and packaging exchanges also contained recommendations. A source pointer records their provenance; it does not independently upgrade a D or V item into a confirmed requirement. Requirement status remains defined in Section 1. The six menu selections are actual answers, not inferred acceptance of the preselected recommended option.

Revision 0.2 records the post-audit approval to make the Fast execution-path scope and local assistant capabilities explicit. Traceability records decisions, not successful implementation, independent review, performance measurements, or release authorization.

### D.2 Decision mappings

| Decision | Final decision or disposition | Requirement / plan mapping | Chat source |
| --- | --- | --- | --- |
| DEC-001 | SOTA ambition spans generation, tracking, speed, quality, modalities, compatibility, and local AI; it is not an achieved-performance claim. | GOAL-01 through GOAL-08; Section 16 | `01a08563-f442-7501-8a20-c6c06e0527ba`: `The best funscript generation and tracking, best speed, quality, modality. Best compatibility, local ai support.` |
| DEC-002 | Portable delivery includes baseline local AI and user-ready offline functionality, without user-managed runtime setup. Later extraction/Core decisions clarify delivery. | GOAL-02, GOAL-03; PKG-04, PKG-05; EDIT-01 | `01a08566-ef51-7880-b7ec-9749c487ced5`: `I wish for totally portable binaries, no external deps, you install, you use it perfectly fine. A local AI in the binary, for options 100% of the time.` |
| DEC-003 | Architecture discussion permits a bundled local inference runtime around the product core. Rust/product responsibilities are the recorded direction; no exclusive runtime or model was chosen. | Section 13; PKG-03, PKG-04; MODEL-02 | `01a08779-2fef-7a62-8e44-7d4b2e776b98`: `Would external deps and a thin core around a bundled local inference runtime?`; `01a08779-53f2-7133-a09f-2def17db8aaa`: `Help at all?` |
| DEC-004 | CPU-only operation, optional GPU acceleration, and a 16 GB minimum system-memory baseline. | Section 7.3; MEM-03, MEM-04 | `01a087af-2914-7580-8cbc-0fc5c341b9c1`: `CPU only, GPU acceleration, minimum 16gb ram.` |
| DEC-005 | CPU Default has a 0.5x floor, with roughly 0.5x to 1x expected; faster is allowed. Maximum may take longer, including on CPU. | PERF-02, PERF-05; Section 7.3 | `01a087b1-2bb5-7660-8352-484ca1e04514`: `Equal or slower than realtime on CPU, but not less than half as fast. GPU acceleration should be massively faster than realtime for default and fast, slower than realtime for most demanding and maximal quality.`; `01a087b2-47b9-7d12-aa92-8e4c8e460f87`: `Yes. Fast tiers should always be faster than realtime.` |
| DEC-006 | Fast must exceed realtime on qualified CPU/GPU stroke-only paths. Later six-axis exception remains intact; revision 0.2 makes non-NVIDIA coverage explicit. | PERF-01, PERF-03, PERF-10; TEST-22; DEC-038, DEC-056 | `01a087b2-47b9-7d12-aa92-8e4c8e460f87`: `Yes. Fast tiers should always be faster than realtime.` |
| DEC-007 | Reference CPU is an 8-core Intel 10th-generation-class processor or faster. | Section 7.3; PERF-01, PERF-02 | `01a087b4-2f16-7033-b587-55de1ff73ff4`: `The reference cpu is a modern cpu, an 8 core 10th gen intel or faster. That must handle .5-1x realtime.` |
| DEC-008 | Reference GPU has 3060-class performance and 8 GB VRAM; exact qualifying configuration remains to be recorded. | Section 7.3; PERF-03, PERF-04 | `01a087b6-9608-7420-87d6-08b6f2d5686e`: `8gb gpu is reference, 3060 level. Maximum and upper default tiers recommended for stronger gpus, like 4090.` |
| DEC-009 | Upper Default and Maximum recommend stronger GPUs such as a 4090-class device; this is not a promise that every model fits every reference GPU. | Section 7.3; PERF-05; PERF-09 | `01a087b6-9608-7420-87d6-08b6f2d5686e`: `8gb gpu is reference, 3060 level. Maximum and upper default tiers recommended for stronger gpus, like 4090.` |
| DEC-010 | Accept reference-GPU minimums of 5x Default and 10x Fast, measured end-to-end. | PERF-03, PERF-04; TEST-04 | `01a087b7-6504-75b1-a95b-aa4931256ba8`: `Yep.` |
| DEC-011 | Default preserves timing, pauses, shallow movements, and relative amplitude. Full-range intensity expansion is opt-in. | GOAL-05; GEN-07; TEST-06 | `01a087b9-5ffc-7d41-8b4d-f79c0c75f78b`: `Yep.` |
| DEC-012 | Attempt best-effort motion and flag uncertainty instead of routinely abstaining or mandating a hold. Preserve observed/inferred distinctions. | GOAL-06; GEN-03, GEN-06; QUAL-08; TEST-07 | `01a087ba-70cd-7022-b066-bed274388062`: `It should flag sections for review, but attempt best effort.` |
| DEC-013 | Input scope includes all discussed media families, not only ordinary video or one VR layout; later image/text decisions add explicit authoring behavior. | IN-01 through IN-08; IN-10; DEC-018, DEC-019 | `01a087bd-0a99-7a61-a61b-fd563c41685d`: `All modalities.` |
| DEC-014 | Launch includes live camera/stream generation with synchronized device output. | IN-08; LIVE-01; TEST-11, TEST-12 | `01a087bf-32a1-7c32-9481-34996f1341a0`: `Yes.` |
| DEC-015 | Responsive live target is 200 ms capture-to-command, distinct from physical device response and from motion-fidelity timing error. | LIVE-02, LIVE-09; QUAL-01 | `01a087c1-dbf8-7921-ac12-9c892ee60d24`: `200ms seems solid. Maybe more? We can sync better.` |
| DEC-016 | Optional buffered-quality live mode may add up to one second of playback delay. Shared-clock A/V/motion synchronization and lag compensation accompany that design. | LIVE-03, LIVE-04; LIVE-09 | `01a087c3-0072-7590-98a8-407e59f84b63`: `Yep.` |
| DEC-017 | Use curated benchmarks, test scripts, and hand-curated references; distinguish fidelity annotations from subjective script preferences. | GOAL-07; Section 9; DATA-07 through DATA-09 | `01a087c5-c4ba-7a31-8d57-83edeb12b556`: `Curated benchmark, we will use test scripts and hand curated.` |
| DEC-018 | Bundled local AI supports natural-language generation/editing, and generation is multimodal. Target selection, flag explanations, and settings control are explicit in revision 0.2. | IN-01 through IN-08; EDIT-01; TEST-23; DEC-056 | `01a087c8-51a7-79f1-aa9f-15b247542659`: `Yes. The generator should be multimodal and able to handle all types of media.` |
| DEC-019 | Still images require an accompanying text prompt. Text uses synthetic generation, premade motions, presets, and patterns. | IN-05, IN-06; GEN-06; CREATIVE-01; TEST-09 | `01a087cd-1de8-7c10-91e3-64ed7af0aabb`: `Still image requires text prompt, text prompt uses synthetic generation and premade preset motions, patterns.` |
| DEC-020 | Represent relative motion, map through device presets, and preserve timing while adapting amplitude to device limits rather than stretching every stroke to full range. | MOT-03 through MOT-05; TEST-10 | `01a087d0-5b48-7ef2-af06-043526645c97`: `Would a relative motion that is scaled to device presets be better? Also, we can simply preserve timing, adjust amplitude to make within device limits.`; `01a087d3-6598-7c83-ab6e-976c0c367819`: `Yep. Device specific profiles and master neutral funscript export` |
| DEC-021 | Keep ordinary master funscript export device-neutral; use device-specific playback profiles and separately identified adapted exports. | MOT-02, MOT-05, MOT-06; TEST-10 | `01a087d3-6598-7c83-ab6e-976c0c367819`: `Yep. Device specific profiles and master neutral funscript export` |
| DEC-022 | Support Windows, Linux, and macOS; Windows single EXE and Linux AppImage are delivery targets. Specific macOS packaging remains a design default. | Section 6.2; Section 7.1; PKG-07 | `01a087d8-5698-7461-b50c-f078dca8cb29`: `Windows, linux, macos. One exe, appimage` |
| DEC-023 | First launch may extract bundled runtimes and models; it must not substitute mandatory downloads for bundled offline functionality. | PKG-03, PKG-05; TEST-01, TEST-02 | `01a087e8-0747-7e43-a5a1-04ec0d46953a`: `First launch extracts bundled runtimes and models.` |
| DEC-024 | Architecture priority: Windows/Linux x86-64, Apple Silicon, other ARM64, then Intel Macs. Intel Macs are not launch-required; other ARM64 timing was not fixed. | Section 7.1; OPEN-04 | `01a087e9-a2ab-7a51-9d2e-6e19965bb1df`: `Not at launch. X86-64, apple, arm, then intel mac` |
| DEC-025 | NVIDIA, AMD, Apple, and Intel GPU support are all required at launch, in that priority order. | HW-01, HW-02; TEST-05 | `01a087ea-7213-7171-bf8c-96d3c5d50149`: `Nvidia, AMD, Apple, then Intel gpu support from launch.` |
| DEC-026 | Maximum also requires genuine GPU acceleration across all four vendors; CPU fallback alone does not satisfy that requirement. | HW-02, HW-03; TEST-05 | `01a087eb-2410-7763-a772-5d6e3db4aad5`: `Yes.` |
| DEC-027 | Default active-editing budget is five minutes per 30-minute benchmark video; later release staging makes this a Stable gate. | Section 9.2; QUAL-07; TEST-19 | `01a087f0-e8cf-79b0-9385-a0e0578a0023`: `5 minute active editing is solid.` |
| DEC-028 | Five-minute editing limit must hold for at least 95% of videos within each benchmark category, not merely on average. | Section 9.2; QUAL-09, QUAL-10; TEST-19 | `01a087f2-46db-7e10-bb0b-02f2099fd473`: `yeah.` |
| DEC-029 | Total review plus editing must be at most ten minutes per 30-minute video under the same 95%-per-category rule; review and active editing are separate. | Section 9.2; QUAL-07; TEST-19 | `01a087f3-3a53-7cc2-811d-e17fd2425f92`: `yeah.` |
| DEC-030 | Flags must catch at least 95% of independently annotated materially incorrect sections, including wrong targets and missed/extra strokes. | QUAL-03, QUAL-04; Section 9.2; TEST-19 | `01a08804-a879-7112-8e14-a55045624198`: `95% of materially incorrect sections.` |
| DEC-031 | Reversal timing deviation over 100 ms is a material error. The 200 ms live-processing target measures something different. | QUAL-01, QUAL-06; LIVE-02 | `01a08808-2763-7c32-8904-bbc187fc08c9`: `yep. 100ms deviation` |
| DEC-032 | Reference stroke-travel error over 20% is material; evaluate device-neutral motion, with missed shallow strokes counted separately. | QUAL-02, QUAL-03, QUAL-05, QUAL-06 | `01a0880e-a7d8-71f2-b07b-9c54713e7528`: `20% error is good` |
| DEC-033 | Initial 95% ideal was refined to Preview 80% floor/90% target and Stable 95% floor, separately for precision and recall in each category; timing/amplitude tolerances remain. | Section 1.1; Section 9.2; QUAL-01, QUAL-02, QUAL-11 | `01a08817-8cb5-7030-9b5b-38ce14ebe3fa`: `95% pre-edit accuracy ideally`; `01a0881d-bf6c-7241-8fc6-59cca115a17c`: `Maybe we can lower it to 80-90% accuracy for the first release? 95% for the stable one.`; `01a0881e-dca6-7d52-8749-d7d057f96722`: `yep.` |
| DEC-034 | Preview may exceed the human-effort budgets if actual effort is reported; Stable retains those gates. | Section 1.1; Section 9.2; Stage 3, Stage 4 | `01a08820-7de8-7400-aad4-0b4ca4106887`: `yep.` |
| DEC-035 | Video must generate without manual target selection; clicking a target or adding a prompt is an optional override. | GEN-01, GEN-02, GEN-08; EDIT-01, EDIT-03 | `01a0882d-6cce-70c3-af0a-c649a5b47a0f`: `Yes.` |
| DEC-036 | Audio must not override clearly visible motion in faithful mode; reinterpretation requires explicit creative intent. | GEN-04, GEN-05; TEST-08 | `01a0882f-00e6-7943-b1ac-9a66d5364203`: `no.` |
| DEC-037 | Six-axis motion is required, with stroke the most important axis; this does not implicitly approve vibration or suction as launch outputs. | GOAL-04; MOT-01, MOT-08; SIX-01, SIX-02 | `01a08832-ce4c-7101-aa4a-beeece232045`: `6 axis motion support with stroke as most important axis.` |
| DEC-038 | Agreed offline speed floors apply only to stroke-only generation. Six-axis generation may be slower. | PERF-06, PERF-10; Section 1.1 | `01a08836-f6bb-7cb1-bbde-a5f081eeeb51`: `No. Speed floors are for stroke only. 6 dof can be slower.` |
| DEC-039 | Live must deliver all six axes at launch on the fastest preset. The original word 'axis' was immediately corrected to 'preset'. | LIVE-05; Section 1.1; TEST-11 | `01a08838-7f67-79a3-ba4b-08321438374b`: `yes. But only needs to do this on fastest axis.`; `01a08838-c4aa-71d2-ae33-9160286c4814`: `fastest *\*preset \* I mean.*` |
| DEC-040 | CPU live six-axis may be much slower and has no agreed live latency gate; GPU fastest-preset live targets remain. | LIVE-05, LIVE-06; TEST-11, TEST-12 | `01a08839-acb5-71f3-a7be-fb818541a7fe`: `No. It can be much slower.` |
| DEC-041 | CPU live six-axis may skip analysis frames, lower cadence, and use a bounded latest-frame queue; show actual delay rather than accumulate growing backlog. | LIVE-07, LIVE-08; TEST-12 | `01a0883b-6eff-7df1-88ad-b90d35530a42`: `yes.` |
| DEC-042 | Every source/device reconnect leaves playback disarmed until explicit manual resume; stale queued commands must not restart motion. | SAFE-01, SAFE-02; TEST-18 | `01a08841-fe2a-7a23-bdeb-df34af3f8ed7`: `yes` |
| DEC-043 | Handy platform, including Handy 2, is the identified launch stroke-device family. No six-axis reference device was named. | DEV-01, DEV-07; OPEN-09, OPEN-10 | `01a0884d-6e24-7af2-829b-e18921b024f5`: `the handy platform, including handy 2.` |
| DEC-044 | Handy workflows support all input modalities, with offline Bluetooth playback as an option rather than the sole transport. | DEV-02, DEV-03; TEST-17 | `01a08852-1a24-7ba0-9ae6-bcb7d126136c`: `All modalities for handy integration, with offline bluetooth playback as an option.` |
| DEC-045 | Online Handy-server use is allowed alongside offline BLE, with explicit script/command upload consent, local source media/prompts, and no silent BLE-to-cloud switch. | DEV-03, DEV-04; SEC-01, SEC-02; TEST-17 | `01a08866-da29-73f0-a31d-a9786a60b258`: `yes. It can do offline or online with handy serverse.` |
| DEC-046 | Preview may ship hardware-unqualified six-axis playback labeled experimental. Stable requires real physical qualification. | DEV-07 through DEV-09; TEST-20; Stage 3, Stage 4 | `01a0895f-1d31-7ea3-af75-dd0dfb62712d`: `Yes` |
| DEC-047 | A Complete bundle of 20 GB or more is acceptable; a lightweight Core edition is desired. This is not a 20 GB cap or a RAM budget. | PKG-01, PKG-02; Section 6.1; MEM-03 | `01a08ab0-9c71-7360-ad57-35095a27be28`: `20gb or more, lightweight core perhaps?` |
| DEC-048 | The smaller Core must itself be portable and offline-capable, not a download stub. The packaging exchange includes versioned packs, offline import, and component updates. | Section 6.1; PKG-03 through PKG-06; TEST-01, TEST-21 | `01a08b05-eff7-7fa3-8f1a-4594e5eaf9df`: `yep. Portable, offline core.` |
| DEC-049 | Core's bundled Fast tier must cover every required input family offline without adding packs, alongside the local assistant/editor/export/playback. | IN-09; Section 6.1; EDIT-01; TEST-01 | `01a08b07-87ed-7711-9e02-86a99fe09910`: `Yep.` |
| DEC-050 | Benchmark pretrained candidates first; targeted fine-tuning is allowed for measured gaps using separate training data. | DATA-01, DATA-02; MODEL-01, MODEL-02; Stage 1 | Question `launch_model_training_strategy`; call `call_HHGYXnRDARgojPclpXlI9AFh`; answer `Tune where needed (Recommended)`; receipt `2026-09-10T11:20:11.060Z`. |
| DEC-051 | Build a small permitted, curated training pilot and expand for measured failure cases, separate from held-out evaluation. | DATA-02, DATA-03; DATA-07, DATA-10; Stage 0 | Question `training_data_start`; call `call_QWojtrLsexU1rFCJIWCYG2jm`; answer `Curate pilot (Recommended)`; receipt `2026-09-10T21:03:15.297Z`. |
| DEC-052 | Spend five human-hours total on pilot labeling and review before reassessing; this is a limited annotation-method trial, not release qualification. | DATA-04, DATA-05, DATA-11; QUAL-10; Stage 0 | Question `pilot_labeling_budget`; call `call_fqHZ88svMvfYNBvyJeytKfpN`; answer `5 hours`; receipt `2026-09-10T21:13:56.418Z`. |
| DEC-053 | On memory contention, pause/checkpoint offline generation for assistant chat and then resume. Protect live device playback. | MEM-01, MEM-02; TEST-13 | Question `assistant_memory_contention`; call `call_xQLP2c5D0J4JzBX7NUupZDiC`; answer `Pause generation for chat (Recommended)`; receipt `2026-09-10T21:19:11.686Z`. |
| DEC-054 | Protect manual edits by default, regenerate unlocked regions, keep undoable versions, and require explicit replacement of protected edits. | EDIT-02; TEST-14 | Question `regeneration_manual_edits`; call `call_LI0gElr5ON6xNuZU6wwjRj63`; answer `Protect edits (Recommended)`; receipt `2026-09-10T21:19:33.711Z`. |
| DEC-055 | Use a creative rubric for synthetic text/image output: prompt adherence, timing/pattern constraints, and correction effort. Keep stroke precision/recall for faithful tracking. | CREATIVE-01 through CREATIVE-04; TEST-09 | Question `creative_output_evaluation`; call `call_WZBc2yLpOvaDbBUPvzHmwHlD`; answer `Creative rubric (Recommended)`; receipt `2026-09-10T21:20:17.029Z`. |
| DEC-056 | Authorize the post-audit clarification of Fast speed coverage and assistant capabilities, plus decision-source mappings. This is a documentation change, not implementation or release approval. | PERF-10; EDIT-01; TEST-22, TEST-23; Appendix D | Current post-audit reply `yep.` to the request to tighten these requirements and add decision mappings; task turn `01a08e14-e964-7ec1-a7f2-a9b2e4bf45c4`. |

### D.3 Superseded statements and rejected alternatives

| Earlier statement or offered alternative | Final disposition | Trace |
| --- | --- | --- |
| Literal single-file operation without extracted assets | Single-file distribution may extract bundled runtimes/models on first launch; no mandatory network bootstrap. | DEC-002, DEC-022, DEC-023 |
| A smaller core that fetches the baseline models before it becomes useful | Not the selected Core contract. Core works offline immediately and includes all Fast input families. | DEC-047 through DEC-049 |
| A general 0.5x to 1x CPU processing expectation | Default has the 0.5x floor; Fast must exceed 1x; Maximum may be slower. Faster Default performance is allowed. | DEC-005, DEC-006 |
| Applying the agreed offline speed gates to full six-axis generation | Explicitly rejected. Gates apply to stroke-only output. | DEC-038 |
| Live output only on the "fastest axis" | Typo corrected immediately to "fastest preset"; the required output remains six-axis. | DEC-039 |
| Requiring CPU six-axis live to meet the GPU latency target | Rejected. CPU is best effort with bounded queues and permitted analysis-frame skipping. | DEC-040, DEC-041 |
| Universal 95% first-release tracking floor | Refined to Preview 80% floor/90% target and Stable 95% floor, per category for both precision and recall. | DEC-033 |
| Five-minute editing and ten-minute total effort as Preview blockers | Preview may exceed and report them; Stable retains both gates. No corresponding relaxation of 95% material-error flag recall was approved. | DEC-027 through DEC-034 |
| Routine abstention or a mandatory hold whenever tracking confidence falls | Replaced by best-effort generation with review flags and honest uncertainty. Disconnect disarming still applies. | DEC-012, DEC-042 |
| Audio overriding clearly visible motion in faithful mode | Rejected. Explicit creative reinterpretation is a separate workflow. | DEC-036 |
| NVIDIA/Apple acceleration first, with CPU fallback sufficient for AMD/Intel or Maximum at launch | Rejected as launch scope. All four vendors, including Maximum acceleration, are required on qualified hardware. | DEC-025, DEC-026 |
| Intel Macs as a launch requirement | Not required at launch. Other ARM64 remains higher priority, with its exact milestone open. | DEC-024 |
| Five human-hours interpreted as five hours of video or as a qualified training corpus | Incorrect. Budget is total human labeling/review effort for an annotation-method pilot. The offered 20-hour and 40-hour budgets were not selected. | DEC-052 |
| Queue assistant chat behind memory-heavy offline generation | Not selected. Pause/checkpoint generation, serve chat, and resume without stalling live playback. | DEC-053 |
| Replace manual work by default or require a fresh complete alternative for every regeneration | Not selected. Preserve protected regions and regenerate unlocked regions with undoable versions. | DEC-054 |
| One prescribed reference script as the universal synthetic-output answer | Not selected. Creative rubric allows valid variation while retaining requested constraints. | DEC-055 |
| Hardware-unqualified six-axis playback treated as Stable-qualified | Preview-only experimental allowance; Stable requires physical qualification. | DEC-046 |

### D.4 Scope boundaries and remaining open items

- Questions about the agent's outside-project context, requests to inspect the repository, and prompts to continue or stop the interview are conversation/process instructions, not Pulsar product requirements.
- Repeated affirmations without a device name do not nominate additional hardware. The explicit Handy-family answer establishes the stroke reference; a six-axis reference remains unselected.
- Model/runtime identities, exact non-NVIDIA reference hardware, minimum OS versions, other ARM64 release timing, Core-size and peak-memory caps, detailed creative/secondary-axis quality thresholds, and statistical qualification rules remain open in Section 17.
- The suggested 2x CPU Fast engineering target, 12 GB application-memory cap, macOS DMG format, and tighter rapid-motion phase thresholds were not independently approved as hard numeric or packaging requirements.
- Historical repository findings remain in Appendix A and Stage 0. They are not newly verified current-state facts or completed remediation.
- Future decisions should append a source-backed row and update the affected requirement. Superseded decisions should retain their source and explicit replacement rather than disappearing from the record.
