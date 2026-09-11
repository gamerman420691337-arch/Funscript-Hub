# Pulsar Architecture Decisions and Design Continuation

Version: 0.3
Date: 2026-09-11
Status: Architecture discussion complete; implementation and qualification unfinished

## 1. Purpose, authority, and current work status

This document preserves the architecture discussion following the product interview, including explicit decisions, the inspected source structure, proposed module responsibilities, verification goals, and questions still to resolve. It is the continuation record for further interface design, not an implementation-complete specification.

Read these documents together:

- [Software specification](PULSAR_SOFTWARE_SPECIFICATION.md): existing product requirements and the earlier interview's decision traceability, including modalities, quality/speed, platforms, packaging, assistant, editing, and devices.
- [Major roadmap](PULSAR_MAJOR_ROADMAP.md): M1-M8 sequencing and evidence-bound milestone closure.
- This record: subsequent architecture decisions and unresolved interface design. The explicitly selected decisions below extend the earlier architecture direction; proposals do not become requirements merely by appearing here.

The product specification remains authoritative for existing product requirements. Subsequent explicit architecture choices are recorded here rather than silently rewriting the older specification. Any actual conflict must be surfaced to the product owner before implementation; numeric product gates are not redefined here.

Status labels:

| Label | Meaning |
| --- | --- |
| Confirmed | Explicit user statement or submitted structured choice. |
| Proposed | Engineering recommendation discussed but not independently finalized. |
| Observed | Dated source or test evidence; not proof of release qualification. |
| Open | Further design, investigation, or human decision required. |

The user requested a full bug, architectural purity, and correctness pass, but first requested discussion of actual source structure and joint design of interfaces. Implementation was paused for that design. This documentation write does not resume implementation, authorize device actuation, or declare any milestone complete.

The user explicitly requested durable documentation and an explicit statement when architecture and planning are finished. Completion criteria appear in Section 11. Until they are satisfied, report planning as incomplete.

## 2. Confirmed user direction beyond the menu choices

- Modernize legacy and misleading names. `FunGenApp` prompted a source-origin concern; changing a name alone neither proves nor disproves implementation origin.
- Harden the design architecturally and technologically, including stronger types and explicit interfaces.
- Preserve provenance across interfaces where needed and remove unnecessary detail deliberately when no longer needed.
- Make modules independently programmable and testable through their interfaces, while checking whole-program composition.
- Include mutation testing, known-answer tests (KATs), fuzzing, and additional verification techniques.
- Seek rigorous correctness assurance, with the formal scope subsequently narrowed by the explicit critical-path decision below.
- Discuss and settle architecture before undertaking the requested broad implementation/bug pass.

These are design objectives, not assertions that the existing implementation already provides them.

## 3. Confirmed architecture decision ledger

Rows ARCH-001 through ARCH-013 retain the exact structured question IDs used in the architecture interview. Subsequent rows identify their follow-up question or direct user statement. Only submitted answers are treated as decisions; no decision is inferred from a preselected option.

