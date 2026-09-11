# Bounded independent Phase A repair review

Date: 2026-09-11
Reviewer role: separate read-only authority-review agent
Scope: five original findings, frame association, worker mounts/runtime ownership
and process termination. Not an exhaustive security audit or platform qualification.

## Original findings and source-verified repairs

| Finding | Initial consequence | Post-repair disposition |
| --- | --- | --- |
| Public session ID was a credential | Revocation events could disclose cross-project bearer authority. | Closed in reviewed scope: separate secret token hash authenticated before authorization/replay; session IDs public; diagnostic redaction. |
| Edit grant could clear protection | Ordinary editor could remove protected regions before replacement. | Closed: separate ManageProtection scope; protection management checked on direct changes and history transitions. |
| Active model could be evicted through source alias | Model input disappeared while another source's job depended on it. | Closed: active attempt manifests and preview leases cover model/source identities; artifact gate closes preparation/admission races. |
| Protection missing from undo history | Undo/redo restored programs but not protection state. | Closed: history includes both, with fresh revisions and authority rechecks. |
| Aggregate storage/query accumulation | Per-file free-space checks raced; read queries grew durable request table. | Closed in reviewed scope: read queries not persisted; artifact operations serialized against aggregate reservations and available capacity. |

## Additional source checks

- Exact-frame matching is distinct from timestamp-seek resolution.
- Engine resolves immutable paths/hashes; native ONNX initialization/model loading
  happens inside the worker adapter.
- Sandbox mounts system runtime trees plus individually admitted read-only
  artifacts. It does not expose the user's home, project database or credential
  directories. Guest output is a size-limited tmpfs; validated hashed egress is
  published by the engine.
- Per-attempt child cgroups are uniquely created and only their own `cgroup.kill`
  is used. No parent-controller mutation, broad process-group kill or `pkill`.
- Cancellation/revocation still fence late candidate publication.

Final reviewer result: **No residual blocker found in this bounded post-repair
audit.** Reviewer inspected repaired semantics, not only passing test names.
Reviewer did not rerun tests. Root execution evidence is recorded separately.
This does not certify all authorization interleavings, all storage failures,
foreign runtimes, other platforms, model accuracy or physical stopping.
