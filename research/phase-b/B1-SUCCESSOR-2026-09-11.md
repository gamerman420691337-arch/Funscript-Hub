# B1 successor: worker regression evidence and open readiness

Status: **four current-worker signal regressions pass; full Good phase unfinished**.
This supplements, rather than rewrites, the original B1 handoff and its nine
hash-bound measurement artifacts.

## Executed worker result

Command:

```sh
CARGO_TARGET_DIR=/tmp/pulsar-phase-a-target CARGO_BUILD_JOBS=2 cargo test -p pulsar-engine worker::good_tests --offline -- --test-threads=1
```

Observed result: exit 0; 13 passed, zero failed, zero ignored, 49 filtered out;
test execution 1.76 seconds. Tool execution session: 93860.

The checks generate authored synthetic media through FFmpeg and execute the
current worker manifest path. They do not rerun the original preserved legacy
suite or qualify real-media neural accuracy, camera/target separation, reference
hardware speed or physical devices.

| Historical ID | Current worker test | Independent expected behavior |
| --- | --- | --- |
| M1-16 | `m1_worker_constant_displacement_never_invents_stroke` | Repeated identical textured frames hold neutral 0.5; no invented stroke. |
| M1-17 | `m1_worker_monotonic_decreasing_motion_keeps_direction_and_shallow_travel` | Known negative one-pixel shifts preserve exact 1/64 neutral increments from 0.5 at 125 ms intervals. |
| M1-18 | `m1_worker_trailing_pause_does_not_add_reversal` | Return to neutral at 250 ms remains flat through 500 ms; no extra reversal. |
| M1-19 | `m1_worker_preserves_both_pause_boundaries_for_peak_and_trough` | Both 125 ms and 375 ms plateau anchors survive, for positive and negative one-pixel movement. |

These four cases replace the earlier handoff's lack of current worker
correspondence. The historical inventory and original assertions remain
unchanged. The sixteen earlier pure-contract correspondences remain separate
evidence; in particular, constant-velocity box prediction alone does not prove
historical dense-flow box propagation correct.

Other worker checks establish six-axis explicit patterns, strict text/pattern
source-byte binding, no video probing for those inputs, decoded absolute audio
RMS, source-clock receipts, still-plus-prompt handling and explicit rejection of
missing prompts or multiframe media labelled as still images.

## B1-AUD-01: audio timestamp-gap defect, RED to GREEN

An authored 16 kHz Float32 audio fixture contains 1,920 samples, with a one-second
PTS gap introduced after 50 ms using FFmpeg. An independent ffprobe assertion
confirms a source timestamp jump greater than 0.5 seconds.

Before the parent repair, this input returned a generated 120 ms candidate:
raw PCM had lost the timestamp gap. The worker filter was RED, with 12 passes
and one failure, exit 101 in tool session 99763.

After the parent continuity-admission repair, the same independently checked
fixture returns typed Unsupported and produces no candidate artifact. The
current 13-test worker filter passes. Continuous audio also retains expected
RMS and silent boundaries, and its receipt records source/resampled sample
counts, source sample rate, rational time base, initial source PTS and explicit
origin normalization.

This is a fail-closed repair for the supported continuous-audio path, not
implementation of discontinuous-audio repair or faithful gap-preserving
generation. It does not qualify musical beat tracking, general audio quality or
physical synchronization.

## B1-RDY-01: large candidate publication and transfer remains OPEN

Severity: **P1 readiness blocker**. Parent inspection identified a 512 KiB
publication program limit (`authority.rs` `PROGRAM_LIMIT`) and the 1 MiB
inline control framing limit used by candidate reads. Current worker framewise
action production can create 54,000 actions for a 30-minute, 30 fps subject.
The parent's synthetic serialization sizing exceeds 2.48 MB, larger than both
enabled-path limits.

This is source-backed size/readiness evidence, not a completed 30-minute media
benchmark. The small worker fixtures do not prove nominal long-video readiness.
The defect remains open; no fix or qualification is claimed here.

Closure requires bounded, authorized bulk candidate storage/transfer and client
consumption, or a separately proved lossless semantic reduction retaining
timing, both pause boundaries, shallow movement, reversals and evidence.
Silently dropping samples, stretching motion or weakening provenance to fit
inline limits is not an acceptable closure.

Source ownership remains the parent engine/protocol/client workstream. The
existing product scope, 30-minute quality/effort subjects and bounded-control
architecture remain unchanged.

## Unchanged external gates

Permitted real-media corpus: 0 supplied to B1. Independent human annotations:
0. Recorded human review/edit effort: 0. Reference-hardware campaigns: 0.
Category taxonomy, segmentation/plateau convention, flag-hit policy and
statistical qualification rules remain unresolved.

The B1 metric harness's 827 passing checks remain synthetic measurement
evidence. The later 13 worker tests are a separate execution class. Neither
count licenses a full Good-phase completion or release claim.