| ID | Decision source | Submitted answer | Confirmed direction |
| --- | --- | --- | --- |
| ARCH-001 | `generation_process_isolation` | Worker processes (Recommended) | Decoding and inference execute in worker processes. GUI and CLI share one typed application interface. Worker launch remains compatible with the portable distribution. |
| ARCH-002 | `headless_engine_lifetime` | Continue headlessly (Recommended) | A headless engine owns jobs and project state independently of the GUI. Offline generation continues after GUI closure/crash, and GUI or CLI can reconnect. Engine crash recovery details remain open. |
| ARCH-003 | `media_editor_scope` | Motion + analysis (Recommended) | Edit motion and non-destructive media trims, crops, regions, and timing alignment. Preserve original media. A full video/audio composition-and-export editor was not selected. |
| ARCH-004 | `source_lineage_strategy` | Audit then refactor (Recommended) | Establish code/dependency origins and attribution, then retain or replace modules based on findings. Do not claim clean-room status without evidence. An independent whole-program reimplementation was not selected. |
| ARCH-005 | `provenance_retention_default` | Compact audit graph (Recommended) | Native projects retain source/model/configuration/transform lineage and traceable observations for generated motion and review flags. Heavy raw/intermediate traces are opt-in. Exact schema, granularity, retention, and size budgets remain open. |
| ARCH-006 | `assistant_edit_autonomy` | Scoped automatic edits (Recommended) | Permit undoable assistant edits within authorized, unprotected project scope. Protected replacements, external effects, and device actions require separate authorization. Grant lifetime and detailed permissions remain open. |
| ARCH-007 | `playback_controller_loss` | Explicit headless mode (Recommended) | Loss of the controlling GUI/CLI stops physical playback by default. Continuing requires explicit prior headless-session authorization. This is separate from permission to continue offline generation. |
| ARCH-008 | `formal_assurance_depth` | Critical-path proofs (Recommended) | Require formal models/proofs for authorization, revision transactions, job/device lifecycles, and key invariants. Use strong numerical and empirical testing elsewhere. A proof-first rewrite of the entire deterministic core was not selected. |
| ARCH-009 | `extension_execution_scope` | Sandboxed executable plugins | Include third-party executable plugins in the initial hardened architecture. Do not defer executable extensions in favor of built-ins and data packs only. |
| ARCH-010 | `initial_plugin_roles` | Also runtimes and drivers | Plugin roles include third-party inference backends and device-protocol implementations, not only analysis and authoring. Different roles need distinct trust and admission contracts. |
| ARCH-011 | `third_party_driver_qualification` | Qualification gate (Recommended) | Normal playback requires accepted driver/version qualification before real device-control authority. Development and physical qualification use a separate explicitly authorized test mode. Unqualified experimental playback was not selected as the normal alternative. |
| ARCH-012 | `engine_protocol_support` | Versioned local protocol (Recommended) | Support a versioned local engine protocol for third-party clients as well as bundled GUI/CLI. Define command/event schemas and compatibility rules; enforce capabilities in the engine. This does not authorize a remotely exposed control server. |
| ARCH-013 | `project_revision_conflicts` | Reject stale commits (Recommended) | Project mutations require the matching project revision. Preserve stale generation/edit results as candidates for explicit review/rebase instead of overwriting newer work. Automatic proven-disjoint merging was not selected. |
| ARCH-014 | Follow-up question: one master timeline versus multiple independent timelines per project | "One master timeline, yep." | Each project has one master motion timeline, with aligned media/analysis inputs, motion axes, and one revision history. Libraries and batches coordinate multiple projects rather than introducing independent timelines inside a project. Concrete timebase and alignment contracts are finalized in CONTRACT-02 and CONTRACT-04 below. |
| ARCH-015 | Follow-up question: one engine per OS user versus one engine per project | "one engine per os user" | One headless engine per OS user coordinates projects, shared compute budgets, and device ownership. Project permissions, revisions, and assistant contexts remain separately scoped. Decoding/inference workers remain isolated; engine sharing does not grant cross-project authority. Startup, discovery, scheduling, and recovery contracts are finalized in CONTRACT-05 below. |
| ARCH-016 | `project_time_authority` | "Independent project time" | Each project has an independent time domain. Source timestamps map onto project time; project time is not defined by a primary video. Playback scheduling uses a separate monotonic clock. Concrete units, mappings, discontinuities, and rounding contracts are finalized in CONTRACT-02 below. |
| ARCH-017 | `source_alignment_motion_policy` | "Explicit retime" | Source alignment changes preserve existing motion and flag affected derived results as stale. Retiming or regeneration requires an explicit operation. |
| ARCH-018 | `project_edit_durability` | "Durable commits" | Persist each committed edit transaction before reporting success. Interactive previews remain transient. |
| ARCH-019 | `project_undo_scope` | "Latest project edit" | One shared, actor-labelled project undo history. Undo reverses the latest committed edit regardless of originating client. |
| ARCH-020 | `client_playback_scope` | "Independent previews" | Clients have independent preview playheads over the same project timeline. Clients can attach to engine-owned device playback, which has one exclusive controller. |
| ARCH-021 | `running_job_revision_change` | "Finish snapshot job" | Project edits do not automatically cancel running generation. Jobs finish against immutable input snapshots; old-revision results remain candidates for explicit review/rebase and cannot silently commit over newer edits. |
| ARCH-022 | `interrupted_offline_job_recovery` | "Resume eligible jobs" | When the engine next starts after a crash, automatically resume eligible interrupted offline generation from validated checkpoints with unchanged inputs/configuration. Missing or changed dependencies block recovery. This does not authorize OS-login autostart or device playback. |
| ARCH-023 | `third_party_client_authority` | "Pair and scope" | Third-party local clients require explicit pairing and grants scoped to projects and operations. Device and external-effect permissions remain separate. This is not protection against a fully compromised same-user OS account. |
| ARCH-024 | `plugin_isolation_failure_policy` | "Allow trusted exception" | If required confinement is unavailable, the user may explicitly authorize unsandboxed third-party plugin execution. Warn that access may exceed project grants; the assistant cannot enable this exception. Such plugins enter the trusted computing base and are excluded from sandbox guarantees. Driver qualification remains separately required. |
| ARCH-025 | `unsandboxed_plugin_approval_scope` | "Exact build" | Unsandboxed plugin approval is bound to the exact artifact identity/hash. Changed plugin bytes require fresh user consent. Driver qualification remains a separate build-specific gate. |
| ARCH-026 | `headless_playback_stop_assurance` | "Require bounded stop" | Explicitly authorized headless device playback requires qualified device-side behavior that bounds residual motion after engine or connection failure. Unsupported device/transport modes remain attended-only. A stop command alone does not establish physical stopping. Acceptable deadlines follow the per-device approval policy. |
| ARCH-027 | `headless_stop_deadline_policy` | "Per-device approval" | Each qualified device/firmware/transport configuration declares its failure-stop bound. Explicit user approval of that bound is required before headless use; no universal numeric deadline is selected. Qualification and approval must correspond to the configuration actually used. |
| ARCH-028 | `first_release_motion_output_cardinality` | "One program initially" | First release supports one device-neutral motion program per project, with up to six axes. Multiple sources/entities may inform it. Explicit motion-target and motion-output identities preserve future expansion. This does not decide simultaneous physical-device playback. |
| ARCH-029 | `viewer_uncached_observation_behavior` | "On-demand preview" | Viewer uses frame-matched cached observations, otherwise requests lightweight preview analysis. Stale boxes remain hidden until results match the displayed frame and current source/mapping context. Rendering remains independent of generation completion. |
| ARCH-030 | `job_media_snapshot_policy` | "Snapshot media" | Generation uses immutable engine-managed media snapshots. Use copy-on-write snapshots when supported, otherwise copies. Preparation time and storage are explicit; jobs block when required snapshots cannot fit approved disk budgets. Snapshot identity is content-bound, not merely a mutable source path. |
| ARCH-031 | `completed_media_snapshot_retention` | "Evict with explicit pinning" | After active work no longer requires them, unpinned media snapshots may be evicted. Preserve committed motion, compact lineage, and source content identities. Exact reruns then require matching originals or restored snapshots. Users may pin/package projects for self-contained replay. Do not treat original media or committed project state as evictable cache. |
| ARCH-032 | `capability_revocation_running_work` | "Stop affected work" | Revoking a client/plugin grant blocks new commands, cancels work authorized solely by it, and withdraws associated device-session authority with stop/disarm handling. Preserve committed edits/history. Late worker results cannot regain revoked authority. This differs from ordinary client disconnection. |
| ARCH-033 | `automatic_backend_failure_policy` | "Equivalent fallback only" | When an automatically selected backend fails, a new recorded attempt may use a qualified equivalent that preserves requested quality tier. Disclose backend/performance changes; never silently lower tier or reuse incompatible checkpoints. Explicit backend pins remain strict. Existing crash-resume identity requirements still apply. |
| ARCH-034 | `unqualified_model_execution_scope` | "Allow labelled custom runs" | Structurally validated user-imported models/backends may run in ordinary projects with prominent unqualified status and no official speed/quality claims. Such imports do not authorize executable custom code, broaden grants, bypass confinement policy, or bypass driver qualification. Qualified built-in tier selection cannot silently become an unqualified custom run. |
| ARCH-035 | `driver_qualification_acceptance_authority` | "Official or local qualification" | Normal playback may use approved bundled qualification records or explicit user acceptance after required local qualification. Bind acceptance/evidence to exact driver artifact, device, firmware, and transport configuration; required evidence remains mandatory. Assistant execution of checks does not authorize acceptance. Headless stopping-bound approval remains separately required. |
| ARCH-036 | `formal_gate_release_stage` | "Gate Preview" | First public Preview requires the defined critical-path lifecycle/model-checking and bounded Rust proof obligations to pass alongside tests. Proof bounds, assumptions, implementation correspondence, and trusted components remain explicit. This does not certify neural quality or physical hardware. |
| ARCH-037 | `assurance_campaign_cadence` | "Layered gates" | Run fast contract/KAT/property and targeted critical proof checks per PR. Run deeper fuzz, mutation, backend, and hardware campaigns for affected surfaces and release candidates. Results must bind to their actual subjects/configurations; successful fast gates do not waive required deeper gates. |
| ARCH-038 | `legacy_public_interface_migration` | "Compatibility window" | Modernize internal names now while preserving existing documented CLI syntax/aliases and non-destructive project import/migration through the first Stable major series. Remove deprecated public interfaces only in a subsequent major with a migration path. Preserve standard funscript interoperability permanently; compatibility does not preserve known defects. |
| ARCH-039 | `active_playback_revision_updates` | "Explicit switch" | Physical playback remains pinned to its admitted motion revision. Ordinary project edits do not alter active hardware commands. A controller with device authority must explicitly request a revision switch; re-admit the new motion and stop/rearm when the transport cannot switch safely. Authorized live-input generation is a distinct session mode, not implicit permission for ordinary edits. |
| ARCH-040 | `assistant_unsandboxed_extension_invocation` | "Approve each invocation default with a second explicitly risky auto mode/standing grant." | Exact-build load approval does not automatically permit assistant invocation. Default requires approval for each proposed assistant call. The user may separately enable a clearly labelled risky automatic mode/standing grant for that exact build and declared call scope. Assistant cannot enable the exception or risky mode; revocation and changed plugin bytes invalidate authority. Native effects remain outside sandbox guarantees. |
| ARCH-041 | `architecture_first_delivery_sequence` | "First we fix the architecture completely into the final shape, scaffolding everything. Then we make it good, then we make it solid." | Supersedes the prior M1-before-architecture delivery order. Phase A establishes the final four-library ownership/dependency shape and real composed paths; Phase B solves functional/technical challenges through evidence-led research and correctness work; Phase C completes hardening and qualification. Preserve M1-M8 acceptance scope, historical defects, product requirements and human authority. ADR decisions link reproducible experiment evidence; structural integration is not qualification. |

No menu choice selected a particular inference runtime, sandbox technology, IPC serialization, formal verification tool, native project schema, or exact crate layout.

## 4. Original module proposal and how discussion refined it

The user's proposed starting structure was:

1. CLI/orchestration, with GUI conceived as a visual extension of a CLI running in a background terminal.
2. Media input and parser.
3. Media viewer.
4. Media editor.
5. DSP: general-purpose DSP, optical flow/Lucas-Kanade, and neural imperative/functional work including runtime, model parsers/detectors, model zoo, and bounded local chat assistant.
6. Hardware validation doctor.
7. Internal test suite.

The discussion retained these capabilities but proposed different ownership:

- GUI and CLI should be peers using the engine interface, not a terminal screen-scraping relationship. Worker isolation and a reconnectable headless engine were explicitly selected.
- Separate media parsing/decoding from viewing, project editing, and playback-clock coordination.
- Separate numerical DSP, perception semantics, model execution, model artifact management, and assistant authority. A single DSP module should not own all of them.
- Add explicit device-command admission and session ownership. A diagnostic doctor cannot substitute for policy enforced on every output path.
- Keep the core motion/project domain shared by editing, processing, playback, and extensions.
- Treat tests as consumers of module interfaces and composition contracts, not as another production-state owner.
- Add an Extensions module because executable plugins, including runtimes and drivers, were explicitly selected.

These logical responsibilities guide the next discussion. Exact module/crate boundaries and interfaces are still open.

## 5. Observed source structure and defects motivating the design

### 5.1 Inspection baseline and limits

Inspected upstream: `7a4f5671c14731b238b64c4827294ce029928dda`.

Roadmap/specification commit: `43260c3165fcf9ddd5b961f0fb58d320268c0265` on `codex/pulsar-roadmap-m1`, in a separate worktree. The older divergent checkout at `434b830c2c7d004d04830d299e8c5159d43b047e` was preserved.

The earlier remote refresh, including the check after the user requested the latest pushed version, still resolved `origin/main` to the inspected baseline. This is historical evidence, not a permanent claim about current upstream.

The inspected package is Rust `pulsar` version `0.8.0`, with a binary entry point in `src/main.rs` and no separate library entry point in the inspected tree. Its manifest declares `AGPL-3.0-only`; no licensing change is authorized by this record.

Source mapping was targeted, not an exhaustive call-graph or runtime audit. Some larger modules were inspected at signature level. Absence of a safeguard from an inspected interface does not prove that no caller implements it elsewhere. These observations were not revalidated for this documentation-only write.

### 5.2 Current modules and interfaces

