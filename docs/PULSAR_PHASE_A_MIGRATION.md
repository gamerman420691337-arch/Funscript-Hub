# Pulsar Phase A migration and capability ledger

Date: 2026-09-11
Status: Architecture implementation record; qualification tracked separately

## Authority

The software specification remains authoritative. ARCH-041 and the
architecture-first execution plan set delivery order, not reduced product scope.
This architecture build is not a feature-compatible release replacement for the
historical application. Disabled or unsupported features below remain delivery
obligations. Parsing a historical command is not proof that its behavior works.

## Production ownership

| Surface | Owner | Boundary |
| --- | --- | --- |
| Executable client/engine/worker dispatch | `src/main.rs` | Composition and engine discovery only; no generation or project implementation. |
| IDs, checked units, geometry, evidence and motion | `pulsar-core` | Validated constructors/deserialization; no effects, GUI or native runtime. |
| Edit, job and device state algebra | `pulsar-core` | Pure transition predicates; engine adapters require separate correspondence evidence. |
| Command/query/event and error schemas | `pulsar-protocol` | Versioned requests, credentials, scopes, revisions and bounded framing. |
| Worker, tensor, artifact and extension-role schemas | `pulsar-protocol` | Explicit identities and budgets; schemas do not enable missing implementations. |
| Authoritative project state and history | `pulsar-engine` | Bundled SQLite transactions, authorization, deduplication, protected edits and revision fencing. |
| Source/model/runtime snapshots and provenance | `pulsar-engine` | Immutable content identities, admission, dependency leases, retention and export receipts. |
| Job execution and preview admission | `pulsar-engine` | Attempts, cancellation/revocation, resource accounting and confined child ownership. |
| Media decoding and native inference | Engine-owned worker role | FFmpeg/ffprobe and optional ONNX runtime; no project database or ambient device access. |
| CLI, GUI, batch and scan presentation | `pulsar-clients` | Same `EngineApi`; no direct project commits or inference initialization. |
| Viewer/editor | `pulsar-clients` | Independent preview time, exact displayed-context observation matching, transient gestures. |
| Device/assistant/doctor presentation | Clients over protocol | Unqualified effects fail closed; no shell or arbitrary engine-memory interface. |

Allowed graph:

```text
                  pulsar executable
                /         |         \
        clients role   engine role   worker role
              |             |            |
       pulsar-clients   pulsar-engine (isolated worker modules)
              |             |
              +--- pulsar-protocol ---+
                          |
                     pulsar-core
```

Both engine and clients may depend directly on core. Core cannot depend upward;
protocol cannot depend on engine/clients; engine cannot depend on clients/GUI;
clients cannot depend on engine/native inference/device implementations.
`scripts/check-architecture.mjs` checks the direct graph and source restrictions.
Its source scan is not a proof against every possible Rust alias or transitive
dependency effect. Compiler `forbid(unsafe_code)` is the core unsafe-code gate.

## Preserved source and replacement inventory

All original non-entry modules moved without semantic edits to
`legacy/pre-engine/src/`. Original `main.rs` and `Cargo.toml` are retained next to
that tree. This code is reference-only, not a transitional authority service.

| Historical surface | Replacement owner and allowed adapter | Removal/completion milestone |
| --- | --- | --- |
| `main.rs` orchestration | Clients parse commands; engine performs project/effect operations. | A1/A3 structure; B6 full behavioral compatibility. |
| `gui/` and `FunGenApp` | `PulsarDesktop`; client-only viewer/editor and engine API calls. | A1/A4 structure; B2/B6 feature restoration. |
| `batch/` | CLI iteration over the shared engine generation path. | A3/A6 structure; B6 scheduling/product behavior. |
| `video.rs`, `audio.rs`, `audio/` | Worker media contract and bounded decode; audio/mixed/live implementations remain missing. | B2/B4/B6. |
| `neural/` | Explicit model input/output contract, worker-only runtime and checked decoder. Advanced routing/tracking/zoo remain missing. | B2/B3. |
| `tracking.rs` | Checked observations and pure DSP kernels; historical tracker and flow defect inventory retained. | B1/B2. |
| `signal.rs`, `kinematics/` | Pure core signal/motion interfaces; no hidden full-range normalization fallback. Advanced processing remains missing. | B2/B4. |
| `funscript.rs` | Engine neutral export; complete historical import/fix behavior still missing. | B6. |
| `plugin/` | Role manifests and broker interfaces; no enabled arbitrary native/WASM plugin loader. | B6 implementation; C1/C2 confinement. |
| `sync/` | Core device transitions and engine admission interfaces; physical drivers disabled. | B5; C1/C2 physical qualification. |
| `stash/` | Historical CLI grammar only; remote automation disabled. | B6. |

