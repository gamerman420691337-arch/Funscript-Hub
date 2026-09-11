# ADR 0003: Constrained multimodal synthesis, not implied semantic inference

Date: 2026-09-11
Status: Accepted implementation direction; integration and quality qualification remain separate
Scope: B4 pure text/preset, still-image prompt gate, audio-envelope and beat synthesis

## Context

Pulsar must support text-driven synthetic motion and still images with explicit prompts. These paths cannot claim that a keyword parser is a local language model or that a still image demonstrates observed motion. Audio decoding and feature extraction are effectful worker responsibilities; mapping validated audio features to neutral motion belongs in the pure core.

The product specification remains authoritative. This decision implements a deliberately constrained baseline interface, not full natural-language understanding, semantic image analysis, beat-detection accuracy, device qualification, or the entire B4 milestone.

## Decision

Use checked PatternSpec values with sine, triangle, hold, and pulse patterns. Expose a strict, documented key=value grammar. Require pattern, axis, duration, frequency, amplitude, and offset. Optional phase, sample_hz, and duty have explicit defaults. Unknown, duplicate, malformed, or unsupported intent fails; no keyword-based interpretation or hidden assistant claim is permitted.

Example:

```text
pattern=sine axis=stroke duration=5s frequency=1hz amplitude=0.2 offset=0.5
```

A still-image request must contain a valid explicit prompt. The same checked synthesis plan is used; image pixels are not interpreted as motion by this module.

All actions carry EvidenceKind::Synthesized, including audio-derived actions. A valid audio feature is not direct observation of intended physical motion.

## Units, timing, and bounds

Project timestamps remain checked integer nanoseconds. Decimal prompt durations have at most nine fractional digits and are converted without floating-point rounding. Duration must be positive and at most 24 hours.

Waveform amplitude is peak deviation around offset. The complete requested range must lie inside neutral [0,1]; invalid configurations fail rather than clip or normalize. Hold requires zero amplitude, frequency, and phase and emits two equal-valued endpoints.

Waveform frequency is limited to 50 Hz. Sample rate is 1 through 1000 Hz, with at least four samples per cycle. Pulse high and low intervals each require at least two sample intervals. These are bounded representation rules, not a claim that pulse harmonics are band-limited or that a device can safely reproduce the waveform.

The sampling grid is computed using integer arithmetic, and the exact requested end timestamp is included. Phase defines the mathematical waveform evaluated at each grid point. Discontinuous pulse transitions are represented on this grid and may be delayed by less than one sample interval; this module does not claim continuous-time edge fidelity or device-admitted velocity.

One request supports at most six unique axes with one master duration. Aggregate action count is checked before allocation and limited to 1,000,000. Sample count and timing overflow fail before generation. Missing axes are omitted, never invented as zero tracks.

## Audio contracts

AudioEnvelopeSample accepts only finite levels in [0,1] and nonnegative bounded timestamps. Samples must be strictly ordered. Mapping is offset + amplitude * level, without per-recording range stretching. Silence remains offset. A missing prefix is an explicit unavailable gap, not interpolated motion.

Beat input is a sequence of supplied peak timestamps, not detector output generated here. Triangular pulses have explicit positive rise and fall times and a bounded unipolar amplitude. Overlapping pulses or pulses outside the requested duration fail rather than shift peaks or shorten pauses. Empty beats produce an explicit stationary hold.

Worker decoding, sample calibration, silence thresholds, beat detection, source lineage, and live latency remain separate contracts. Standard export must continue to reject unresolved gaps unless the user explicitly resolves them through the engine.

## Ownership and effects

The core has no file, network, model, runtime, GUI, or hardware dependencies. Clients submit commands to the engine; they do not synthesize authoritative project state themselves. Workers decode inputs, call checked core synthesis, and return candidates with compact lineage. Only the engine commits candidates, checks revisions and grants, and writes exports.

Local assistant integration remains a separate inference capability. The constrained preset parser is always identified as such. Physical playback still requires independent profile adaptation, admission, qualification, and controller authority.

## Evidence and falsifiable tests

The paired tests include mathematical known-answer points for sine/triangle quadrature, pulse plateaus, exact nanosecond duration, explicit hold behavior, and beat peak timestamps. Parameter sweeps check neutral bounds, strict time ordering, phase handling, and synthesized labels. Metamorphic audio tests verify linear amplitude scaling without changing timestamps or stretching silence.

Adversarial cases cover unknown prompt fields, duplicate keys, invalid units, numeric aliases, nonfinite/out-of-range values, allocation limits, duplicate axes, deserialization bypass attempts, missing still-image prompts, unavailable audio prefixes, overlapping beats, and out-of-window pulses.

Test source existence is not a passing receipt. The implementing task must report exact executed commands and results. These tests do not establish neural accuracy, real-audio beat-detection quality, end-to-end throughput, formal proof, or hardware safety.

## Alternatives rejected

- Free-form keyword heuristics: imply unsupported semantic understanding and hide ambiguity.
- Automatic full-range normalization: turns quiet audio or shallow requested motion into invented large strokes.
- Silent sample-rate reduction: weakens requested representation without a visible plan change.
- Dropping missing audio intervals: allows consumers to interpolate fabricated motion across absent evidence.
- Retiming overlapping beat pulses: changes user-supplied peaks and pauses.

## Bounded PCM feature extraction

The pure core additionally accepts already-decoded mono f32 PCM with explicit sample rate and window size. Samples must be finite and calibrated to [-1,1]; input beyond that contract fails rather than being silently normalized. The input cap is 96,000,000 samples, the sample-rate cap is 192,000 Hz, and one analysis window spans at most one second. Aggregate feature count is checked before allocation against the existing synthesis-action budget. Engine admission must account for the PCM buffer, feature vectors, and generated actions together.

Nonoverlapping RMS windows preserve absolute input amplitude. Window timestamps use integer sample-index-to-nanosecond conversion, and an explicit final endpoint retains the last window level rather than inventing a return to baseline.

Amplitude-onset events are upward RMS crossings of the fixed engineering default 0.35. They are explicitly not musical beat detections. Window-start timestamps carry up to one window of onset uncertainty. Steady loud sound can produce one initial onset; threshold jitter can produce multiple amplitude events. Rhythm-aware beat inference, calibration against real audio, and robustness to noise remain open research/qualification work.

Tests cover RMS known answers, the exact 0.35 threshold rule, integer timestamp quantization, silence, quiet amplitude preservation, linear scaling, malformed PCM, invalid sample/window parameters, and partial final windows. No real-audio beat-accuracy claim follows from these unit results.
