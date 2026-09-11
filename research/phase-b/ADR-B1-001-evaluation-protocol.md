# ADR-B1-001: Independent, bounded quality measurement

Status: selected technical baseline; unresolved release-policy choices listed below.
Scope: B1 groundwork, with measurement interfaces for B2-B6.
Authority: `docs/PULSAR_SOFTWARE_SPECIFICATION.md` QUAL-01 through QUAL-11,
DATA-02 through DATA-10, TEST-19 and the B1 architecture-first milestone.

## Context and accepted product decisions

Stroke precision and recall are separate. Preview floors are 80%, with a 90%
target; Stable floors are 95%, evaluated within each required category. A
reversal timing error strictly greater than 100 ms is material. Neutral
stroke-travel error strictly greater than 20% of reference travel is material.
Wrong targets, missed strokes, extra strokes and missed shallow strokes remain
material regardless of amplitude tolerance. Flag recall must reach 95% on
independently annotated materially incorrect sections. Stable human budgets are
at most five active-edit minutes and ten total review-plus-edit minutes per
30-minute subject, for at least 95% of subjects in each category. Preview reports
effort without using the Stable effort ceilings as Preview blockers.

These thresholds are not measurements achieved by this repository.

## Selected measurement representation

Annotations identify source subject, evaluation window, target, axis and each
stroke. Times use canonical decimal strings parsed as signed 64-bit integer
nanoseconds. Positions use integer millionths of neutral travel, inclusive
0..1,000,000. A zero-travel interval is a plateau, not a stroke. An annotator must
identify both plateau boundaries in the upstream timeline; segmentation of
plateau reversals remains a policy decision, not a hidden evaluator heuristic.

This version evaluates annotated strokes, not raw sample arrays. It does not
silently extract or rescale generated movement and does not use device-adapted
output as the neutral oracle. Converting a product candidate to this schema must
retain the source candidate identity and declare any quantization. For standard
integer funscript positions, multiplication by 10,000 is exact. Native positions
with finer resolution need an explicit conversion receipt.

Matching uses target, axis, direction, both endpoint timing deviations and
reference-relative travel error. Both endpoint deviations must be <=100,000,000
ns and absolute travel difference * 100 must be <= reference travel * 20.
Integer cross multiplication avoids a floating-point boundary error. Missing
small strokes cannot disappear into an absolute tolerance.

Eligible pairs form a bipartite graph. Deterministic augmenting paths find a
maximum-cardinality one-to-one matching. A candidate can explain at most one
reference stroke. This is not a minimum-cost temporal alignment, and temporal
error distributions describe matched strokes only; unmatched identifiers remain
visible as false positives or false negatives. Whether a crossing assignment
between near-simultaneous same-target strokes should be forbidden remains a
segmentation-policy question. No such heuristic is silently added.

Precision is matched/predicted; recall is matched/reference. A zero denominator
is null, never 100%. Each axis and category is reported separately. Synthetic
and real-media evidence are never pooled. The runner intentionally emits
`release_qualified: false` even when all observed metrics equal one.

## Flags, uncertainty and burden

Material sections are authored independently of evaluated review flags. Their
reasons include timing, travel, wrong-target, missed/extra movement and other
documented material defects. Unions of flags avoid double-counting overlapping
durations. Touching a section boundary with zero overlap is not detection.

Three descriptive recalls are emitted: any positive overlap, at least half of
section duration covered, and full duration covered. These are alternative
measurement views, not three approved definitions of the 95% release gate.
Flagged duration, nonmaterial flagged duration and duration-based material
precision expose the trivial flag-everything strategy. A whole-clip flag may
reach full recall while imposing maximal burden; it is not accepted quality.

Scores do not substitute for error annotation. Missing annotations, zero
denominators, unrecorded human effort and unqualified hardware remain unknown.
No synthetic confidence score becomes a human-confirmed material-error label.

## Data, annotation and leakage protocol