Keep historical source until B6 migration and lineage review account for its
behavior and defects. Removing reference bytes requires an explicit follow-up;
their existence does not make the old application an enabled production path.

## Current capability classification

| Capability | Architecture-build status | Required later work |
| --- | --- | --- |
| One default per-user engine; paired clients | Implemented Linux path; schema-versioned state. | C1 adversarial campaigns; C2 Windows/macOS integration and qualification. |
| Checked six-axis neutral program | Implemented domain types; actual generation currently stroke-only. | B4 secondary-axis inference and modality behavior. |
| Project create/open, source import, edits, history | Implemented authoritative path; integration evidence recorded separately. | B6 complete editor/project workflows; C1 crash/fault depth. |
| Default/Fast stroke generation | Real decoded pixels and relative-motion candidate, explicitly unqualified. Preset labels are not speed/quality qualification. | B1/B2 correctness, B3 reference-hardware benchmarks. |
| Explicit custom CPU ONNX model | Worker-only declared tensor/decoder contract; unqualified custom model status. | B3 real-media accuracy, profiles, zoo, bundled distribution. |
| GPU backends | Feature/interface names do not establish acceleration. Unsupported paths must not silently claim GPU execution. | B3 NVIDIA/AMD/Apple/Intel implementation and C2 qualification. |
| Preview and observation overlays | Decoded frames and typed observations; separate seek token and resolved frame identity. No decorative fixed tracking region. | B2 real-media tracking, cuts/projection behavior and quality. |
| CLI `generate`, `batch`, `scan`/`audit` | Shared engine path for supported settings. | B6 complete legacy flag behavior. |
| CLI `fix`, `doctor`, `info`, `audio-synth`, `model`, `bench`, `play`, `stash` | Grammar/aliases retained where documented; unsupported behavior reported explicitly. | B3-B6 implementations; C2/C3 qualification as applicable. |
| Audio/text/image-plus-prompt/mixed/live input | Contract families and explicit unsupported responses; not completed modalities. | B4. |
| Maximum/advanced generation | Unsupported; no substitute labeled Maximum. | B2/B3/B4. |
| Device playback and Handy/Handy 2 | Admission/session contract only; physical actuation disabled. | B5 implementation and authorized physical evidence; C2 qualification. |
| Local assistant and extensions | Scoped proposal/approval and role interfaces; no enabled local chat model or arbitrary effects. | B3/B6; C1/C2 confinement. |
| Bundled offline portable packages | Not yet produced. Development worker discovery still uses installed tooling or explicit runtime path. | C2 clean-machine package qualification. |
| Interrupted-job recovery/fallback | State/dependency interfaces; only implemented, validated resume/fallback may run. No automatic equivalence assertion. | B3/B6 and C1 failure campaigns. |
| Full-project bulk edits/portable snapshots | Bounded inline path is limited; large programs and portable archives need dedicated implementations. | B6/C2. |

## Known defects and evidence boundaries

Historical 20-case regression inventory remains B1/M1 work: eight detector,
four tracker, three point-tracker, four stroke-processing and one flow-dimension
case. New core/protocol tests are not a rerun of those historical cases.
The historical GUI drew a fixed analysis region instead of detector telemetry.
Removing that misleading drawing does not qualify a real detector or tracking
quality. Synthetic moving pixels, synthetic observations, native-runtime smoke,
real-media evaluation and physical qualification are separate evidence classes.

Phase A independent review found credential disclosure, protected-edit bypass,
missing protection history, active-model eviction and aggregate-storage hazards.
Closure requires their regression evidence, not this ledger or renamed code.
See `PULSAR_PHASE_A_STATUS.md` for the current gate results and remaining blockers.
