# Actual GUI and native-model architecture scenario

Date: 2026-09-11
Product binary: SHA-256
`3499a17007134ea4584c66bd97f497bf032fd61d578358a4e8e8cfc24dac1cf0`.
Host/tool identities: `environment.txt`.

## Isolation and setup

Used private Xvfb display `:109`, software OpenGL and root-owned temporary engine
state `/tmp/pulsar-phase-a-qualified`. Final GUI invocation explicitly removed
inherited Wayland settings and isolated client configuration/data/cache paths:

```sh
env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET DISPLAY=:109 \
WINIT_UNIX_BACKEND=x11 LIBGL_ALWAYS_SOFTWARE=1 \
XDG_CONFIG_HOME=/tmp/pulsar-phase-a-qualified/client-config \
XDG_DATA_HOME=/tmp/pulsar-phase-a-qualified/client-data \
XDG_CACHE_HOME=/tmp/pulsar-phase-a-qualified/client-cache \
PULSAR_STATE_DIR=/tmp/pulsar-phase-a-qualified \
/tmp/pulsar-phase-a-target/debug/pulsar gui
```

The engine was started separately with the same state directory and explicit
`PULSAR_ORT_DYLIB` pointing to the recorded CPU runtime. GUI and engine ran the
same final binary. X11 window geometry was observed as 1280x820 at 0,0 before
input. Inputs were sent to that private display only.

An earlier final-rerun attempt inherited `WAYLAND_DISPLAY=wayland-0`, opened no
window on the test display and generated no project/export. That attempt was
not accepted as GUI evidence. Its owned process was stopped and the environment
corrected. Earlier exploratory output is not the final receipt.

## Observed scenario

1. Create project in the real GUI. Import the actual generated media fixture.
2. Generate Default stroke through the engine and confined worker. Review its
   revision-0 candidate without modifying the committed project.
3. Commit through the GUI. Engine acknowledges revision 1. Export the committed
   neutral funscript through the same engine API.
4. Request preview without a model. Actual frame pixels render; analysis is
   explicitly unavailable and no detector box is invented.
5. Enable the supplied custom ONNX model with declared RGB/NCHW 640x640 input,
   channel-major YOLO decoder and ten classes. A real native CPU generation job
   completes. Output remains prominently unqualified.
6. Analyze time 0. Actual native model produces one observation associated with
   decoded source frame 0 and rational source timestamp 0/1000 seconds. GUI
   renders its box, not a stationary decorative region.
7. Seek to 0.75 seconds and analyze again. Actual decoded source frame 6 carries
   timestamp 750/1000 seconds and zero observations. The old box is absent.

Final observed database state, queried read-only after UI actions:

```text
4b87747b-bb3d-41d6-859d-c07db7ccb980|Untitled motion|1
completed|2
```

The retained export contains eight standard `at`/`pos` actions and no internal
provenance graph, model identity, session credential or device payload. Its
constant position is not presented as a quality result.

## Artifacts

| Artifact | Evidence |
| --- | --- |
| `gui-fixture.mkv` | Generated FFmpeg test-pattern input, retained for replay. |
| `gui-candidate.png` | Reviewed but uncommitted candidate at project revision 0. |
| `gui-export-preview.png` | Committed revision 1, export acknowledgement and decoded preview with unavailable analysis. |
| `gui-export.funscript` | Actual GUI-triggered engine export. |
| `gui-native-job.png` | Custom-model declaration and completed native generation job. |
| `gui-native-frame-0.png` | One native-model observation attached to decoded frame 0. |
| `gui-native-frame-6.png` | Decoded frame 6 with no observation and no stale box. |
| `gui-durable-state.txt` | Read-only final project revision and completed-job count. |

## Limits

This is a generated test pattern, not held-out real-world evaluation. The model's
box on that pattern is not ground-truth semantic accuracy. The scenario proves
native execution, boundary propagation, committed export and observation
freshness. It does not qualify a detector, moving-target tracking, all projections,
human review burden, throughput, other operating systems or physical devices.
Moving-box/late-result/resizing-related association predicates also have separate
synthetic contract tests, not an implied real-media campaign.

No private source clip was uploaded. No model/runtime redistribution or clean-room
claim is implied; the existing model file and official CPU runtime were used as
local unqualified test inputs. No physical device was connected or actuated.