| Current source | Observed responsibility and interface |
| --- | --- |
| `src/main.rs` | CLI and generation orchestration; directly configures media, flow/neural work, action extraction, multi-axis assembly, and export. |
| `src/gui/app.rs` | `FunGenApp`, application/presentation state, editing/playback integration, Cinema rendering, and a separate background generation loop. `WorkerMessage` carries progress/status or a completed `MultiAxisScript`, not tracking boxes. |
| `src/gui/timeline.rs` | Timeline state, coordinate conversion, selection, zoom/pan, loops, bookmarks, and timeline rendering tied to egui and funscript types. |
| `src/gui/live_recorder.rs` | Arming, recording/simplifying takes, and splicing recorded actions into a script. |
| `src/gui/rig_simulator.rs` | Presentation of rig geometry using kinematics types. |
| `src/video.rs` | Media probing, extraction and streaming, timestamps, RGB/gray conversion, and playback-related helpers. Exposes `FrameStreamReader`, `StreamConfig`, and `extract_frame_at`. |
| `src/audio.rs`, `src/audio/spectral.rs` | Audio-related implementation exists; detailed ownership and all call sites remain to be mapped. |
| `src/tracking.rs` | Dense flow, scene-cut/motion-center calculations and projections; exposes `FlowField`, `FlowScratchContext`, and dense-flow/projection functions. |
| `src/neural/mod.rs` | ONNX session construction and `NeuralDetector` entry points. |
| `src/neural/yolo.rs`, `src/neural/yolo26.rs` | Preprocessing, legacy/direct tensor decoding, detection types, layout/configuration choices, and related mask assembly. Family names alone do not establish compatibility with actual model exports. |
| `src/neural/tracker.rs` | `AnatomicalTracker`, `TrackedPoint`, `TemporalMemoryBank`, detection selection, occlusion state, and pose estimation. |
| `src/neural/point_tracker.rs` | Sparse trajectory state, seeding, flow propagation, and an optional neural path. Its algorithm branding is not evidence of a qualified learned tracker. |
| `src/neural/router.rs`, `src/neural/hypothesis.rs`, `src/neural/pose.rs`, `src/neural/pipeline.rs` | Confidence routing, alternative motion interpretation, landmark-to-motion mapping, and another adaptive orchestration layer. Further ownership separation is required. |
| `src/neural/model_manager.rs`, `src/neural/sam.rs` | Model discovery/registry/artifact and segmentation-related surfaces; actual capability and model qualification remain separate. |
| `src/signal.rs` | Integration, detrending, normalization, and action extraction using numeric vectors and timestamp slices. |
| `src/funscript.rs` | `Action`, `Funscript`, `AxisChannel`, `MultiAxisScript`, interpolation, sanitization, diagnostics and export-related behavior. |
| `src/batch/queue.rs`, `src/batch/detector.rs` | Batch jobs/configuration/progress, a threaded worker with media/flow/signal dependencies, and library coverage/audit/enqueue operations. Full parity with GUI/CLI is not established. |
| `src/kinematics/device.rs`, `scurve.rs`, `rig.rs`, `thermal.rs` | Device profiles and limits, kinematic state, curve smoothing, rig solutions, and a thermal model. Calculated thermal load is not measured hardware temperature. |
| `src/sync/tcode.rs`, `serial.rs`, `buttplug.rs`, `handy.rs`, `vr.rs` | Concrete protocol/transport/session implementations, including UDP, serial, Buttplug, Handy HTTP, and incoming VR telemetry. Interfaces vary in command shape, errors, events, and lifecycle. |
| `src/plugin/engine.rs` | In-process `FunscriptPlugin: Send + Sync` registry with metadata/parameters/action transformation and three built-ins. This interface is not a security sandbox. |
| `src/stash/client.rs` | Synchronous HTTP/GraphQL integration with queries and remote mutations such as tagging/scanning. External path strings need separate local resolution/authorization policy. |

Current generation shape, simplified:

```text
CLI generation loop ----+
                       +--> media frames --> optical flow / neural detector + tracker
GUI generation loop ----+                                      |
                                                        samples / poses
                                                               |
                                                        signal processing
                                                               |
                                                        script channels
                                                               |
                                                        export / playback

Batch worker has its own media/flow/signal orchestration dependencies.
```

Observed architectural pressure includes duplicated orchestration, weakly distinguished numeric/coordinate/time values, overlapping temporal state, and presentation disconnected from measured observations. These motivate design; they do not imply every proposed replacement has already been approved.

### 5.3 The reported stationary GUI box

The user reports a box locked at one coordinate across all attempted videos, without a failing clip to supply.

At the inspected baseline, `render_tracking_overlay` in `src/gui/app.rs` starts around line 795. It fixes the reticle's horizontal coordinate at video center and derives vertical position from funscript stroke percentage. Around lines 835-838, it creates a rectangle centered on `video_rect.center()`, with width `min(video_rect.width() * 0.40, 320.0)` and height `video_rect.height() * 0.75`. The Cinema caller uses the script's interpolated playback position.

This is a viewport-centered ROI guide, not a detector-driven bounding box. The existing worker message interface does not deliver bounding-box telemetry. Fixing backend propagation alone cannot make this rectangle follow media content.

The source-level explanation is confirmed for that overlay. Reproduction against the user's running build and representative media remains separate. Required future design work includes source/time/coordinate-qualified observations reaching the viewer, visible handling of unavailable or stale tracking, and distinct labeling of simulation versus measured tracking. Exact preview/analysis behavior is still open.

### 5.4 Prior debugging evidence, not completed fixes

Before architecture planning paused implementation, the existing suite passed 130 tests. Twenty added regressions failed on an isolated copy of the pinned upstream, before production source changes:

| Area | Count | Reproduced problems |
| --- | --- | --- |
| Legacy decoder | 5 | Negative extents, non-finite inputs, clipped-center inconsistency, unknown-class panic, zero-class candidate. |
| Direct decoder | 3 | Reversed corners, guessed border/center format, non-finite score. |
| Temporal tracking | 4 | Center moves without box, stale state across scene cuts, confidence ordering replacing spatial continuation, missing detections not aging without flow. |
| Sparse points | 3 | Degenerate box seeding, zero-count mask division, forward flow reused as backward flow. |
| Signal extraction | 4 | Flat input inventing a stroke, reversed decreasing motion, trailing pause gaining a reversal, lost plateau boundary. |
| Flow scratch | 1 | Equal-area resolution change retaining old grid dimensions. |

Baseline commands were `cargo test --locked --bin pulsar -- --nocapture` and a targeted `cargo test --locked --manifest-path /tmp/pulsar-m1-baseline.hdP72q/Cargo.toml --bin pulsar m1_ -- --nocapture`. The latter returned exit 101 with 0 passed, 20 failed, and 130 filtered out. The temporary path is historical local evidence, not a guaranteed durable fixture location.

The local FunGen-12n-pov-1.1.0 model and hand-only animation fixture were present for the earlier smoke tests. Observed model shapes were `[1, 3, 640, 640]` input and `[1, 14, 8400]` output. These are smoke observations, not quality or overlay qualification.

Prepared backend edits were not applied. Their transient preparation is not a durable implementation artifact or a completed milestone. Reconcile any future implementation with the approved interfaces and current baseline rather than blindly applying an earlier patch. The full requested bug/correctness pass remains pending.

## 6. Proposed target modules and ownership

The selected process direction is:

```text
GUI / CLI / assistant controller / third-party clients
                         |
             versioned local engine protocol
                         |
                  headless engine
       projects / permissions / jobs / playback
                         |
           isolated workers and qualified adapters
        media / inference / executable plugins / devices
```

This is a logical ownership diagram, not a finalized process topology. It does not choose one engine per user versus per project, a worker pool size, or the physical location of every adapter. One portable distribution does not require one process, and a background terminal is not the engine protocol.

| Proposed module | Owns | Must not silently absorb |
| --- | --- | --- |
| Domain | Motion/project/source identities, revisions, units, coordinate/time distinctions, and invariants. | GUI types, filesystem operations, inference sessions. |
| Application | Typed commands/events, job coordination, transactions, cancellation, capability enforcement. | Rendering or detector-specific numerical implementation. |
| Media | Probe/parse/decode and source timestamp/projection metadata. | Editing authoritative project state or synthesizing motion. |
| Project editing | Undoable edits, protected regions, regeneration application, revision conflicts. | Direct device control or destructive source-media editing. |
| Playback | Clock authority, seek/pause/rate, synchronization and scheduling. | Independent generation algorithms. |
| Perception | Optical flow, detection, tracking and observation fusion. | Treating inferred device positions as measured physical truth. |
| Motion processing | General DSP, synthesis, neutral-axis motion and explicit creative transforms. | Model installation or unrestricted external effects. |
| Inference | Model execution, backend adapters, tensor contracts and resource accounting. | Project commands or permission decisions. |
| Model catalog | Manifests, artifact identities, capabilities, compatibility and artifact lifecycle. | Deciding whether model observations are correct. |
| Device execution | Profile adaptation, command admission, arming, deadlines, stop/reconnect and transport coordination. | Rewriting the neutral master or delegating all safety to diagnostics. |
| Diagnostics | Capability probes, validation reports and reproducible evidence. | Bypassing normal device authority outside explicit test mode. |
| Extensions | Role-specific plugin interfaces, grants, budgets, lifecycle, provenance and qualification integration. | One unrestricted host interface for all extension classes. |
| Clients and integrations | GUI, CLI, assistant controller and external integration adapters. | Separate generation pipelines or independent project truth. |
| Persistence and distribution adapters | Durable project/artifact storage and portable pack lifecycle behind explicit interfaces. | Hidden mutation or ambient authority in pure computation. |

