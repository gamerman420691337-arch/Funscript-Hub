# ADR 0006: Revision ancestry and unresolved review history

Status: Implemented architecture path, not product or physical qualification.

## Decision

The engine records revision lineage in the same SQLite transaction as every
program or protection transition. Each revision records its explicit parent,
operation, optional candidate, contribution envelopes, and a bounded unresolved
review snapshot. Undo and redo also record the exact restored history revision
and history position. History positions have explicit revision links; equal
program bytes are never used to infer origin. Removed redo positions do not
delete immutable revision lineage.

Full candidate replacement adopts the candidate's unresolved reviews. Selective
merge clips incoming reviews to the same writable axis and interval mask used
by the checked merge kernel. Global reviews expand into selected axes. Protected
segments, their required interpolation anchors, and omitted axes cannot acquire
incoming candidate reviews. Contribution envelopes may include unchanged samples;
they are not claims of precise point-level attribution.

Ordinary edits and selective merges retain prior unresolved reviews
conservatively over current program coverage. Project diagnostics label this
retention explicitly. A candidate commit is not human review resolution.
Undo and redo restore the linked history state's review snapshot.

## Bounds and migration

Snapshots are validated against the protocol's 1024-review limit and a 512 KiB
serialized review-state limit. Overflow rejects the entire transaction; reviews
are never silently discarded to complete an edit. The merge contribution mask
also has a 1024-envelope bound. Diagnostic display may page/truncate with an
explicit notice, while the durable bounded snapshot remains intact.

Legacy projects receive an explicitly unknown baseline. Only an already
persisted candidate-to-committed-revision association is reused. The currently
stored history cursor can link to that baseline, but older missing ancestry is
not reconstructed from equal motion. Restoring an unlinked legacy position
retains an explicit unknown-history diagnostic.

## Source catalog

Source kinds are engine assigned. Imported media, materialized generation JSON,
and funscripts are distinct. Existing synthetic sources are backfilled using
persisted job manifests and verified project/source associations, never labels
or filename extensions. Known nonmedia inputs are refused before media worker
admission.

## Remaining scope

There is no explicit human review-resolution command yet. Conservative review
retention can over-report after manual replacement and is labelled accordingly.
A source lineage declaration is not verified authorship or model qualification.
The existing 512 KiB motion-program and 1 MiB control-message limits remain;
long full-detail projects need a separate bounded bulk/editing design.
Portable project archives, physical qualification, and external human acceptance
are not supplied by this decision.
