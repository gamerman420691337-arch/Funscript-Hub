# Pre-engine source reference

This directory preserves the selected pre-migration Pulsar implementation and its
original Cargo manifest. It is not a workspace member and is not compiled by the
new executable. Existing attribution and license obligations remain in force.
Renaming the desktop application does not establish clean-room provenance.

`main.rs` and `Cargo.toml` preserve the previous composition root. `src/` preserves
its other original modules, including `FunGenApp`, duplicated orchestration and
unqualified device paths. No compatibility client may import this tree with a
path attribute or give it engine authority.

The migration inventory in `docs/PULSAR_PHASE_A_MIGRATION.md` at the repository root
assigns replacement owners and retirement milestones. Keep this reference until
its behavior, defects and lineage have been accounted for. It is not a standalone
supported build or an alternative way to bypass disabled production capabilities.