Module names and crate placement are proposals. Use separate crates where compiler-enforced dependency direction is valuable, not automatically one crate per table row. Favor deep modules: substantial behavior behind a small interface, with implementation choices hidden from callers. Avoid pass-through abstraction layers that merely move complexity around.

Pure numerical functions and explicit state machines should be separated from effectful adapters. Stateful tracking, runtime sessions, and resource scheduling should not be mislabeled pure merely to satisfy an architectural slogan.

GUI presentation state, authoritative project state, and worker execution state require distinct ownership. The proposed split leaves viewport/layout rendering with the GUI while the engine owns semantic application state. Exact selection, navigation, preference, and assistant-to-GUI intent interfaces remain open.

## 7. Interface, type, and provenance design agenda

For every module, specify the complete interface: owned state, accepted values, output guarantees, units, identities, ordering, errors, cancellation/reset, concurrency, performance/resource behavior, and allowed side effects. A type signature alone is not a sufficient contract.

Candidate distinctions discussed, with exact Rust types and wire schemas still open:

- Source identity, project revision, job/attempt identity, and result lineage.
- Media timestamps versus monotonic scheduling instants, including seek/cut epochs and timestamp conversion.
- Pixel versus normalized coordinates and the identity of the image/projection space to which they belong.
- Observed, inferred, synthetic, unavailable, and unresolved information without fabricated certainty.
- Neutral motion versus device-specific, admitted commands.
- Immutable source references and reversible analysis transforms, including mapping observations back through crops/trims/time alignment.
- Generation/edit candidates versus committed project revisions.
- Authorized capabilities versus a model's requested operation or a plugin's declared desire for access.

Potential interface work packages are client commands/results, job events/checkpoints, media frames/buffer ownership, observations, motion candidates, edit transactions, admitted device commands, and plugin host operations. These are not finalized method names or schema definitions.

Provenance design recommendations:

- Carry compact references through hot paths instead of copying full lineage into each tensor or sample.
- Keep detailed lineage in project-associated records and retain enough linkage to explain motion and localized review flags.
- Treat metadata stripping as an explicit projection to a different artifact purpose, such as playback export, rather than deletion of native project history.
- Do not let stripped artifacts masquerade as fully traceable originals or discard identities required for permission, revision, freshness, or device checks.
- Distinguish reproducibility metadata from a promise that missing source/model artifacts or nondeterministic backends can be replayed exactly.

Which records are mandatory, how references are validated, retention/garbage collection, privacy redaction, and the lifecycle of optional heavy traces remain open.

## 8. Authority, isolation, and source-origin constraints

### 8.1 Assistant and client authority

The discussed security direction is an engine-enforced, capability-scoped command interface. The assistant must not receive unrestricted shell or filesystem authority. If it emits CLI syntax, parse that syntax into allowlisted application operations rather than executing a shell command string.

The policy-enforcing module validates requests independently of model output. Scoped automatic edits remain undoable and subject to project revision and protection rules. GUI navigation can be expressed through bounded application/view intents rather than arbitrary mutation of GUI internals. Exact grants, authenticated client sessions, approval persistence and revocation are unresolved.

### 8.2 Workers and extensions

Process isolation is crash containment, not a complete security sandbox. Native decoders, inference backends, and plugins need explicit authority/resource contracts; the specific OS mechanisms and their supported-platform limits are not yet selected.

An in-process Rust trait bound such as `Send + Sync` does not restrict filesystem, network, or device access. The present plugin host is not evidence that the selected sandbox architecture exists.

Sandboxing also does not prove semantic correctness. A driver can misencode an otherwise approved motion command. Driver qualification and device-I/O admission are distinct obligations. Qualification must bind to the accepted driver/version; an update must not silently inherit old evidence. Who accepts qualification, what evidence is required, and how qualification is invalidated remain open.

The engine's verified guarantees must state assumptions about native runtimes, extension behavior, protocol conformance, and hardware. Untrusted input/output validation and a qualification gate do not automatically prove arbitrary third-party implementations correct.

### 8.3 Naming and source provenance

Audit source/dependency origin and attribution before deciding what to retain or replace. A legacy symbol is not by itself evidence of copying, and a rename does not establish independent origin. Preserve licensing/attribution obligations; no license change is approved here.

Use responsibility-based names for generic interfaces and accurate implementation names for concrete adapters. `PulsarApp` was discussed as an example replacement for `FunGenApp`, not as a final naming scheme. Algorithm/model-family branding should correspond to actual implementation and qualified artifacts, not aspirational comments.

The scope, method, evidence standard, and compatibility implications of the naming/source-lineage pass remain to be designed.

## 9. Verification and correctness strategy

The confirmed assurance target is critical-path formal work plus layered testing, not a claim that tests formally verify an entire application.

| Technique | Intended obligation | Limit to record |
| --- | --- | --- |
| Type/architecture checks | Prevent invalid value mixing, invalid construction, and forbidden dependency directions. | Types do not establish model accuracy or validate arbitrary external bytes without parsing/checks. |
| Known-answer tests | Check fixed numerical/protocol/transform cases against independent expected results. | Specify precision, tolerances and backend applicability; universal bitwise GPU equality is not assumed. |
| Property and metamorphic tests | Exercise invariants and controlled transformations for geometry, timelines, motion, and composition. | Sampling is not exhaustive proof. |
| Fuzzing | Search parsers, protocols, command interfaces and state transitions for counterexamples and resource failures. | Declare corpus, bounds, budgets, isolation and reproduction rules. |
| Mutation testing | Challenge whether tests detect meaningful behavioral changes. | Review surviving/equivalent mutations; a score alone is not correctness evidence. |
| Formal models/proofs | Authorization, revision transactions, job/device lifecycles and selected key invariants. | Declare assumptions/bounds and check correspondence to implementation; a disconnected model is not proof of running code. |
| Differential and integration tests | Compare independent implementations and exercise real caller/adapter compositions. | Shared bugs or invalid references can invalidate the comparison. |
| Real-media and hardware qualification | Evaluate actual perceptual quality, temporal fidelity, resource/performance behavior and device response. | Do not substitute synthetic tests or a build success for qualified results. |

Tests should exercise the same meaningful interfaces used by callers, with internal seams where needed for numerical kernels and fault injection. Provide controlled substitutes for clocks, I/O, runtimes, and devices rather than accidentally actuating hardware during routine tests.

Concrete tools, formal proof obligations, arithmetic/float treatment, trusted implementation surface, accepted bounds, CI budgets, mutation thresholds, fuzz duration, and release gate policy remain open. No such tests or proofs were run as part of writing this document.

## Resolved architecture agenda

The former ARCH-O01 through ARCH-O14 questions are resolved by the submitted decisions and selected contracts below. This replaces the open-question list; it does not claim implementation or qualification.

| Former item | Resolution and implementation home |
| --- | --- |
| ARCH-O01 | Four library boundaries, dependency rules and modern names: CONTRACT-01; incremental M1/M2 migration. |
| ARCH-O02 | Per-user engine lifecycle, scheduling and aggregate resource admission: CONTRACT-05; M2/M3/M5. |
| ARCH-O03 | Versioned local command/query/event API, pairing, errors and resynchronization: CONTRACT-06; M2. |
| ARCH-O04 | Distinct identities, rational source time, integer project time and labelled coordinate spaces: CONTRACT-02; M1/M2/M4. |
| ARCH-O05 | Durable engine-owned SQLite transactions, shared undo and explicit rebase: CONTRACT-04; M2. |
| ARCH-O06 | Immutable job inputs, distinct attempts, validated checkpoints and bounded retry: CONTRACT-05; M2/M3. |
| ARCH-O07 | Frame/transform/seek-matched observations and independent previews: CONTRACT-03; M1/M2. |
| ARCH-O08 | Checked neutral motion and explicit observed/predicted/inferred/synthesized/unavailable semantics: CONTRACT-02 and boundary catalogue; M1/M3/M4. |
| ARCH-O09 | Compact lineage, scoped references, pinning and deliberate export stripping: CONTRACT-07; M2-M6. |
| ARCH-O10 | Wasmtime plugins, native worker isolation, exact-build exceptions and assistant approval policy: CONTRACT-08; M2/M3/M5/M6. |
| ARCH-O11 | Profile adaptation, admission, human-accepted qualification and separately approved headless stopping bounds: CONTRACT-09; M5. |
| ARCH-O12 | Evidence-led lineage audit and public compatibility window: CONTRACT-11; M1/M2/M6. |
| ARCH-O13 | Kani/TLC obligations, correspondence tests and layered campaigns before Preview: CONTRACT-10; M1-M8. |
| ARCH-O14 | Architecture-first execution under ARCH-041, preserving M1-M8 acceptance scope: CONTRACT-11, the major roadmap and the architecture-first execution plan. |

