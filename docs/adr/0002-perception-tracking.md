# ADR 0002: Checked perception and bounded temporal tracking

Status: accepted implementation choice; pure-kernel checks passed. Product
integration and real-media qualification remain separate gates.

Authority: Pulsar software specification, ARCH-002/003/040/041, and Phase B2.
This ADR selects implementation defaults. It does not replace user decisions,
reduce modality scope, or claim neural accuracy, physical pose, or clean-room
lineage.

## Problem

The quarantined implementation mixed neural sessions, anatomy-specific target
selection, coordinate normalization, trajectory state, and signal generation.
Its point tracker sampled the forward field again when claiming a backward
check. Box propagation could move the center while leaving endpoints frozen.
Confidence ranking could replace spatial identity. Missing detections, source
cuts, and equal-area frame resizing needed independent regression coverage.

Those historical findings are not proof that the new production pipeline passes.
The new seam must own these invariants before worker adapters can reuse them.

## Decision and interface

A pure `pulsar_core::perception` module owns detector canonicalization, temporal
identity tracking, bounded point seeding, sparse optical flow, and an explicitly
limited target/background translation summary. Native inference, media decoding,
frame identity construction, span receipts, and job authority remain outside.

- `decode_detections` accepts one declared tensor layout and exact resize or
  letterbox transform. It validates rank, element count, all finite values,
  class scores, class indices, box extents, and work limits. Geometry is clipped
  after inverse transform; endpoints and center cannot disagree.
- Supported output layouts are channel-major pixel center/extent YOLO and
  end-to-end pixel corners/score/class YOLO. Layout or normalized coordinates
  are never guessed from tensor length. Unsupported model formats need explicit
  adapters, not heuristic fallback.
- Non-maximum suppression is class-aware and deterministic. At most 256 boxes
  cross the seam. Scores remain uncalibrated detector scores.
- `TargetTracker::with_namespace` uses an attempt-unique identity namespace.
  Engine callers must use it rather than the isolated-local `new` convenience.
  Track identifiers are never reused after a cut or context reset.
- Source version, placement, transform, seek generation, and image dimensions
  reset tracking context. Source frame ordinals must otherwise increase.
  Invalid updates leave state unchanged.
- Association is bounded, class-aware, one-to-one, predicted-position based,
  and independent of confidence ordering. Near-equal alternatives produce
  explicit ambiguity instead of arbitrary identity assignment.
- Explicit missing updates age by elapsed source-frame ordinals. Intentionally
  unsampled frames between positive observations are unknown, not known absences.
  Short-lived constant
  velocity predictions translate both box endpoints, clip them together, and
  carry `Predicted`, never `Observed`. Expired tracks cannot be resurrected.
- Explicit cuts retire all previous target identities. This module consumes a
  cut decision; it does not pretend to be a qualified scene-cut detector.
- `track_points_fb` estimates source-pixel 2-D translation. It uses bounded
  integer SSD initialization, bilinear Lucas-Kanade refinement, template-gradient
  conditioning, and a separately solved current-to-previous trajectory starting
  at the forward destination.
- Rejected flow has no displacement, rather than a fabricated zero. Reasons
  include boundary loss, low texture, repeated-pattern ambiguity, brightness
  mismatch, nonconvergence, and forward/backward disagreement.
- `separate_target_motion` requires at least three consistent accepted points
  in each caller-selected region. It retains image-relative target motion and
  background motion separately; relative subtraction is unknown without both.
  Background image translation is a proxy, not measured physical camera pose.

No bounding-box size or aspect change is promoted into observed depth, roll,
pitch, yaw, or other true 6-DoF. Predicted boxes, optical flow, background
selection, and semantic target selection retain their distinct meanings.

## Scientific basis and alternatives

Lucas and Kanade formulate iterative image registration under local image
constraints. Baker and Matthews organize efficient template-gradient alignment
and its assumptions. These support an explicitly translational, bounded
implementation, not a claim that arbitrary media satisfies brightness constancy.

