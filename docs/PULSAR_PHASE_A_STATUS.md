# Pulsar Architecture Phase A1-A6 completion record

Date: 2026-09-11
Status: **Architecture Phase complete; ready for Phase B (Good), starting B1**
Scope: structural integration of the enabled Linux CPU paths and fail-closed
contracts for unsupported capabilities. Not a feature-complete or qualified release.

## Subject and authority

The software specification and ARCH-001-041 remain authoritative. No product
scope or required platform/vendor support was removed. The architecture-first
execution plan defines this completion gate; the migration/capability ledger
records work that is intentionally not enabled yet.

Production source digest:
`4e84e6c97dea136ed8e24dfb61e457604c61f05b490870920f07c953a964b2c2`.
Executable SHA-256:
`3499a17007134ea4584c66bd97f497bf032fd61d578358a4e8e8cfc24dac1cf0`.
These are local artifact identities, not an upstream commit or release receipt.
No git operations, commit, physical actuation, GPU workload or hosted CI run was
performed during this architecture implementation/qualification pass.

## Milestone disposition

| Milestone | Result | Delivered evidence |
| --- | --- | --- |
| A1 Workspace/ownership | PASS | Four libraries and role-only executable; PulsarDesktop; reference-only legacy quarantine; dependency gate and migration inventory. |
| A2 Checked domain/contracts | PASS | Distinct identities, checked time/geometry/motion/evidence, pure transitions and validating deserialization; 20 core tests. |
| A3 Authoritative engine/path | PASS | Secured Linux IPC, separate session secrets, scoped commands, bundled SQLite, durable revision/history/dedupe, real shared-client subprocess path. |
| A4 Media/DSP/viewer integration | PASS | Real decoding and CPU generation, optional native ONNX worker, exact observation association, separate seek resolution, GUI import/candidate/commit/export/preview. |
| A5 Workers/artifacts/resources | PASS for enabled Linux path | Per-attempt confinement and cgroup budgets, immutable snapshots, dependency leases, bounded hashed egress and aggregate storage reservations. Unsupported backends/confinement fail closed. |
| A6 Whole-architecture closure | PASS | All enabled effects have designated ownership; device/assistant interfaces cannot enable unqualified effects; lifecycle model and correspondence entry points; capability ledger and independent repair review. |

## Exit-gate crosswalk

| Required gate | Evidence |
| --- | --- |
| Four-library graph and core restrictions | `scripts/check-architecture.mjs`; `evidence/phase-a/structure.json`; compiler unsafe-code prohibition. |
| No duplicated client/worker project authority | Root composition, client dependency checks, engine-only transactions, reference-only legacy tree and independent worker-mount review. |
| GUI and CLI use real vertical path | `tests/phase_a_integration.rs`, actual CLI/API action equality, `evidence/phase-a/gui-run.md` and GUI export artifact. |
| Checked boundary values | Core/protocol suites; actual rational preview PTS and context matching in subprocess/GUI paths. |
| Auth/revision/revocation/durability | Engine and subprocess tests covering separate credentials, scoped imports/protection, stale candidates, rebase, cancellation, dedupe and hard engine restart. |
| Bounded isolated workers/artifacts | Actual cgroup/bubblewrap worker runs; artifact/tensor/budget tests; independent mount, publication, lease and storage review. |
| Truthful viewer evidence | Frame 0 actual native-model observation; frame 6 no detection and no old box; synthetic freshness tests cover all context dimensions. |
| Device/assistant effects fail closed | Core admission/session transitions, protocol interfaces, engine physical-effect rejection test; no physical invocation. |
| Transitional ownership/removal | `PULSAR_PHASE_A_MIGRATION.md`; no legacy path imports or compiled duplicate orchestration. |
| Defects and missing scope retained | Capability ledger, B1 historical regression obligation and the limitations below. |

## Executed gates

The final local command was:

```sh
CARGO_TARGET_DIR=/tmp/pulsar-phase-a-target CARGO_BUILD_JOBS=2 \
TLA2TOOLS_JAR=/tmp/pulsar-phase-a-tools/tla2tools.jar \
bash scripts/qualify-architecture.sh
```

Result: **exit 0**. Raw output is `evidence/phase-a/architecture-gates.log`.