## 11. When architecture and planning are done

Architecture discussion is complete only when module ownership, dependency direction, public interfaces, state/effect ownership, types/units/time/provenance, failure/recovery, permissions, plugin trust, and verification obligations are settled sufficiently to implement without inventing material decisions.

Implementation planning is complete only when that architecture has executable, ordered work packages with concrete acceptance scenarios, migration/compatibility handling, required evidence and explicit prerequisites. Known product decisions must remain traceable. Open items may be deferred only through an explicit scoped decision with an owner/resolution point, not by silently assuming a default.

At that point, explicitly tell the user: **Architecture and implementation planning are complete for the agreed scope.** State any intentionally deferred scope alongside that declaration. Until then, say what remains and continue design.

That declaration does not mean implementation, the bug pass, M1, or release qualification is complete. Those require their own evidence and authorization. Do not claim completion because documents exist, because a long interview has occurred, or because tests pass on only a subset of behavior.

## Architecture closure: 2026-09-11

**Architecture discussion is complete across all 11 areas. Implementation and qualification remain unfinished.** ARCH-001 through ARCH-041 record submitted user decisions. The technical choices below are selected engineering contracts approved through the architecture-closure plan, not quotations attributed to earlier user answers and not descriptions of already implemented APIs. ARCH-041 subsequently changes delivery order to architecture first, then functional quality/research, then hardening/qualification; the [architecture-first execution plan](PULSAR_ARCHITECTURE_FIRST_EXECUTION_PLAN.md) defines its milestones.

The [Pulsar Software Specification](PULSAR_SOFTWARE_SPECIFICATION.md), including its decision sources, remains the product authority. This record refines architecture without reducing required modalities, quality, performance, offline availability, compatibility or platform/vendor scope. The [Pulsar Major Roadmap](PULSAR_MAJOR_ROADMAP.md) owns staged delivery and evidence gates. Earlier source maps describe their inspected baseline only; this consolidation does not refresh upstream or establish that defects still reproduce unchanged.

### Precedence and settled qualifications

- The final contracts below supersede earlier proposals and statements that these interfaces remain open. Earlier submitted answers and historical source findings remain preserved.
- ARCH-041 supersedes earlier M1-before-architecture sequencing, not M1 correctness obligations or M2-M8 scope. Phase A must establish real composed paths and enforced ownership, not empty scaffolding. Structural integration, functional demonstration and qualification remain separate states.
- ARCH-024 and ARCH-025 qualify the sandbox default: an explicit human-approved, exact-build native exception enters the trusted computing base. It is not made safe by a scoped API grant. ARCH-040 separately requires per-invocation approval for assistant calls by default, with an explicitly risky, human-controlled standing-grant alternative.
- ARCH-026 and ARCH-027 require qualified device-side failure-stop behavior and explicit approval of its per-configuration bound before headless playback. No universal numeric stopping deadline was chosen. Attended-only status does not certify an unsupported mode safe.
- ARCH-018 selects durable commit acknowledgement; bundled SQLite is the selected implementation technology. ARCH-019 selects shared actor-labelled undo; undo and redo are new authorized transactions, not restoration of an old revision identifier.
- ARCH-021 permits snapshot jobs to finish after edits, not stale commits. ARCH-022 permits eligible resume when the engine next starts, not OS-login autostart or device rearming.
- ARCH-028 permits one neutral program with up to six axes and multiple contributing sources/entities. It does not authorize simultaneous multi-device playback.
- ARCH-034 permits prominently labelled unqualified custom generation in normal projects, not silent degradation of qualified tiers or bypass of plugin, driver or device authority.
- ARCH-035 permits approved official qualification records or explicit human acceptance after mandatory local qualification. Running checks and accepting their result are separate authorities.

### CONTRACT-01: Modules, ownership and modern names

Use four Rust libraries and one packaged executable. Library boundaries express dependency and authority rules; they do not require a separate process for every logical module.

| Package | Owns | Forbidden responsibilities |
| --- | --- | --- |
| `pulsar-core` | Checked domain types, pure media/geometry/motion kernels, explicit state transitions and invariant predicates. | Unsafe code, I/O, GUI/runtime dependencies, database access, grants or device effects. |
| `pulsar-protocol` | Versioned wire DTOs, framing/schema validation, typed error/event shapes and checked conversion to domain values. | Project mutation, persistence, scheduling or effect authority. |
| `pulsar-engine` | Authorization, transactional projects, scheduler, job/attempt state, artifact/provenance stores, effect brokers and device sessions. | Presentation state or unmediated extension execution in its address space. |
| `pulsar-clients` | CLI and GUI adapters, presentation, transient editing gestures and independent previews. | Authoritative project writes, direct device access or a second generation implementation. |
| `pulsar` executable | Composition root and dispatch to client, per-user engine or isolated worker roles. | Business rules duplicated between process entry points. |

Dependency arrows mean "may depend on":

```text
pulsar executable
  -> pulsar-clients -> pulsar-protocol -> pulsar-core
  -> pulsar-engine -> pulsar-protocol -> pulsar-core
  -> worker-role composition -> protocol/core + isolated effect adapters

GUI / CLI / third-party client -- secured local API --> per-user engine
engine -- bounded worker protocol --> isolated decoder/inference/plugin/driver worker
worker -- immutable observations or candidate artifacts --> engine
engine -- checked durable transaction --> project store
```

There is no client-to-engine implementation dependency and no core-to-protocol, client or effect dependency. Pure kernels receive validated values and return explicit results. Unsafe/FFI code is confined to reviewed adapters outside core. Native runtime dependencies are role/feature-scoped so launching a basic client or engine does not initialize every backend.

Logical modules are concrete responsibilities inside these packages, not additional mandatory libraries:

| Logical module | Core responsibility | Engine/effect responsibility | Client responsibility |
| --- | --- | --- | --- |
| Media input/parser | Identity, metadata contracts, time/space mappings and buffer validation. | Import, snapshot, decode/probe workers and artifact access. | Select/relink sources and show explicit availability. |
| Media viewer | Frame/evidence association and transform composition. | Cache lookup and lightweight preview-analysis jobs. | Decode/display presentation, viewport and independent playhead. |
| Media editor | Edit algebra, protected regions and motion invariants. | Authorized revision transactions, candidate rebase, undo/redo. | Gesture previews and commit requests; not a full video NLE. |
| General DSP and optical flow | Pure filters, resampling, geometry, Lucas-Kanade kernels and track state transitions where practical. | Budgeted execution, artifact handling and worker lifecycle. | Parameter controls and truthful diagnostics. |
| Neural pipeline | Tensor/pre/postprocessing contracts, fusion semantics and checked outputs. | Model catalogue, local runtime workers, scheduling and qualified execution profiles. | Tier selection and qualified/unqualified status. |
| Local assistant | Typed action proposals and domain validation shared with other clients. | Capability broker, approvals, resource accounting and audited command execution. | Conversation and explicit approval surfaces. |
| Hardware doctor | Evidence/configuration types and eligibility predicates. | Read-only probes by default; separately authorized physical qualification and admission. | Explain unsupported configurations, required evidence and human acceptance. |
| Assurance suite | Harnesses for kernels and invariants. | Fault, protocol, lifecycle, backend and hardware harnesses. | GUI/CLI parity and real observation-rendering scenarios. |

Rename `FunGenApp` to `PulsarDesktop`. Preserve attribution and perform the agreed source/model lineage audit before redistribution claims. A rename, rewrite or new crate layout does not establish clean-room provenance.

### CONTRACT-02: Domain identities, units and evidence