Sources:
[Lucas and Kanade, original paper](https://www.ri.cmu.edu/pub_files/pub3/lucas_bruce_d_1981_2/lucas_bruce_d_1981_2.pdf),
[CMU Lucas-Kanade framework](https://www.ri.cmu.edu/project/lucas-kanade-20-years-on/).

Kalal, Mikolajczyk, and Matas evaluate trajectory disagreement by tracking in
both temporal directions. The backward trajectory must use reversed image
ordering; sampling forward motion again does not implement that criterion.
A small cycle error is useful rejection evidence, not proof of correct identity.

Sources:
[original publication record and paper](https://dspace.cvut.cz/entities/publication/4f34fd8a-dffe-4857-96c2-42ed3d5a8449),
[author thesis, section 3.1](https://cmp.felk.cvut.cz/ftp/articles/matas/kalal-2010-phd.pdf).

Alternatives considered:

1. Whole-frame vertical SAD remains a useful image-motion baseline, but cannot
   independently distinguish camera from target. It is not substituted for
   semantic observations.
2. Unbounded dense or neural point tracking would add runtime, model, and
   resource obligations before this checked seam exists. It remains an adapter
   research option, not a silently loaded dependency.
3. Bounded integer block matching alone gives a transparent initialization but
   loses subpixel refinement and does not check local gradient conditioning.
4. A full assignment solver with appearance embeddings could improve crowded
   crossings. The present conservative association makes ambiguity visible;
   it does not claim appearance-based re-identification.
5. Pyramids, robust photometric costs, affine background estimation, occlusion
   models, and confidence calibration need held-out experiments before replacing
   these defaults.

Implementation was authored after reading legacy source and primary papers.
It is not represented as a clean-room implementation. Existing AGPL licensing
and historical attribution remain untouched.

## Selected defaults and resource bounds

These are technical defaults, not benchmark-qualified optimum values:

| Surface | Bound/default |
| --- | --- |
| Detector tensor | 10,000,000 elements; 100,000 rows; 256 classes |
| Output / retained tracks | 256 |
| Missing-track lifetime | 3 source-frame ordinals |
| Association distance | 0.25 image diagonal per frame, capped elapsed multiplier |
| Ambiguity margin | 0.02 image diagonal |
| Flow image | 1,048,576 grayscale pixels |
| Points per invocation | 64 |
| Patch radius | 3, configurable up to 5 |
| Integer search radius | 6, configurable up to 8 |
| Refinement iterations | 12, configurable up to 20 |
| Minimum gradient eigenvalue | 16 intensity-gradient units |
| Mean absolute brightness error | 20 intensity units |
| Forward/backward error | 1 source pixel |
| Region summary | at least 3 points; two-thirds support within 1 pixel |

A source-frame gap is not the same as elapsed time or calls to the tracker.
A positive associated observation does not expire merely because a preset
intentionally skipped frames. Explicit missing updates still accumulate the
elapsed source-frame interval. The worker selects a 30-frame absence budget;
this is not long-occlusion re-identification qualification. Fast tiers are not
qualified by these bounds alone.

## Experiments and evidence

Test-first integration file was written before the module. Initial command
failed with `E0432` because `pulsar_core::perception` did not yet exist. This is
a scaffold-red receipt, not a behavioral reproduction of the legacy program.

After implementation and parent-owned module registration:

```sh
CARGO_TARGET_DIR=/tmp/pulsar-phase-a-target CARGO_BUILD_JOBS=2 \
  cargo test -p pulsar-core --test perception --offline
```

Result: **32 passed, 0 failed, 0 ignored**.

The additional sampled-frame test produced a behavioral RED: a positive target
at source frame 32 incorrectly changed identity from `local.0` to `local.1`.
Removing pre-match expiry based on unobserved frames fixed it. Explicit absence
at frame 30 still expires correctly.

Coverage includes malformed detector layouts and scores, clipping/letterbox
geometry, deterministic class-aware NMS, confidence-independent identity,
constant-velocity box propagation, absence aging, cuts, seeks, stale-frame
rejection without state mutation, ambiguous matching, degenerate seeding,
zero-count masks, malformed frame buffers, equal-area changed-aspect frames,
stationary textured versus textureless input, known integer translations,
reversed-translation metamorphism, occlusion, and background-relative motion.

Two isolated source mutants were compiled outside the product workspace graph:

1. Reverse solve uses forward image ordering. The translation regression failed
   with cycle error `7.211102550927978` and
   `ForwardBackwardMismatch`: mutant killed.
2. Prediction no longer updates box endpoints. The box regression failed with
   x-min `12.0` instead of `14.0`: mutant killed.

The first mutation harness attempt had a bad dependency path and was rejected
as invalid evidence. A shared target directory also reused the wrong mutation
artifact; frozen-box evidence was rerun in its own target directory. Only the
actual assertion failures above count as killed mutations. Product source was
never changed to run these mutants. Temporary mutation sources were removed.

Historical case correspondence is semantic and interface-adapted. In particular,
the old normalized direct-output API is replaced by declared pixel schemas;
flow-driven legacy box propagation is not equivalent to the new velocity
prediction test. See the Phase B1 ledger for unresolved production correspondence.

## Worker adapter integration

The worker's existing normalized `RawDetection` adapter now delegates tensor
decoding to this core seam. It no longer owns a second detector validation/NMS
implementation. Normalization is retained solely for existing preview callers;
`StrokeTracker` converts back to checked source pixels for temporal reasoning.

`worker::tracking` binds gray buffers, actual observed detector boxes, context,
attempt-derived track identities, and source-pixel flow. It retains only the
previous selected observation and never uses a predicted box as an observation.

The first anchor is explicitly `Synthesized`. Cuts, missing detections,
ambiguous associations, identity discontinuity, and unsupported relative flow
produce no measured position. Accepted target-minus-background vertical motion
integrates by image height without full-range normalization and remains
`Inferred`. Automatic class/target selection and background-region selection
carry explicit unqualified review codes. A synthetic anchor after a gap does
not establish evidence for the gap.

Commands and results:

```sh
CARGO_TARGET_DIR=/tmp/pulsar-phase-a-target CARGO_BUILD_JOBS=2 \
  cargo test -p pulsar-engine worker::tracking --offline
CARGO_TARGET_DIR=/tmp/pulsar-phase-a-target CARGO_BUILD_JOBS=2 \
  cargo test -p pulsar-engine worker::detector --offline
```

Results: **9 tracking tests passed; 2 detector adapter tests passed**.

Synthetic known-answer scenarios show that two-pixel whole-image translation
cancels in the relative stroke; two-pixel target-only translation produces
`2/128 = 0.015625` amplitude and reverses back without normalization. Tests also
cover stride 32, 128-character attempt IDs, low texture, source cuts, absence,
and malformed input. These are adapter/pixel KATs, not real-media detector or
anatomical stroke accuracy results.

## Limits and next experiments

No result above qualifies real detector accuracy, subpixel error across a
curated corpus, long occlusion recovery, scene-cut detection, 3-D pose, neural
runtime correctness, device safety, or CPU/GPU speed floors.

Required follow-on experiments:

1. Run the real worker and GUI on moving targets, absent detections, cuts, seeks,
   resizing, late results, and projection changes. Bind receipts to actual
   source, model, runtime, transform, attempt, and displayed frame.
2. Curate target/background point labels and held-out camera-motion sequences.
   Measure flow endpoint error, rejection recall, target drift, and identity
   switches separately from final stroke error.
3. Compare integer initialization plus refinement against pyramidal LK and
   qualified learned trackers at fixed resource budgets and explicit presets.
4. Calibrate thresholds from held-out media. No hand-chosen score threshold is
   a probability, and synthetic perfect translations are not real-media accuracy.
5. Extend mutation/fuzz campaigns over tensor contracts, source transitions,
   association ties, buffer dimensions, arithmetic, and boundary sampling.

