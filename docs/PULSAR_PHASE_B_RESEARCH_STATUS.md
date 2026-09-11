# Pulsar Phase B Research Status

Date: 2026-09-11
Scope: Delegated B3/B5 runtime, local-assistant, and Handy integration research only
Status: Research recorded; implementation and qualification not passed

## Authority and limits

The software specification and accepted architecture retain authority. The detailed research plan is [ADR 0004](adr/0004-runtime-device-research.md). Its implementation choices are proposed technical defaults, not additional user decisions. This document does not report the status of other Phase B work.

No runtime/model assets were downloaded, no inference or tests were run, no private data was uploaded, and no device was actuated during this research. No commit is part of this documentation task.

## Findings and readiness

| Surface | Grounded finding | Remaining closure work |
| --- | --- | --- |
| ORT CPU | Upstream release endpoint resolved to v1.30.0. | Select target builds and source/binary pins, integrate worker, run CPU KAT/differential/malformed-input evidence and offline packaging checks. |
| NVIDIA | CUDA package defaults changed at ORT 1.27; the CUDA and TensorRT compatibility tables do not describe one current universal tuple. | Pin current complete runtime/CUDA/cuDNN/TensorRT/driver tuples; prove actual node placement, cold/warm behavior, cache invalidation and qualified fallback. |
| Windows providers | DirectML remains supported but has specific session restrictions and is in sustained engineering. | Implement those restrictions; qualify model coverage and offline dependency closure. Do not silently substitute WinML. |
| Apple | CoreML availability and model-format requirements are documented. | Qualify concrete native package, OS, compute-unit policy, model shape/partition behavior, and Apple hardware. |
| Intel | Intel EP v5.9 documents ORT 1.24.1/OpenVINO 2025.4.1. | Resolve maintenance and packaging choice; obtain exact dependency pins and execution evidence rather than mixing it with an unrelated ORT build. |
| AMD Linux | ROCmExecutionProvider was removed from ORT 1.23; MIGraphX is the documented replacement. | Select a supported native MIGraphX/ROCm/driver tuple and qualify actual devices. |
| Local assistant | llama.cpp v0.4.0/source 5266f24da75dc449bd56cbed7addb9c8e4a6a73e is a source candidate, not an installed or accepted Pulsar runtime. | Select and pin runtime build, GGUF, tokenizer/template and budgets; keep built-in effects disabled; prove engine-brokered proposal validation and bounded behavior. |
| Handy online | API v3 and FW4 HSP are documented. | Capture and hash the REST contract, provision approved application credentials, implement authorized external effects, qualify live behavior. |
| Handy offline | FW4 BLE service and TX/RX characteristics are documented; Handy 2 is FW4-only. | Pin exact protobuf/SDK and framing semantics; progress from independent codec fixtures and simulators to explicitly authorized device tests. |
| Stopping and headless use | Stop commands and starvation controls are documented, but no physical stopping bound was established. | Exact-configuration physical qualification and explicit human acceptance; separate headless approval; disarmed reconnect. |

Sources, publication-context caveats, and exact version contradictions are preserved in ADR 0004. In particular, the older vendor BLE/security explanation must not override FW4 documentation; the OpenVINO "latest v5.8" label lagged its v5.9 release list; provider compatibility tables must not be extrapolated to ORT v1.30.0.

## Required experiment sequence

1. Capture and pin official runtime/model/protocol source artifacts and record license/redistribution obligations without assuming they are compatible.
2. Establish bounded CPU inference, malformed-input rejection, and independent known-answer/differential results.
3. Measure cold and warm end-to-end processing separately, including required preparation; capture graph placement, dependency identities, peak memory, and raw timing data.
4. Qualify each provider tuple on its declared host configuration; strict pins must fail rather than silently change backend. Failure-triggered fallback must create a distinct attempt.
5. Exercise cache invalidation and resource-budget rejection, then demonstrate clean-machine, offline, relocated-package operation without undeclared installed runtimes.
6. Build a semantic device simulator, then a wire simulator based on pinned schemas and independently sourced fixtures. Self-consistent codecs are not independent conformance evidence.
7. Obtain separate authorization for actual discovery, bounded motion, and failure-stop scenarios. Record physical observations, not only transport acknowledgements.
8. Obtain the required human acceptance and per-configuration headless approval only after qualification evidence exists.

Each experiment's inputs, failure conditions and required evidence are defined in ADR 0004. Runtime/model correctness, real-media quality, performance and physical-device qualification are separate claims.

## Resource constraint

The parent agent reported approximately 22 GB of a 24 GB RTX 4090 in use by another workload. This report does not claim a fresh GPU measurement or identify the owner. Do not stop that workload or assume its allocation is available. No GPU campaign is authorized by this research record.

## Open acceptance definitions

The architecture discussion is complete, but acceptance definitions and qualification evidence remain distinct work. Statistical release rules and corpus adequacy, unselected quality floors, latency percentile/jitter definitions, exact platform/driver matrices, and device firmware stopping bounds must be resolved in the applicable qualification plan. Do not invent numeric thresholds or count missing evidence as success.

## Completion criteria for this research stream

B3/B5 cannot close on documents alone. Required outputs include selected artifact manifests with real byte digests, bounded implementation contracts, raw experiment records, reproducible environment identities, independently checked numerical/protocol evidence, measured product gates, and configuration-bound device qualification where applicable.

Current conclusion: enough primary-source grounding to begin bounded implementation and experiments; insufficient evidence to declare any provider bundle, assistant model, Handy transport, or headless device configuration qualified.
