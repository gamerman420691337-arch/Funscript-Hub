# Pulsar clients

This crate owns command grammar, presentation state, independent preview clocks,
disposable gesture drafts, and authenticated local-protocol calls. It does not
depend on `pulsar-engine`, decode media, load inference models, write project
databases, host extensions, or operate physical devices.

## Public entry points

- `command_needs_engine(&[OsString]) -> bool` lets the executable avoid engine
  startup for help, version, completion, and parse-error paths.
- `run_cli(Vec<OsString>, &Path, &Path)` accepts executable argv, endpoint, and
  the engine's owner-private bootstrap credential path.
- `run_gui(PathBuf, PathBuf)` opens `PulsarDesktop` with the same connection
  inputs. With no CLI subcommand, the desktop opens.
- `session::EngineApi` is the test seam shared by all client orchestration.

The engine owns discovery, single-instance exclusion, its endpoint and bootstrap
token. First-party CLI and GUI reuse `first-party.session.json` in the engine's
private state directory, preserving one first-party actor's project grants.
The credential file contains a public session ID and a distinct secret bearer
token; a public session ID alone never authenticates a request. Third-party
pairing is separate. A revoked credential is not silently replaced
with a newly paired identity. Ambiguous request outcomes are not automatically
replayed under a fresh request ID.

## Active paths

- CLI and GUI create/open projects through the engine.
- Import creates an immutable engine-owned media source.
- Default/fast stroke generation starts an engine job, not a GUI-owned pipeline.
- Candidate review, explicit rebase, revision-checked commit, undo/redo, and
  standard funscript export use the same typed protocol.
- CLI batch and scan/audit select local files with bounded directory traversal
  and drive the same generation path. They do not contain another generator.
- GUI preview requests exact source time; the decoder supplies actual source
  timestamp and frame ordinal. Source version, placement, projection, seek and
  request generations must match before frame/evidence becomes visible.
- Boxes are actual normalized source-plane observations. Other coordinate
  spaces are not guessed. An absent observation is not replaced by a fixed ROI.
- Raw frame artifacts are bounded, layout-checked and SHA-256 checked before
  display. Preview state and clocks do not control a physical device.
- GUI point drags retain the original revision, remain local drafts during the
  gesture, and submit one `ApplyEdit` at release. The engine alone commits.

## Explicit migration and qualification gaps

Preserving CLI grammar is not a claim of complete behavioral compatibility.
The original command names, flags, `audit` alias and `stash query` alias remain
parseable. Unsupported settings return `ProtocolError::Unsupported` rather
than being ignored or returning fabricated results.

- `fix`, `doctor`, `info`, `audio-synth`, model administration and `bench` still
  require their engine/worker adapters. Existing product scope is not removed.
- Stash effects, extensions and assistant operation require brokered authority.
- `play` and physical GUI controls remain disabled until driver admission,
  per-configuration qualification and stopping evidence exist.
- Batch model selection, multi-axis and overwrite policies require additional
  typed contracts; current batch supports baseline stroke generation.
- Non-default legacy confidence, FPS, detrend/normalization, batch size, VR/POV,
  global-motion, keyframe-reduction and cut options are explicit unsupported
  settings, not silently mapped to unrelated behavior.
- A custom ONNX model requires explicit tensor dimensions/class count. CLI adds
  `--model-class-count`; the initial declared ABI is RGB/NCHW/channel-major YOLO.
  Imported bytes and runtime selection remain engine-owned and unqualified.
- Source-identity projection is supported; arbitrary VR/crop transforms are not.
- Independent desktop preview is not synchronized audio/video playback and has
  no qualified real-time deadline.
- Large motion programs still require paged/artifact-backed project reads and
  edit transactions instead of exceeding the bounded control-message ABI.

These are implementation/qualification obligations, not passed release gates.
Legacy presentation code remains historical lineage evidence during migration;
this crate does not claim a clean-room implementation or replace attribution.

## Good-phase client paths

