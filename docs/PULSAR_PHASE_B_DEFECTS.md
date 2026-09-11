# Pulsar Phase B defect and evidence inventory

Status: B1 baseline. Historical cases are retained obligations, not automatically
fixed by the architecture migration.

Authoritative scope: `PULSAR_ARCHITECTURE_FIRST_EXECUTION_PLAN.md` B1-B6,
`PULSAR_SOFTWARE_SPECIFICATION.md`, and `PULSAR_PHASE_A_MIGRATION.md`.
Machine-readable inventory:
`research/phase-b/regressions/legacy-20.json`.

## Evidence vocabulary

- `RECOVERED_HISTORICAL`: original cached assertion preserved; not a current execution.
- `CONTRACT_PASS_REPORTED`: a named current-interface pure test was reported passing; production correspondence remains separate.
- `PRODUCTION_REPRODUCED`: a candidate-bound enabled production path demonstrates the defect.
- `PRODUCTION_REGRESSION_PASS`: the enabled path passes its independently defined regression after repair.
- `REAL_MEDIA_EVALUATED`: a permitted independently annotated subject was measured; report only its declared coverage.
- `HYPOTHESIS`: falsifiable investigation item, not a discovered vulnerability.
- `UNRESOLVED_EXTERNAL`: required source, permission, annotation, hardware or human decision absent.

## Historical 20-case map

Recovered from cached M1 assertions associated with selected historical baseline
`7a4f5671c14731b238b64c4827294ce029928dda`. Original legacy bytes remain
unchanged. New metric KATs are not these twenty cases.

| ID | Historical assertion | Required invariant | Current owner/workstream |
| --- | --- | --- | --- |
| M1-01 | `m1_reject_negative_box_extent` | Negative width/height must not create a box. | B2 detector validation |
| M1-02 | `m1_reject_nonfinite_geometry_and_scores` | NaN geometry and infinite confidence must be rejected. | B2 detector validation |
| M1-03 | `m1_clipped_box_center_matches_visible_bounds` | Clipped box and center must describe the same geometry. | B2 coordinate propagation |
| M1-04 | `m1_unknown_class_does_not_panic` | Unknown class must not index past class labels. | B2 detector schema |
| M1-05 | `m1_zero_classes_cannot_create_detection` | Zero classes cannot produce a detection. | B2 detector schema |
| M1-06 | `m1_direct_rejects_reversed_corners` | Declared corner decoder must reject reversed corners. | B2 detector validation |
| M1-07 | `m1_direct_clips_corners_without_guessing_center_format` | Negative corner coordinates are clipped, not guessed as center format. | B2 decoder contract |
| M1-08 | `m1_direct_rejects_nonfinite_score` | NMS-free decoding must reject infinite confidence. | B2 detector validation |
| M1-09 | `m1_flow_advances_box_and_center_together` | Flow must move visible box with tracked center. | B2 tracking/GUI |
| M1-10 | `m1_scene_cut_clears_old_targets_and_points` | Scene cuts must clear previous target, probe and point identities. | B2 tracking lifecycle |
| M1-11 | `m1_confidence_change_does_not_replace_spatial_match` | Higher confidence on another target must not silently replace spatial identity. | B2 target association |
| M1-12 | `m1_missing_detection_ages_without_flow` | Missing detections age tracks even without an available flow field. | B2 track aging |
| M1-13 | `m1_degenerate_boxes_do_not_seed_points` | Reversed or zero-area regions must not seed tracking points. | B2 point seeding |
| M1-14 | `m1_zero_requested_mask_points_do_not_panic` | Zero requested mask points must return safely without points. | B2 point seeding |
| M1-15 | `m1_forward_flow_is_not_its_own_backward_flow` | A forward field is not independent backward-flow evidence. | B2 flow evidence |
| M1-16 | `m1_constant_displacement_never_invents_stroke` | Constant displacement must not invent full-range movement. | B2 stroke extraction |
| M1-17 | `m1_monotonic_decreasing_motion_keeps_direction` | Decreasing displacement must preserve direction. | B2 stroke extraction |
| M1-18 | `m1_trailing_pause_does_not_add_reversal` | Trailing plateau must not add an invented reversal. | B2 stroke extraction |
| M1-19 | `m1_keyframe_reduction_preserves_both_pause_boundaries` | Simplification must retain both plateau boundaries. | B2 stroke extraction |
| M1-20 | `m1_equal_area_resize_updates_flow_geometry` | Equal pixel count does not imply equal width/height layout. | B2 flow geometry |