| Gate | Result |
| --- | --- |
| Workspace/all-target compile check | PASS, locked/offline. |
| Workspace tests | PASS: 86 tests, zero failed, zero ignored. |
| Core | 20 passed. |
| Protocol | 20 passed. |
| Engine, including worker tests | 31 passed; worker tests are included here, not an additional count. |
| Clients | 14 passed. |
| Actual-binary integration | 1 comprehensive test passed, including CLI parity, scoped authority, preview and hard-restart persistence. |
| Formatting | `cargo fmt --all -- --check`, exit 0. |
| Finite lifecycle model | PASS: 888 generated states, 273 distinct states, depth 9. |
| Authorization-negative model mutant | KILLED by `NoUnauthorizedCommit`; unrelated tool failure does not count. |
| Real GUI and native CPU model smoke | PASS in the recorded isolated-display scenario; semantic accuracy unqualified. |
| Independent post-repair review | No residual blocker found within its bounded scope. |

The first model adequacy probe exposed a vacuous unauthorized-commit counter.
The model was corrected to record unauthorized transitions, then the same guard
removal was rejected. This is one adequacy probe, not a full mutation campaign.
Four Kani harness entry points exist; Kani was unavailable and was **not run**.
No bounded Rust proof result is claimed. Remaining critical formal obligations
still apply before Preview.

## Security repairs

Independent initial review found five blockers: bearer-credential disclosure,
ordinary-edit protection bypass, missing protection history, active-model
eviction and aggregate storage/read-query accumulation. Repairs were exercised
by engine/process tests and source-reviewed independently. See
`evidence/phase-a/independent-review.md` for scope and disposition.

This does not establish absence of all vulnerabilities. Host OS and the account
running the engine remain trusted; per-user file permissions are not protection
against unrestricted malicious software already running as that same OS user.
Sandboxed workers are deliberately denied that ambient file access.

## Limits retained for Good and Solid

- This architecture build is narrower than the historical GUI/CLI feature set.
  Unsupported commands/settings remain visible obligations, not completed parity.
- Linux worker execution currently requires installed development tools and
  delegated cgroup v2. CPU worker limits are real; GPU execution is not enabled.
  Windows secured server and macOS native-worker paths remain unavailable.
- No portable offline package, bundled model zoo, local chat model, complete
  modality/six-axis inference, qualified fallback or physical driver is delivered
  by this Phase A record. Required release scope remains unchanged.
- Inline motion programs are capped at 512 KiB; large-program bulk editing,
  portable project archives and complete migration workflows remain B6/C2 work.
- The GUI fixture is a generated FFmpeg pattern. Its native-model detection is
  not a correct semantic target annotation. It proves execution/transport/render
  association only, not neural accuracy or real-media tracking quality.
- Historical 20 M1 regression cases were not rerun as that original inventory.
  New tests do not replace B1 reproduction and B2 defect closure.
- The lifecycle model abstracts successful device admission and finite state
  bounds. It does not prove neural accuracy, foreign runtime correctness,
  physical stopping, scheduling latency or complete implementation refinement.
- Cross-platform hosted CI configuration was updated but not executed remotely.
  A future CI run is not implied by this local evidence.

## Next milestone

**B1: research baseline, defect map and evaluation protocol.** Reproduce the
historical cases against the migrated implementation, classify current failures
and hypotheses, define independent oracles and held-out protocols, then conduct
bounded research with ADR-linked results. Do not start with an unmeasured claim
of quality or superiority. Architecture integration is complete; functionality,
hardening and release qualification remain unfinished.

## Durable records

- `PULSAR_PHASE_A_MIGRATION.md`: ownership, compatibility and capability ledger.
- `adr/0001-phase-a-boundaries.md`: technical defaults, alternatives and limits.
- `evidence/phase-a/structure.json`: source-bound structural gate.
- `evidence/phase-a/architecture-gates.log`: compile/tests/model/mutant output.
- `evidence/phase-a/environment.txt`: compiler, host, binary, fixture/model/runtime/tool identities.
- `evidence/phase-a/gui-run.md`: actual UI scenario and evidence limitations.
- `evidence/phase-a/independent-review.md`: independent repair disposition.