| Term | Meaning and invariant |
| --- | --- |
| Project / `ProjectId` | One independent project time domain, one master motion timeline and one revision history. |
| Source version / `SourceVersionId` | Content-bound identity of immutable source bytes, distinct from a mutable filesystem path. |
| Source placement / `SourcePlacementId` | A version's explicit mapping into project time; the same source may have different placements. |
| Frame / `FrameId` | A decoded sample identified within a source version with its source timestamp/sample identity; not a GUI frame counter. |
| Entity track / `EntityTrackId` | Perception identity with explicit lifecycle, continuity and cut/reset scope. |
| Motion target / `MotionTargetId` | Selected entity, relationship, region or synthetic intent from which motion is derived; not a hardware identity. |
| Motion output / `MotionOutputId` | The project's neutral program, with up to six named axes in the first release. |
| Revision / `RevisionId` | A committed project state. Undo/redo creates a new revision rather than reusing one. |
| Candidate / `CandidateId` | Proposed motion/edits bound to an input revision and lineage; not committed state. |
| Job / `JobId` | Stable requested work against an immutable input manifest. |
| Attempt / `AttemptId` | One execution with exact model/runtime/backend/configuration dependencies. |
| Artifact / `ArtifactId` | Immutable, validated content with an explicit owner, format, size and retention policy. |
| Physical device / `PhysicalDeviceId` | A hardware endpoint identity, distinct from an output, profile, driver or transport connection. |
| Device session / `DeviceSessionId` | Engine-owned controller authority, admission and playback epoch for a device. |
| Grant / client session | Explicit operation/project authority and authenticated connection context; neither is an arbitrary process credential. |
| Provenance reference / `ProvenanceRef` | Opaque reference to compact lineage, not a copied graph or implicit read capability. |
| Qualification / admission | Qualification binds evidence to an exact configuration; admission checks a particular operation/program against it and current authority. |

Preserve rational source PTS/timebase, checked signed integer-nanosecond project time, and a separate monotonic playback deadline type. Source placement mappings are explicit rational transforms, including offsets and rate changes. Conversions check denominators, range and overflow. Floating-point frame indices must not replace source timestamps. Quantization occurs only at declared export/transport boundaries with a recorded rounding/error report.

Changing source alignment leaves existing motion unchanged and marks affected derived results stale. Retiming/regeneration is an explicit edit. Seeks, cuts, discontinuities and source replacements reset the relevant association/tracker epochs rather than pretending continuity.

Neutral positions are finite checked values in `[0,1]`, with explicit axis identity, direction conventions and availability. Missing axes are not silently zero-filled. Relative motion is not automatically stretched to full range; physical distance/angle conversion belongs to a device profile. Detailed six-axis encodings and profile tolerances must be published and qualified in M4/M5 without changing these invariants.

Coordinates always identify their space: decoded source, model tensor, crop, rotated image, projection or viewport. Transform composition carries version/identity and dimensions. Invalid, non-finite, reversed or degenerate geometry produces typed invalid/unavailable results; clipping and normalization are explicit operations, not a reason to fabricate a valid detection.

Every span or observation distinguishes observed, tracked prediction, inferred, synthesized and unavailable evidence. Prediction carries age and its last observation association. Confidence includes whether it is calibrated; an uncalibrated score is not a promised probability. Text/still-image synthesis and preset motion are labelled synthesis, never detector observations.

### CONTRACT-03: Viewer and tracking

The viewer reuses matching observations or requests lightweight on-demand analysis when none exist. Rendering does not depend on completing a generation job.

An observation may render only when source version, displayed frame, source placement/mapping, transform context and seek/request generation match. A late result can enter a correctly keyed cache but cannot overwrite current display state. On a seek, source change or incompatible transform, hide stale boxes while awaiting matching evidence.

Actual detections, bounded-age track predictions and user-defined analysis regions have distinct labels/styles. A fixed user region is legitimate only as a labelled region; it cannot stand in for tracking. Missing detections show unavailable/no-observation state rather than a decorative box. Resizing/projection changes must recompute or invalidate the applicable display transform.

Client previews have independent playheads. Attaching to physical playback displays the engine session's admitted revision and clock explicitly; preview seeks or edits do not move the device.

### CONTRACT-04: Projects and persistence