The forward/backward case's historical expectation of zero error when no
independent backward pass exists must not become fake validation. The new
contract must represent backward validation as unavailable or evaluate a truly
independent reverse field. A translated synthetic field is not neural tracking
qualification.

Current pure-module correspondence reported by B2 is recorded separately in
`research/phase-b/regressions/current-contract-correspondence.json`. This
inventory does not claim those kernels are all connected to the worker or that
the old full-range extraction behavior was safely ported.

## Additional investigations, not unsupported bug claims

| ID | Classification | Falsifiable investigation | Evidence needed |
| --- | --- | --- | --- |
| B1-H01 | HYPOTHESIS | Global camera motion may be mistaken for target-relative stroke movement. | Known camera-only translation/rotation fixture plus independently labelled real camera-motion subjects. |
| B1-H02 | HYPOTHESIS | Static output, detection loss or low texture may be turned into confident inferred movement. | Static/textureless/occluded fixtures, explicit evidence-kind and review-span assertions. |
| B1-H03 | HYPOTHESIS | Decimated sampling may miss shallow/rapid reversals while improving speed. | Held-out frequency/travel strata with identical source/runtime identities across presets. |
| B1-H04 | HYPOTHESIS | Viewer association may fail across seek, cut, resize, orientation or projection changes. | Deliberately late real worker results and displayed-frame/transform identity checks. |
| B1-H05 | HYPOTHESIS | Parser/model declarations may disagree with tensor rank/layout or decoder meaning. | Malformed manifests, exact shape limits, nonfinite tensors and real pinned-runtime probes. |
| B1-H06 | HYPOTHESIS | A broad review flag may game material-section recall. | Flag-duration and false-alarm measures plus recorded human effort, not recall alone. |
| B1-H07 | HYPOTHESIS | Neighbouring frames/derived sources may leak across training and held-out data. | Immutable manifests, source-group review, content/near-duplicate audit and holdout access history. |
| B1-H08 | HYPOTHESIS | Neutral output may be rescaled or device-adapted before quality measurement. | Candidate/export lineage receipts and independently retained neutral stroke travel. |

## External gates kept explicit

No permitted real-media corpus, independent human references or review/edit
sessions were supplied to B1. The corpus placeholder is empty. No 80%, 90%, 95%,
reference GPU speed, five-minute editing or ten-minute review claim is made.

The measurement harness reports per-axis precision/recall, exact 100 ms and 20%
boundaries, three explicitly unapproved material-section hit views, flagged
duration burden and category-separated human effort where real sessions exist.
It cannot decide permissions, neural validity, user preference, source
independence, model lineage clearance or physical qualification.

## B1 exit versus full Good phase

B1 can establish a reproducible inventory, independent analytic metric oracles,
schemas and explicit unresolved acceptance definitions. Completing B1 does not
close B2-B6. The Good phase requires their enabled functional paths and declared
acceptance evidence; unsupported capability stubs and unavailable external
subjects must not be relabelled successful.


## Successor worker evidence and readiness update (2026-09-11)

Four current-worker signal regressions now pass; the audio timestamp-gap defect has
RED-to-GREEN evidence. These are reconstructed current-contract tests, not the
original legacy20-suite execution. Large framewise candidate publication/transfer
remains OPEN: 54,000 actions exceed 2.48 MB versus 512 KiB publication and 1 MiB
inline-read limits. Corpus and human-effort evidence remain absent.

See `research/phase-b/B1-SUCCESSOR-2026-09-11.md` and `research/phase-b/regressions/current-contract-correspondence-2026-09-11-successor.json`.
Earlier hash-bound measurement receipts and historical source assertions are
unchanged. Full Good phase remains unfinished.
