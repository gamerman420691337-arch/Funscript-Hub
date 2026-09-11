# B1 measurement groundwork result

Status: **groundwork implemented and checked; real-media pilot and acceptance decisions unresolved**.
This is not Good-phase completion or release qualification.

## Executed evidence

| Check | Result | Receipt |
| --- | --- | --- |
| Analytic KAT, time-translation, malformed annotation/corpus and metric mutation harness | PASS, 299 checks | `results/b1-metric-kat.json` |
| Independent exhaustive assignment oracle and artifact pipeline checks | PASS, 528 checks | `results/b1-independent-oracle.json` |
| Synthetic manifest/reference/candidate pipeline | PASS, one synthetic subject; not real media | `results/b1-synthetic-example.json` |
| Empty held-out corpus | Zero clips; no categories; `release_qualified: false` | `results/b1-empty-heldout.json` |

Total: **827 executable harness checks**. The 20 analytic metric KATs are distinct
from the 20 historical production regression obligations.

The independent assignment oracle checks 512 small synthetic bipartite matching
problems through exhaustive enumeration, separately implemented from the
augmenting-path matcher. Five metric mutants are killed: ignored target
identity, incorrect timing boundary, candidate-relative rather than
reference-relative travel error, candidate reuse and perfect empty denominators.

Artifact checks cover changed reference/candidate hashes, relative and symlink
path escapes, absolute paths, non-file and oversize artifacts, training-set
evaluation rejection, missing human effort and empty-corpus nonqualification.
No runtime, model, GPU, source media or physical device was used by these checks.

## Historical regression status

The recovered six-file inventory contains exactly twenty named historical
assertions. B2 reported current pure-interface correspondence for sixteen cases
under `cargo test -p pulsar-core --test perception --offline`, with 31 tests
passing. That report is retained as such, not reclassified as an original legacy
rerun or an end-to-end worker result. Constant-velocity box prediction is not
the historical dense-flow propagation path. Four signal cases remain without
current correspondence in this B1 result.

Future worker/production receipts may close those statuses in a successor
record. This receipt must not be read as claiming their current absence or
presence beyond the recorded B1 handoff.

## External gates

- Permitted real-media subjects supplied to B1: **0**.
- Independent human reference/section annotations supplied to B1: **0**.
- Recorded human review/edit sessions: **0**.
- Reference-hardware speed campaigns: **0**.
- Approved category taxonomy, segmentation/plateau convention, flag-hit rule and statistical release decision: **not supplied**.
- Creative numeric pass thresholds and rater protocol: **not supplied**.

The manifest/schema, bounded evaluator, durable case map, source-group split
rules, annotation provenance and explicit unknowns satisfy the measurement
groundwork scope. They do not discharge corpus acquisition, training,
neural quality, physical-device or full Good-phase gates.

## Reproduce

```sh
lamu agent -- node scripts/benchmark-good.mjs --self-test
lamu agent -- node research/phase-b/artifact-tests.mjs
lamu agent -- node scripts/benchmark-good.mjs --corpus research/phase-b/fixtures/synthetic-corpus.json --split development
lamu agent -- node scripts/benchmark-good.mjs --corpus research/phase-b/corpus.pending.json --split heldout
```

Optional `--report` arguments on the main evaluator and an optional output
argument on the artifact checks require a new filename. Existing evidence is
never silently overwritten. All repository mutations in this task ran through
`lamu agent`. No commits, original legacy edits, network uploads or device
actuation occurred.


## Successor worker evidence and readiness update (2026-09-11)

Four current-worker signal regressions now pass; the audio timestamp-gap defect has
RED-to-GREEN evidence. These are reconstructed current-contract tests, not the
original legacy20-suite execution. Large framewise candidate publication/transfer
remains OPEN: 54,000 actions exceed 2.48 MB versus 512 KiB publication and 1 MiB
inline-read limits. Corpus and human-effort evidence remain absent.

See `research/phase-b/B1-SUCCESSOR-2026-09-11.md` and `research/phase-b/regressions/current-contract-correspondence-2026-09-11-successor.json`.
Earlier hash-bound measurement receipts and historical source assertions are
unchanged. Full Good phase remains unfinished.