Use bundled SQLite: no user-installed database service or library. Bundled compilation is a build-time concern, not an end-user installation step. [Bundled SQLite build documentation](https://github.com/rusqlite/rusqlite#notes-on-building-rusqlite-and-libsqlite3-sys).

Only the engine commits. A commit validates the authenticated actor/grant, project scope, protected-region permissions, expected revision, request identity and domain invariants. Revision state, compact lineage, request outcome and the corresponding durable event are recorded atomically. Acknowledge success only after the transaction's configured durable synchronization completes. Hardware/filesystem durability assumptions remain explicit; transient gesture previews are not acknowledged commits.

Interactive gestures form atomic edits. Shared actor-labelled undo targets the latest committed project edit regardless of originating client. Undo/redo creates a fresh revision and rechecks current permissions/protection. Conflicting or protected changes are not bypassed by history operations.

Candidates retain their input revision. Stale candidates require explicit rebase and then a fresh expected-revision commit; rebase does not grant commit authority or silently replace user edits. Rebase failures remain inspectable candidates.

Generation media is an immutable engine-managed snapshot, using copy-on-write where supported and otherwise copying. Content identity, preparation time and required disk space are explicit. Linked mutable originals are not a silent substitute when snapshot admission fails.

Recovery restores committed state and eligible job records. Migrations preserve a recoverable original and do not overwrite the sole project copy before successful migration. Portable exports capture a consistent database/artifact snapshot, not a raw copy of live database files. Missing media can be relinked only to matching content for exact replay; changed content is a new source version.

### CONTRACT-05: Engine, jobs and resources

One engine per OS user owns project writes, resource scheduling and device sessions. Secured discovery and instance exclusion prevent accidental duplicate owners. Incompatible clients must report protocol incompatibility, not kill or replace a running engine. Closing a GUI does not stop authorized offline jobs; this does not authorize OS-login autostart.

A job has immutable input/configuration identities and separate attempts. Every attempt records exact runtime/backend/model/transform dependencies and produces immutable artifacts or a candidate, never a direct project mutation. Edits during generation do not cancel it automatically; completed old-revision work remains stale until explicit review/rebase.

Eligible interrupted jobs resume when the engine next starts only from validated checkpoints and matching dependencies. Checkpoint publication is atomic and includes progress, content identities and compatibility metadata. Missing/evicted prerequisites block recovery unless restored from matching content.

Automatic backend failure may start a new attempt on a qualified equivalent preserving the requested quality tier. Disclose backend and performance changes. Explicit backend pins are strict. Never reuse incompatible checkpoints or silently downgrade into a lower tier or unqualified custom run. Retries and crash recovery loops are bounded.

Protect control/device/live deadlines first, then interactive preview/assistant work, then offline generation with project fairness. Assistant requests may suspend offline work for resources, not disrupt live control. Account for engine, workers, model residency, plugin memory, buffers, checkpoints and snapshots in aggregate CPU/RAM/VRAM/storage budgets.

Insufficient resources block admission or expose a recoverable blocked state; do not silently weaken durability, isolation, timing or output guarantees. Pinned or authoritative data cannot be reclaimed to make admission succeed. Performance gates remain end-to-end and include required preparation, not inference-only timing.

### CONTRACT-06: Local API and authority

Use versioned, length-framed UTF-8 JSON over secured per-user Unix sockets or Windows named pipes. Protocol v1 bounds control messages at 1 MiB. Bulk frames/artifacts use separately bounded authorized streams or immutable read-only buffer handles. The transport library supplies local sockets, not the application framing or authority policy. [Local IPC documentation](https://docs.rs/interprocess/latest/interprocess/local_socket/index.html).

Endpoint naming alone is not authentication. Use platform-appropriate per-user endpoint ownership/access controls, peer/session validation and pairing. Do not treat a generic namespace or shared temporary directory as equivalent security across platforms. [Endpoint naming documentation](https://docs.rs/interprocess/latest/interprocess/local_socket/enum.GenericNamespaced.html).

The published request envelope identifies protocol version, request ID, authenticated client session, operation, validated arguments, project scope when applicable and expected revision for project mutation. Request identifiers are scoped to authenticated identity; reusing an identifier with different arguments is an error.

| Surface | Contract families | Required semantics |
| --- | --- | --- |
| Project commands | Create/open project, import/relink source, set alignment, apply edit, undo/redo. | Engine authorization, checked domain conversion and atomic expected-revision mutation. |
| Job commands | Start/cancel/resume job, rebase/commit candidate. | Immutable manifests, distinct attempts, cancellation fencing and explicit stale handling. |
| Artifact commands | Export/package, pin/unpin, authorized artifact transfer. | Consistent snapshots, bounded transfer, deliberate metadata stripping and local receipts. |
| Authority commands | Pair, grant/revoke, controller acquire/release. | Explicit human-owned elevation, scoped grants and revocation effects. |
| Device commands | Arm, start/stop and switch playback revision. | Qualified admission, controller ownership, revision pinning and explicit session epochs. |
| Queries | Project/job/candidate snapshots, capabilities, qualification and session state. | Coherent revision/identity tags; no mutation hidden inside reads. |
| Durable events | Revision committed, grant revoked and authoritative job/session transitions. | Ordered cursor within its documented scope; reconnect replay or snapshot resynchronization. |
| Ephemeral events | Progress, preview frames and transient diagnostics. | Bounded queues; coalesce/drop under backpressure, never counterfeit durable outcomes. |

Publish command/query/event schemas, field constraints, compatibility negotiation and typed errors. Error families include invalid input, unauthorized/revoked grant, revision conflict, protected edit, missing dependency, resource blocked, incompatible checkpoint/protocol, unqualified configuration, cancelled work and unknown external-effect outcome. Unsupported operations and malformed data fail explicitly, not by panic or implicit fallback.

For committed project mutations, persist request identity and outcome in the same transaction as the edit. Retrying the same request returns its recorded outcome rather than creating a second edit. For external/native/device effects, distinguish accepted, dispatched, acknowledged and unknown outcomes; do not claim exactly-once physical effects or blindly replay an ambiguous command.

Reconnect provides a snapshot plus a valid event cursor; expired/gapped cursors require resynchronization. Authority and current revision are rechecked after reconnect. Client disconnection is not grant revocation. Revocation blocks new work, cancels work authorized solely by the revoked grant and withdraws associated device/controller authority; late results cannot restore it.

No arbitrary shell, SQL, raw project-memory mutation or ambient device access is exposed. Third-party clients pair explicitly and receive project/operation grants; device and external effects remain separately scoped. This is not protection against a fully compromised same-user OS account.

Buffers validate identity, size, dimensions, strides, element type and ownership before allocation/use. Shared handles are immutable, authorization-scoped and lifetime-managed. Never reuse a client-visible shared object for future unauthorized data. Revocation can deny new transfers but cannot make already disclosed bytes unread.

Public wire compatibility does not promise a stable internal Rust ABI. GUI and CLI use the same engine contracts; the GUI is not a background terminal wrapper.

### CONTRACT-07: Provenance, privacy and disposal

Compact lineage links source versions and observations to model/runtime/algorithm/configuration identities, transforms, candidates and committed output spans. Hot paths carry opaque references; consumers resolve only the lineage they are authorized to access. Provenance is not an authority token.

Keep active job/checkpoint/candidate dependencies pinned until their documented consumers release them. Explicit user pins and packaged projects retain required media for self-contained replay. Once unused and unpinned, source snapshots and other recomputable cache artifacts are evictable. Retain committed motion, compact lineage and source content identities; original media and authoritative project state are not cache-eviction targets. Eviction may make an exact rerun unavailable until matching content is restored, which must be shown honestly.

Heavy traces are opt-in, bounded and separately disposable. Avoid credentials, prompt/media contents or source-identifying metadata in ordinary logs. Retention and diagnostic export permissions are explicit.

Standard funscript exports and device payloads strip internal/source-identifying provenance at that boundary while preserving local adaptation/export receipts. Explicit portable project packaging is a different operation with a declared manifest, not accidental inclusion of private lineage in standard motion files.

Official engine-mediated processing remains local/offline as specified. Authorized online device/service operations remain explicit effects. User-approved unsandboxed native code is outside confinement/privacy enforcement guarantees and must be labelled accordingly.

### CONTRACT-08: Plugins, models and runtimes

Use Wasmtime/WASM for bounded processing/editing extensions. Native runtimes, codecs and drivers use isolated worker interfaces. Do not load third-party extension code into engine or GUI address space.

Role-specific manifests specify artifact identity, interface version, required capabilities, buffer/tensor formats, preprocessing/postprocessing, budgets and effect requests. Validate tensor rank/shape/type/size, image transforms and output values before use. Model import does not implicitly execute arbitrary custom code or authorize downloads.

Host effects are brokered. Guests and workers cannot grant permissions, mutate project memory, self-qualify drivers or bypass device admission. Guest memory/compute limits do not bound blocking host calls: each broker call needs separate deadlines, cancellation and resource accounting. [Wasmtime resource-control limitations](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html#method.epoch_interruption).

Native confinement must be qualified on each supported platform. If unavailable, refuse execution unless the human approves the exact-build trusted exception. Changed bytes invalidate approval. The exception belongs to the trusted computing base and cannot be presented as sandboxed. Wasmtime platform support is a feasibility reference, not qualification of Pulsar's packaged configurations. [Wasmtime platform support](https://docs.wasmtime.dev/stability-platform-support.html).

For assistant invocation of unsandboxed extensions, exact-build load approval and invocation approval are distinct. Default: approve each proposed call. The human may separately enable an explicitly risky standing grant bound to exact build and declared call scope. The assistant cannot enable either the native exception or standing grant. Revocation or changed bytes invalidates authority; native effects remain outside sandbox guarantees.

The model zoo distinguishes qualified built-in execution profiles from structurally validated, prominently unqualified custom runs permitted in ordinary projects. Custom runs do not inherit official quality/speed claims or relax driver/confinement requirements. Qualified automatic selection cannot silently become a custom run.

Official core/complete packages bundle their required offline models/runtimes as verified, versioned resources with atomic first-use extraction and recoverable update/rollback. Large model archives are packaged resources, not enormous linked program-data sections. No silent system-package installation or network bootstrap is required for the promised offline core. Preserve the specification's platform/architecture/vendor scope and release staging.

### CONTRACT-09: Device admission and qualification

Keep neutral motion, profile adaptation, admission, driver encoding and transport separate:

```text
neutral program + revision
  -> profile adaptation + local receipt
  -> engine admission against grants, limits and qualification
  -> qualified driver encoding
  -> transport / physical device
```

Adaptation preserves event timing and constrains amplitude to the selected device profile, with explicit physical-unit conversion. It cannot invent full-range motion or silently retime to pass limits. If constraints cannot be met under this policy, block admission or require an explicitly reviewed alternative rather than weakening limits.

Each physical device has exclusive engine ownership and one authorized session controller. Playback is pinned to its admitted motion revision. Ordinary edits and independent preview seeks do not change physical commands. A controller-authorized revision switch requires re-admission and stop/rearm where the transport cannot switch safely. Live-input generation is a distinct authorized session mode.

Normal playback requires an approved official qualification record or explicit human acceptance after required local qualification. Evidence/acceptance binds exact driver artifact, physical device, firmware and transport configuration; changed configuration invalidates applicability. The assistant may help execute authorized checks but cannot supply human acceptance. Generation/model qualification does not substitute for driver qualification.

Controller loss stops by default unless explicit headless mode is granted. Headless additionally requires a qualified device-side stopping bound after engine/connection failure and explicit approval of that bound for the actual configuration. A sent or acknowledged stop command does not prove physical stopping. Unknown physical state remains unknown.

Restart/reconnect remains disarmed; old commands/session epochs cannot rearm implicitly. Separate physical test mode requires explicit authorization and cannot promote itself to normal playback or qualify its own acceptance.

### CONTRACT-10: Assurance and evidence

Use Kani for bounded critical Rust kernels and TLA+/TLC for lifecycle models, with separate implementation-correspondence tests. Record assumptions, bounded domains, unwinding limits, numerical tolerances and trusted components. Kani does not prove the whole concurrent application; TLC checks a specified model, not arbitrary Rust implementation. [Kani documentation](https://model-checking.github.io/kani/print.html), [TLC documentation](https://docs.tlapl.us/using:tlc:start).

| Obligation | Owner / boundary | Planned assurance | Required delivery gate |
| --- | --- | --- | --- |
| No cross-project or operation authority escalation | Engine broker and protocol conversion. | Bounded grant predicates; lifecycle model; negative integration tests and protocol fuzzing. | M2 implementation evidence; critical formal obligations before M7 Preview. |
| No partial or stale durable commit | Core revision transition and engine transaction. | Kani checked predicates; TLC commit/conflict model; crash injection and durable-outcome retry tests. | M2; M7 release-bound proof/correspondence evidence. |
| No late result accepted under cancelled/revoked authority | Job/attempt and grant lifecycle. | TLC race/interleaving model; worker fault injection; correspondence tests. | M2/M3; M7. |
| Exclusive device ownership and no stale session replay | Device controller, admission and session epoch. | Lifecycle checking, protocol tests, reconnect fault injection and real transport qualification. | M5; M7, with physical evidence separately required. |
| Checked time/space/tensor/buffer arithmetic | Core types and trust-boundary adapters. | Kani bounded arithmetic; KAT; property/metamorphic/differential tests; malformed-input fuzzing. | M1-M4 affected surfaces; M7 critical formal gates. |
| Truthful frame/evidence association | Perception-to-viewer boundary. | KAT/property tests, seek/cut/resize/late-result scenarios and real detector media. | M1/M2; no synthetic-only detector qualification. |
| No invented stroke or timing distortion | Motion/DSP/adaptation boundaries. | Independent KATs, metamorphic properties, differential checks and annotated media/device comparisons. | M1/M3/M4/M5 and existing quality gates. |
| Enforced confinement and bounded resources | Runtime/plugin/buffer brokers. | Malicious guest/host-call fixtures, deadline/OOM/storage/crash injection and platform qualification. | M3/M5/M6; M7. |
| Correct public behavior across clients and migrations | CLI/GUI/protocol/store adapters. | Contract parity, non-destructive migration, reconnect and portable-export scenarios. | M2/M6; M7/M8. |

Use independent known answers, property/metamorphic/differential tests, fuzzing, mutation testing, fault injection, real-media benchmarks and hardware qualification. Mutation campaigns must kill critical invariant mutants or record reviewed equivalence evidence; tests derived solely from the implementation are not an independent oracle.

Fast PR checks cover contracts, KATs, properties and targeted critical proofs. Deeper affected-surface and release campaigns cover fuzzing, mutation, native backends, performance and physical hardware. Every receipt binds its actual source/build/artifact/configuration, tool version, assumptions and result. Missing, unsupported, stale or failed evidence is not PASS.

Defined critical formal obligations gate the first public Preview. Neural accuracy, foreign runtime correctness, operating-system guarantees and physical safety are not established by these proofs. Development/CI proof tools are not end-user runtime dependencies. Existing product quality, performance and physical qualification gates remain mandatory.

### CONTRACT-11: Migration, compatibility and milestones

Preserve documented CLI syntax/aliases and non-destructive project imports through the first Stable major series. Remove deprecated public interfaces only in a later major with a migration path. Preserve standard funscript interoperability permanently, not historical defects.

Replace duplicated GUI/CLI/batch generation incrementally through shared contracts. Every transitional adapter must record its owner, supported compatibility surface, replacement gate and removal milestone. Modernize internal names without rewriting provenance or licensing history.

ARCH-041 selects architecture-first delivery: Phase A establishes the final four-library ownership/dependency shape and real integrated interface paths; Phase B performs functional implementation, deep technical research and correctness closure; Phase C completes hardening and qualification. The [architecture-first execution plan](PULSAR_ARCHITECTURE_FIRST_EXECUTION_PLAN.md) owns the A1-A6, B1-B6 and C1-C3 execution milestones.

M1 retains its baseline, bounding-box, tracking and stroke-correctness acceptance scope, but no longer precedes architecture as a mandatory execution stage. Re-establish the selected current baseline without overwriting local documentation or unrelated work. Reproduce the historical 20 regression cases against the migrated/current implementation; do not assume old failures or apply cached patches blindly.

Phase A must retain known defects explicitly, correct migration-created regressions and minimum data-loss/authority/unsafe-effect blockers, or disable affected paths. Its real vertical slice must use GUI/CLI engine interfaces, immutable worker input, a revision-bound candidate, authorized durable commit and neutral export. Empty modules and success-returning stubs do not satisfy architectural completion.

Phase B must fix demonstrated detector shape/coordinate validation and clipping, tracker identity/aging/cut behavior, box propagation, forward/backward-flow defects and stroke artifacts. Preserve pauses, shallow motion, reversals and plateaus; eliminate invented full-range movement. Connect actual observations to GUI rendering and exercise moving targets, absent detections, seeks, cuts, resizing, projection changes and late results. Real-media behavior is separate from synthetic/unit evidence; a moving synthetic box does not qualify a real detector.

Deep research follows stable architecture and measurement interfaces. Record falsifiable hypotheses, independent oracles, exact artifacts, budgets, results and negative findings. ADRs record decisions and link reproducible experiment evidence; an accepted ADR is not qualification. Fix deterministic bugs that invalidate an experiment before trusting its measurements.

The M1-M8 table below preserves delivery scope and acceptance obligations; it is not the chronological execution order. No product requirement or critical formal/device gate is weakened by the new sequence.

| Milestone | Contract delivery without scope reduction |
| --- | --- |
| M1 | Reproduced baseline defects, truthful box/flow/frame boundaries and stroke correctness, with separate real-media evidence. |
| M2 | Shared engine/API, checked domain contracts, durable projects, revocation/revision lifecycle and GUI/CLI/batch parity. |
| M3 | Qualified local inference, explicit attempts/fallback, bounded resources, assistant authority and reference performance. |
| M4 | All required modalities, projections, labelled synthesis and six-axis neutral output. |
| M5 | Live deadlines, profile admission, Handy/Handy 2 modes, controller/session lifecycle and hardware qualification. |
| M6 | Portable offline packages, extraction/migrations, compatibility and required OS/architecture/GPU-vendor qualification. |
| M7 | Preview product gates plus required critical proofs, models, correspondence and candidate-bound evidence. |
| M8 | Stable and comparative qualification, preserving all specification quality/performance/device obligations. |

### Boundary contract catalogue

These are target interface families, not claims that callable interfaces or wire schemas already exist. Each implementation must publish its checked input/output types, error cases, ownership, resource lifetime and evidence obligations before replacing a legacy seam.

| Boundary | Input / output | Authority and invariant |
| --- | --- | --- |
| Import / decode | Source request -> immutable source version, metadata and source-timed frames. | Engine owns access and snapshots; malformed media is untrusted; buffer allocation is bounded. |
| Inference | Frame/tensor plus artifact/configuration identities -> validated observations. | Isolated runtime, explicit model/source transforms and availability; no project/device mutation. |
| Tracking / flow | Ordered frame/evidence state plus cut/reset context -> tracks/predictions and next state. | Explicit identity/age, dimension compatibility and backward-check semantics; no invented observation. |
| Motion derivation / DSP | Target-bound evidence or labelled synthesis -> neutral motion candidate. | Checked units/timing, uncertainty/provenance and amplitude preservation. |
| Project edit | Actor/grant, request identity, expected revision and edit/candidate -> new revision or typed failure. | Engine-only atomic durable transaction; stale/protected changes cannot bypass checks. |
| Worker execution | Immutable job/attempt manifest -> checkpoints, progress and immutable artifacts. | Worker cannot commit or regain revoked authority; engine owns accepted transitions. |
| Preview | Displayed-frame/transform/request identity -> matching observations or explicit unavailable state. | Independent client playhead; stale evidence cannot render as current tracking. |
| Device adaptation / admission | Neutral revision plus profile/configuration/authority -> admitted session or rejection. | Preserve timing, constrain amplitude, bind exact qualification and controller. |
| Driver / transport | Admitted commands plus session epoch -> encoded payload and explicit outcome. | No ambient access, no implicit rearm, unknown outcome is not physical completion. |
| Doctor / assistant | Diagnostic/action proposal -> bounded evidence or authorized engine command. | Human-only acceptance/elevation remains outside tool authority. |

### Lifecycle invariants and trust boundaries

- Acknowledged project edits survive supported crash recovery under documented storage assumptions; cancelled previews are not commits.
- A job's input revision and content manifest never change in place. Backend changes create attempts; stale results cannot silently become current edits.
- Cancellation/revocation is fenced at result acceptance and commit, not merely signalled to a worker. Late work cannot recreate a grant.
- Normal client disconnection preserves authorized offline jobs; controller-loss device policy is a separate lifecycle rule.
- Preview association is keyed to source/frame/mapping/transform/request context. A matching rectangle shape alone is not evidence freshness.
- Device arming/admission, running, stop-requested, stopped/disarmed and unknown physical state remain distinct. Software acknowledgement cannot collapse those states into a physical-stop claim.
- Playback revisions and session epochs are explicit. Reconnect, crash recovery or project edits cannot replay authority or rearm.
- Parser, model, plugin, native worker and wire outputs are untrusted until validated. Checked core types do not make foreign code trusted.
- Per-user IPC pairing limits supported clients; it does not defend against a compromised same-user OS account. Native trusted exceptions weaken confinement explicitly, not invisibly.
- Source originals, committed motion and compact lineage are authoritative/user data, not cache victims. Disposal cannot invalidate active dependencies silently.

### Closure record and remaining execution

All 11 architecture areas and ARCH-001 through ARCH-041 are now recorded. No architecture preference question remains open. ARCH-041 revises execution order without reopening the settled module/trust contracts. Concrete implementation schemas, migration adapters, model/backend selection, dataset curation, numerical tolerance qualification, source/model lineage evidence, platform packaging and physical-device evidence remain delivery work under the architecture-first milestones and preserved M1-M8 acceptance scope.

This consolidation changes documentation only. It applies no production patches, runs no tests/proofs/benchmarks, refreshes no upstream baseline, qualifies no model/device/platform and creates no commit. Historical regression results are not fresh qualification of this document's target architecture.


Current status: **ARCHITECTURE DISCUSSION COMPLETE. Implementation, testing, proofs, benchmarks and hardware qualification remain unfinished.**

The user's requested completion signal is `pineapple`. Architecture closure satisfies the discussion milestone only; it is not a claim of implementation, release readiness or physical qualification. Subsequent changes to settled contracts must record their decision source and supersession explicitly.