Every source has a content digest, opaque leakage-group identity, allowed uses
and a permission evidence reference. Training permission does not imply
evaluation or redistribution permission. An owner must establish permission
before admission. The manifest validates records structurally; it cannot prove
a license or human claim is genuine.

Split by original source and leakage-relevant source families before extracting
frames or edited derivatives. Both content-digest and group collisions across
training, development and held-out data are rejected. Grouping still requires
human assessment: crops, transcodes, near duplicates, adjacent scenes and
derivative scripts must not evade grouping by changing bytes.

Development failures guide tuning. Freeze a held-out manifest with owner and
decision record before evaluation; maintain access records. Repeated tuning
against held-out failures invalidates the claim and requires a new evaluation
protocol or a separately preserved qualification set. A supplied freeze record
is evidence to audit, not proof of non-exposure.

Annotators record reference author, method, independence and review record.
Independent material-error labels should be adjudicated without exposing
generator flags. Double annotation and adjudication of ambiguous target,
occlusion, plateau and shallow-motion cases are proposed protocol tasks.
Rater names should be pseudonymous in shareable manifests. Original media and
private permission records remain local unless redistribution is authorized.

## Proposed coverage facets, not approved categories

- Target complexity: one target, multiple candidates, target crossing.
- Visibility: clear, partial occlusion, full loss, reappearance.
- Motion: shallow travel, pauses, plateaus, rapid reversals, static scenes.
- Camera: fixed, translation/rotation, zoom, cuts and dissolves.
- Media: VFR, orientation changes, aspect/resize changes, projection/VR.
- Domain: requested supported content categories and acquisition sources.
- Creative modality: text, still-image-plus-prompt, audio-only and mixed creative tasks, evaluated under a separate rubric.

Canonical category taxonomy and minimum independent-source counts must be
approved before category-level qualification. These facets do not reduce the
product's supported media scope.

## Human effort and speed

Effort requires a real-media rater/session receipt with active-edit and total
review-plus-edit durations. Active time cannot exceed total time. The runner
marks the Stable 30-minute budget only on actual 30-minute subjects; it does not
extrapolate short-clip effort into a passing video result. Missing sessions are
null and excluded from numerator and denominator, with eligible counts visible.

Execution records bind CPU, GPU, RAM, backend, model/runtime/configuration digests,
preset, axis mode and a qualification-record reference. Realtime factor is
media duration divided by measured wall time. This diagnostic ratio alone
cannot qualify reference hardware, accuracy or the performance matrix.
Stroke-only and six-axis runs remain distinct. No training, GPU work or
performance campaign was run for this B1 artifact.

## Falsifiable harness checks

Twenty manually derived synthetic KATs exercise exact timing/travel boundaries,
target/direction/axis mismatches, shallow misses, extras, undefined denominators,
one-to-one assignment, non-greedy matching, review coverage and exact large or
signed times. Deterministic metamorphic checks shift time origins without
changing results. Invalid numeric/schema inputs and corpus rights/leakage
violations must fail. Deliberate metric mutations must be killed by a named
golden witness; a surviving mutant blocks the harness result.

Expected values are not generated by Pulsar. These tests validate the measurement
implementation, not production tracking or neural output.

## Unresolved acceptance decisions

1. Canonical categories, pilot allocation and permitted real-media sources.
2. Stroke segmentation, plateau reversal convention and ambiguous target adjudication.
3. Material-section hit definition and false-alarm/review-burden acceptance policy.
4. Independent-source sample sizes, uncertainty method, multiple-category decision rule and holdout stewardship.
5. Creative modality rubric details, numeric thresholds and rater instructions.
6. Human review/edit effort collection protocol and reference-hardware campaign availability.

A source-cluster uncertainty analysis is preferable to treating correlated
adjacent strokes as independent trials, but no confidence level, interval
method or sample-size threshold is represented as user-approved. The five-hour
pilot mentioned in planning is not release qualification.
