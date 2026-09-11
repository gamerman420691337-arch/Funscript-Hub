# ADR 0005: Admit the audio source clock before extracting motion features

- Date: 2026-09-11
- Status: accepted implementation default; not media-quality qualification.
- Owners: engine media effects, core pure feature extraction, protocol review metadata.
- Scope: the current full-band audio envelope and amplitude-onset pulse recipes.

## Observed defect

The first Good-phase implementation decoded audio into a contiguous raw Float32
PCM stream and inferred time from sample index alone. A synthetic file with an
independently verified one-second internal PTS jump produced a roughly 120 ms
candidate. The later signal was incorrectly shifted earlier. An independent
review identified the same boundary failure.

Normalizing the first source timestamp to project zero is allowed when explicit.
Discarding an internal source discontinuity without a declared repair policy is
not.

## Decision

Before decoding raw PCM, the worker probes decoded audio frame timestamps,
sample counts, sample rate, and rational stream time base. It requires a bounded
single-stream index, stable sample rate where frame metadata declares it, and
positive checked sample counts.

Both adjacent and cumulative continuity are checked in integer rational
arithmetic. The tolerance is one source-clock tick, with admitted clock
precision no coarser than one millisecond. Cumulative checks prevent a small
per-frame timing error from accumulating unchecked. Missing timestamps,
non-increasing frames, discontinuities, clock drift, unsupported precision,
and changed declared rates reject the attempt before candidate publication.

The worker retains the exact first PTS, time-base numerator and denominator,
source sample rate/count, frame count, tolerance, and origin mapping in the
local receipt. Raw PCM is resampled to 16 kHz mono; decoded sample count must
match the admitted duration within one output sample. Memory and output bounds
remain enforced by admission and worker confinement.

Core then computes fixed-window RMS and upward amplitude-threshold onsets.
Neither RMS nor an onset threshold is a musical-beat detector. All resulting
motion remains Synthesized. Standard funscript export strips internal lineage;
local receipts retain it.

## Evidence

- A real FFmpeg-generated Matroska fixture independently proves its PTS gap
  with ffprobe before exercising the worker.
- The failing behavior was observed before the repair.
- The repaired worker returns typed Unsupported and publishes no candidate
  for the gap fixture.
- A continuous Float32 WAV fixture checks absolute RMS, quiet boundaries,
  source-clock receipt fields, and lack of full-range normalization.
- Pure adapter tests cover a negative source origin, overlap/gap/coarse-clock
  rejection, and cumulative drift rejection.

See worker audio.rs and good_tests.rs plus the final Phase B gate receipt.
Synthetic media proves these interface behaviors only.

## Consequences and unfinished work

Valid discontinuous or coarse-clock audio is currently unsupported, not silently
repaired. Future support needs explicit gap-preserving timeline resampling,
review spans, and independent real-media qualification. Codec delay, edit lists,
rate changes, and resampler behavior across a representative permitted corpus
remain research/qualification work. The current recipe is not a bundled local
assistant, semantic audio interpretation, or a beat-quality result.
