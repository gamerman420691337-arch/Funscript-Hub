# Phase B research and benchmark baseline

Status: B1 measurement groundwork. This directory does not qualify the Good phase,
Preview, Stable, neural accuracy, reference-hardware speed or physical devices.

Authoritative product thresholds remain in
`docs/PULSAR_SOFTWARE_SPECIFICATION.md`. Measurement choices in this directory
are selected technical proposals, not new user decisions. See
`ADR-B1-001-evaluation-protocol.md`.

## Artifacts

- `regressions/legacy-20.json`: exact recovered historical case inventory.
- `historical-regressions.json`: cached original Rust assertions, supplied by the parent task.
- `schemas/annotation.schema.json`: checked event and review-section format.
- `schemas/corpus.schema.json`: identity, rights, split and annotation provenance.
- `fixtures/metric-kat.json`: twenty analytic synthetic metric cases.
- `fixtures/synthetic-corpus.json`: a synthetic pipeline example, not a permitted real-media corpus.
- `corpus.pending.json`: empty real-media intake placeholder. Empty results cannot pass a release gate.
- `../../scripts/benchmark-good.mjs`: bounded offline evaluator and executable KAT/property/mutation harness.

## Run locally

```sh
lamu agent -- node scripts/benchmark-good.mjs --self-test
lamu agent -- node scripts/benchmark-good.mjs --corpus research/phase-b/fixtures/synthetic-corpus.json --split development
lamu agent -- node scripts/benchmark-good.mjs --corpus research/phase-b/corpus.pending.json --split heldout
```

Add `--report <new-file.json>` for an append-only result receipt. Existing receipt
paths are rejected. Node is a development measurement tool, not a Pulsar runtime
dependency. No data, model, telemetry or annotations are uploaded.

The runner refuses unpermitted use, training/evaluation leakage, changed
annotation digests, annotation path escapes, unknown fields, unchecked numeric
positions and invalid temporal identity. Bounds: 8 MiB per JSON artifact, 64 MiB
aggregate annotations, 256 manifest entries, 5,000 strokes/sections per artifact
and 250,000 candidate matching edges per subject. Larger campaigns must use
explicitly identified windows and aggregate independent source groups, not
silently discard excess events.

## Evidence classes

Synthetic mathematical fixtures establish evaluator behavior. Current-interface
regressions establish narrow code contracts. Real decoded-media smoke tests
establish only their exercised paths. Independently annotated, permitted,
held-out real-media subjects establish empirical quality. Hardware qualification
and recorded human-effort studies require their own evidence. These classes must
not be substituted for one another.

## Next intake

No permitted real-media subjects or independent human annotations were supplied
to B1. The requested pilot is preparation work, not permission to scrape media.
Ask the user for local permitted sources, allowed training/evaluation uses and
annotation ownership. Keep source bytes local and access-controlled; commit
opaque identities and distributable fixtures only.


## Successor worker evidence and readiness update (2026-09-11)

Four current-worker signal regressions now pass; the audio timestamp-gap defect has
RED-to-GREEN evidence. These are reconstructed current-contract tests, not the
original legacy20-suite execution. Large framewise candidate publication/transfer
remains OPEN: 54,000 actions exceed 2.48 MB versus 512 KiB publication and 1 MiB
inline-read limits. Corpus and human-effort evidence remain absent.

See `research/phase-b/B1-SUCCESSOR-2026-09-11.md` and `research/phase-b/regressions/current-contract-correspondence-2026-09-11-successor.json`.
Earlier hash-bound measurement receipts and historical source assertions are
unchanged. Full Good phase remains unfinished.