These additions supersede earlier pending audio/import/review entries. They are
working protocol clients, not claims of qualified model quality.

- `synthesize PROMPT [-o PATH] [--image PATH]` submits strict pattern text or a
  still-image-plus-required-pattern-prompt. Example syntax:
  `pattern=sine axis=stroke duration=5s frequency=1hz amplitude=0.2 offset=0.5`.
  This is deterministic preset parsing, not free-form language understanding,
  visually inferred motion, or a local AI assistant.
- `audio-map PATH --mode envelope|beats --axis AXIS --amplitude VALUE --offset
  VALUE` maps full-band audio. `beats` means amplitude-onset pulses with a fixed
  0.35 upward RMS threshold, not musical beat tracking.
- Legacy `audio-synth` now has a real engine path for explicit `--band full`,
  mode `envelope` or `beats`, threshold 0.35, and no bundle. It reports its
  conservative default mapping: offset 0.25 and amplitude 0.5. Unsupported bands,
  custom thresholds, vibration mode and bundles remain typed errors.
- `import-script PATH [--project ID]` snapshots/imports a funscript as an
  uncommitted engine candidate. It never overwrites the original.
- `review PROJECT [--candidate ID]` requests read-only localized diagnostics.
  This does not silently implement the different legacy speed/jitter doctor.
- `merge-candidate PROJECT CANDIDATE --axes stroke,yaw [--start S --end S]`
  merges selected axes and an optional checked project-time range. The engine
  preserves unrelated tracks, protected neighborhoods and revision authority.
- `export-axis PROJECT PATH --axis yaw` explicitly exports one selected axis.
  Standard generation exports Stroke; single-axis synthesis exports its declared
  axis. A multi-axis merge cannot silently imply a single-axis export.
- All CLI generation paths query the completed candidate and surface review
  flags before committing/exporting. Stale base revisions fail rather than
  rebasing implicitly.
- Desktop Generator exposes these engine paths. The Doctor tab provides
  read-only evidence/protection diagnostics. Amber timeline bands mark candidate
  review intervals; grey bands mark known protected regions.
- Desktop timeline now selects all six axes, with Stroke default. Every gesture
  preserves other tracks and commits against its original revision. Timeline
  duration remains project-wide rather than changing with the selected axis.
- GUI export names the selected axis and uses `ExportAxis`. Physical preview and
  device playback remain separate; no device commands are added.

Client unit tests establish orchestration and checked presentation behavior only.
Real-media behavior, pattern/audio quality, physical behavior and complete legacy
behavioral compatibility still require separate evidence.


## Large-program client transport

Wire v2 snapshots carry motion descriptors, never inline full programs. The shared
`SessionClient` authenticates chunked downloads and checks the complete length,
SHA-256, axis/action/gap summaries, and project/revision/candidate binding before
publishing an Arc-backed local snapshot. Partial bytes never become a timeline.
The bounded cache holds at most eight artifacts and 128 MiB of encoded content;
individual transfers retain the engine's 64 MiB admission bound.

CLI and GUI use this same hydration path. Bulk credentials are sent only to the
configured control endpoint's exact `bulk.sock` sibling. Lease epoch, direction,
range, digest and non-renewed expiry are checked before use. Transfer errors are
not blind-replayed; incomplete uploads are abandoned when possible.

GUI gestures serialize checked edit values, with explicit gaps but no caller
evidence or provenance. Finalization produces a candidate; the existing
revision-pinned `CommitCandidate` API remains the sole commit path. A failed
commit retains the candidate identity in its error rather than silently retrying.
`SessionClient::from_credentials` supports independently paired scoped actors
without adopting or persisting first-party credentials. Read authority is
separate and required to hydrate resulting motion.

Editing preserves explicit gaps and full action detail. Timeline lines clip
unavailable intervals and shade them explicitly; the editor does not synthesize
motion through gaps. Large point edits use binary-search lookup rather than
quadratic rescanning. These implementation bounds and tests are not performance,
neural-quality, device-safety, or platform qualification.

