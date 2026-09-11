# ADR 0004: Runtime and device research boundaries

Date: 2026-09-11
Status: Proposed technical defaults and research plan; not B3/B5 acceptance
Scope: Inference providers, local assistant execution, and Handy/Handy 2 integration

## Authority and disposition

The Pulsar Software Specification remains the product authority. The accepted architecture record establishes offline operation, isolated workers, engine-owned authorization, qualified fallback, device admission, and human-controlled exceptions. This ADR proposes implementation details within those decisions. It does not select an official model, approve a runtime artifact, waive a platform, authorize device movement, or establish performance or physical safety.

This research used public primary sources on 2026-09-11. Documentation and release tags establish candidate compatibility, not compatibility of a Pulsar build. No models or runtime assets were downloaded, no inference was run, and no hardware was actuated. References to "latest" below describe the release endpoint observed on the research date, not a floating dependency policy.

## Proposed implementation direction

1. Keep a mandatory bundled CPU path and versioned, isolated provider-worker bundles behind the same Pulsar worker protocol.
2. Permit different native runtime builds in different worker bundles. Do not combine independently released provider libraries into one process on the assumption that a shared ORT name guarantees ABI or dependency compatibility.
3. Bind each attempt to a complete execution profile: model and preprocessing identities, runtime bytes, provider options, allowed provider order, actual node placement, precision, hardware, and driver.
4. Keep assistant inference separate from effect execution. The model proposes typed actions; the engine validates authorization, arguments, revision, protected regions, and applicable approval.
5. Implement Handy online and offline BLE as separate transports under shared neutral-motion, adaptation, admission, and session contracts.

These are proposed engineering defaults, not newly submitted user decisions or qualified production selections.

## Provider findings and unresolved tuples

| Provider | Primary-source finding as observed | Packaging and qualification consequence |
| --- | --- | --- |
| CPU | The upstream release endpoint resolved to ORT v1.30.0. The release includes numerical, input-validation, and resource-lifetime corrections. [S1] | Candidate for a current CPU worker; actual Rust/C API compatibility, target builds, numerical behavior, and cold-start cost remain untested. |
| CUDA | Official GPU packages changed to CUDA 13.0 beginning with ORT 1.27. The compatibility table explicitly lists ORT 1.29.x/CUDA 13.0/cuDNN 9.x and ORT 1.21-1.26/CUDA 12.8/cuDNN 9.x. It did not establish a complete ORT 1.30 bundle tuple. [S2] | Bundle compatible user-space libraries, record their hashes, and qualify minimum host driver. Do not assume that an older CUDA installation satisfies the selected artifact. |
| TensorRT | The provider table still identifies ORT 1.22/TensorRT 10.9/CUDA 12.0-12.8. Engine/profile caches depend on model, precision, shapes, runtime versions, and hardware. [S3] | This documented older tuple is not a recommendation to ship an old runtime. A current supported tuple needs direct build/release evidence. Cache reuse requires matching dependencies and shape profiles. |
| DirectML | DirectML remains supported but is in sustained engineering; new Windows development moved to WinML. The page lists DirectML 1.15.2, opset 20 with exceptions, and Windows 10 1903 introduction. Sessions require disabled memory patterns, sequential execution, and serialized Run calls per session. [S4] | Implement provider-specific session policy. WinML is a separate offline-packaging investigation, not an automatic replacement. The provider's documented OS minimum is not Pulsar's support-matrix decision. |
| CoreML | CoreML EP requires macOS 10.15+; MLProgram requires macOS 12+. Official ARM64 native packaging includes CoreML. Dynamic shapes can affect performance. [S5] | Pin model format, compute-unit policy, and shape policy. Qualify partitioning and latency on actual Apple hardware. Do not infer Intel Mac support from ARM64 packaging. |
| OpenVINO | Intel EP release v5.9 explicitly combines ORT 1.24.1 and OpenVINO 2025.4.1. The general provider page's "latest v5.8" label lagged the linked release list. [S6, S7] | Preserve the full tuple in a separate worker. A current alternative needs matching build evidence. Library discovery and setup belong in the bundle, not an end-user setupvars or Python-install requirement. |
| AMD Linux | ROCmExecutionProvider was removed starting ORT 1.23; the official direction is MIGraphX. The MIGraphX table documents ORT 1.23.2/ROCm 7.2.1 Ubuntu builds. [S8, S9] | Do not plan a new ROCmExecutionProvider integration. Qualify MIGraphX by exact GPU, driver, target OS, model, and native library tuple. Windows DirectML coverage does not establish AMD Linux coverage. |

ORT performs graph partitioning and can assign unsupported nodes to another configured provider. A GPU provider being available does not prove that the intended graph ran on that GPU. A qualified composite provider profile must identify permitted CPU partitions and retain actual placement evidence. Failure-triggered fallback remains a new attempt under the accepted architecture; strict backend pins must not silently become CPU-only execution. [S10]

The portability contract can bundle application runtimes and models. It cannot remove prerequisites imposed by the OS, GPU driver, Bluetooth stack, or hardware permissions. Exact prerequisites must be explicit in the support matrix and doctor output.

## Artifact identity and supply-chain requirements

No downloaded binary or model digest was established in this research. A release tag, library filename, model name, or provider enumeration is not sufficient artifact identity.

| Artifact family | Known source anchor | Missing before admission |
| --- | --- | --- |
| ORT CPU candidate | microsoft/onnxruntime tag v1.30.0 [S1] | Resolved source commit, reproducible build recipe, target ABI, binary and dependency digests, SBOM, redistribution assessment, qualification receipt. |
| CUDA/TensorRT workers | Compatibility references [S2, S3] | Selected current complete tuple, source and binary pins, driver requirements, build and execution evidence. |
| Intel OpenVINO candidate | intel/onnxruntime tag v5.9; ORT 1.24.1/OpenVINO 2025.4.1 [S6] | Exact native package or build pins, dependency closure, current maintenance assessment, target qualification. |
| AMD and Apple workers | MIGraphX and CoreML contracts [S5, S9] | Selected target-specific builds, dependency identities, model coverage, hardware receipts. |
| llama.cpp candidate | v0.4.0; source commit 5266f24da75dc449bd56cbed7addb9c8e4a6a73e; associated binary release b10809 [S11, S12] | Selected build options, actual binary SHA-256, native dependencies, worker integration, model/template identities and qualification. |
| Handy protocols | API v3 documentation, FW4 BLE characteristics, official SDK repository [S15, S17, S20] | Captured and hashed OpenAPI/protobuf definitions, SDK commit or package integrity, wire framing and notification fixtures, exact firmware builds and device qualification. |

The release pages' observations are source anchors, not accepted Pulsar artifacts. Resolve and preserve source bytes, dependency versions, signatures or available attestations, toolchain, build flags, and SHA-256 digests before qualification. Record the process used to obtain each digest. Do not manufacture a digest for an artifact not acquired.

## Local assistant boundary

The candidate llama.cpp stable release is not evidence that a suitable runtime or assistant model is installed on this host. No official bundled assistant model, GGUF, tokenizer, chat template, context budget, or quality floor was selected or measured by this research.

Current llama.cpp server documentation exposes built-in file/shell tools and MCP child-process execution, warns against enabling these in untrusted environments, and declares its /tools endpoint unsuitable for downstream use. Pulsar must not inherit those effect surfaces. Prefer an isolated inference worker. If a private HTTP adapter is used, require per-instance authentication, loopback binding, explicit origin restrictions, and disabled agent/tools/MCP features. [S13]

Use structured output as a parsing aid, not a permission system. llama.cpp supports tool calling and a subset of JSON Schema grammar constraints; unsupported schema features and semantic errors require independent engine validation. Bound input size, context, output tokens, wall time, action count, and cumulative effects. No model output may enable its own grant, unsandboxed exception, hardware qualification, or dangerous standing approval. [S14, S21]

## Handy and Handy 2 findings

New online integration should target API v3. Its documented account-access token is not the application API token; application credentials require provisioning outside Pulsar's inference loop. HSP on FW4 buffers streamed points, while HSSP is retained through HSP internally. State notifications exist. No authenticated calls were made. [S15, S16]

Handy 2 uses FW4 exclusively. Treat Handy 1, Handy 2 Regular, and Handy 2 Pro as separately identified configurations rather than assuming identical limits from script interoperability. Vendor documentation describes BLE and compatible scripts, not Pulsar qualification. [S18, S19]

FW4 BLE documents service 77834d26-40f7-11ee-be56-0242ac120002, TX characteristic 77835032-40f7-11ee-be56-0242ac120002, and RX characteristic 77835410-40f7-11ee-be56-0242ac120002. It describes full RPC with separate RX/TX, but explicitly offers limited developer support for BLE. Exact protobuf schemas, framing, fragmentation, notification behavior, and firmware-specific semantics were not captured here. Those remain prerequisites to a conforming wire implementation. [S17]

Legacy FW3 BLE is explicitly discouraged for new integrations and exposes unbuffered position/duration behavior. It may remain an explicit compatibility adapter, never a substitute for claiming FW4 HSP behavior. [S22]

FW4.0.13 release notes describe stop-current-mode and HSP pause-on-starvation controls. Neither establishes a physical stopping bound after link loss, process failure, queued movement, or device failure. Buffered playback may continue without its initiating client. Headless playback remains inadmissible without the required exact-configuration qualification and explicit human approval. Reconnect remains disarmed. [S23]

Online script hosting can disclose motion contents. Require an authorized network route and explicit upload permission, minimize outgoing metadata, and keep device/application credentials out of logs. No research authorization implies consent to upload private media/scripts, update firmware, recalibrate hardware, overclock, reset, or move a device. The older security article's BLE discussion predates FW4 and must not override current FW4 protocol documentation. [S24]

## Falsifiable runtime experiments

All rows are planned, not executed.

| Experiment | Inputs and controls | Required evidence and failure condition |
| --- | --- | --- |
| CPU functional baseline | Pinned runtime/model/preprocessing; valid and malformed tensors; explicit worker budgets. | KAT/differential outputs, typed failures, no engine crash, dependency receipt. Wrong shapes, non-finite outputs, unbounded allocation, or unexplained output differences fail. |
| Cold versus warm execution | Fresh process and empty runtime cache versus repeated execution; same media/model/configuration; report extraction, compilation, decode, preprocessing, inference, DSP, and export separately. | Raw timestamps, attempt identity, peak RSS/VRAM, cache identity, wall-clock total. Reject an end-to-end claim that excludes required preparation or uses only warm samples. |
| Provider placement and strict pins | Same model across CPU and each proposed provider; inspect resolved graph partitions; intentionally make a pinned provider unavailable. | Placement report and typed admission failure. A pin that silently becomes CPU-only fails. Approved partial CPU placement must match the profile. |
| Qualified fallback | Induce worker/backend failure using controlled fixtures; retain immutable input identity. | New attempt record, explicit equivalent-profile selection, bounded retries, correct output comparison, no incompatible checkpoint reuse. |
| Resource competition | Admitted offline and interactive loads with bounded synthetic pressure on authorized machines. | Aggregate accounting and measured latency against the specification's selected metrics. Budget oversubscription or live/control starvation fails. No speed claim without agreed percentile/jitter and workload definition. |
| Redistribution and offline installation | Clean supported OS images, network disabled, bundle relocated, absent developer toolchains and Python packages. | Dependency-load inventory, first extraction/restart traces, CPU functional run, provider doctor results, SBOM and redistribution review. Undeclared library downloads or user-installed runtimes fail. |
| Cache invalidation | Change model bytes, runtime/provider versions, precision, shape profile, driver/hardware identity as applicable. | Rebuild or explicit rejection according to cache contract; no silent use of a mismatched compiled engine. |

Reference CPU/RAM/GPU targets and speed floors remain those of the software specification. Provider tables alone establish none of them. Statistical release rules, corpus adequacy, latency percentiles, and unselected quality floors must be resolved in their acceptance definitions before a campaign can close.

## BLE simulator to authorized hardware experiments

| Stage | Scope and prerequisites | Evidence and exit condition |
| --- | --- | --- |
| 1. Pin the wire contract | Obtain official OpenAPI/protobuf and SDK source/package identities without device access. | Exact schema bytes and hashes; supported firmware mapping; known-answer encoding/decoding fixtures. Missing framing or method semantics blocks wire-conformance claims. |
| 2. Semantic simulator | Model prepare/arm/play/pause/stop, bounded buffers, starvation, timestamps, revisions, and session epochs. | Deterministic traces for loss, duplication, delay, reorder, disconnect, stale commands, and restart. This checks Pulsar state transitions, not vendor wire fidelity. |
| 3. Wire simulator | Use the pinned codec and genuine documented or separately authorized captured fixtures. | Fragment/reassembly, malformed-message, buffer-limit, timeout, and notification tests. Do not call a self-consistent encoder/decoder pair an independent oracle. |
| 4. Authorized device discovery | Exact Handy model, firmware, transport and host stack; explicit user permission and no motion. | Identity/capability receipt, pairing behavior, observed state, and credential-redaction evidence. Discovery does not arm playback. |
| 5. Authorized bounded motion | Separate human approval, constrained profile, controlled setup, and independent timing/position observation. | Adaptation, timing, range, pause, reversal and stop measurements for the exact configuration. Simulation or API acknowledgements do not substitute. |
| 6. Failure-stop qualification | Separately approved fault scenarios including controller/process/link loss, driver stall, buffer starvation, and reconnect. | Observed physical stopping behavior and bound, uncertainty, failure disposition, and disarmed reconnect evidence. An unknown or unqualified bound blocks headless use. |
| 7. Human acceptance | Required local evidence reviewed against exact driver/device/firmware/transport/configuration. | Explicit human acceptance and any separate headless approval. A model or assistant cannot issue either. |

## Shared resource constraint

The parent agent reported approximately 22 GB of a 24 GB RTX 4090 already occupied by another workload. This subtask did not independently enumerate the GPU or establish that workload's owner. It is not Pulsar's allocation: do not kill processes, unload models, claim free capacity, or schedule a GPU campaign on that basis. CPU-only research can proceed under its own budget; GPU execution requires explicitly available capacity and a recorded allocation.

## Primary sources

All references accessed 2026-09-11. Mutable documentation is not a byte-pinned qualification input.

- S1: [ONNX Runtime v1.30.0](https://github.com/microsoft/onnxruntime/releases/tag/v1.30.0).
- S2: [ORT CUDA EP](https://onnxruntime.ai/docs/execution-providers/CUDA-ExecutionProvider.html).
- S3: [ORT TensorRT EP](https://onnxruntime.ai/docs/execution-providers/TensorRT-ExecutionProvider.html).
- S4: [ORT DirectML EP](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html).
- S5: [ORT CoreML EP](https://onnxruntime.ai/docs/execution-providers/CoreML-ExecutionProvider.html).
- S6: [Intel ORT EP v5.9](https://github.com/intel/onnxruntime/releases/tag/v5.9).
- S7: [ORT OpenVINO EP](https://onnxruntime.ai/docs/execution-providers/OpenVINO-ExecutionProvider.html).
- S8: [ORT ROCm EP removal](https://onnxruntime.ai/docs/execution-providers/ROCm-ExecutionProvider.html).
- S9: [ORT MIGraphX EP](https://onnxruntime.ai/docs/execution-providers/MIGraphX-ExecutionProvider.html).
- S10: [ORT execution-provider partitioning](https://onnxruntime.ai/docs/execution-providers/).
- S11: [llama.cpp v0.4.0](https://github.com/ggml-org/llama.cpp/releases/tag/v0.4.0).
- S12: [llama.cpp source commit](https://github.com/ggml-org/llama.cpp/commit/5266f24da75dc449bd56cbed7addb9c8e4a6a73e).
- S13: [llama.cpp server documentation](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md).
- S14: [llama.cpp grammar constraints](https://github.com/ggml-org/llama.cpp/blob/master/grammars/README.md).
- S15: [Handy REST API v3](https://ohdoki.notion.site/Handy-Rest-API-v3-ea6c47749f854fbcabcc40c729ea6df4).
- S16: [FW4 features](https://intercom.help/ohdoki/en/articles/9457599-what-s-new-in-firmware-4).
- S17: [FW4 Bluetooth contract](https://ohdoki.notion.site/Bluetooth-control-1ee344f4b55d42e5be14d98b001ee504).
- S18: [Handy 2 firmware support](https://intercom.help/ohdoki/en/articles/12161275-updating-your-handy-2).
- S19: [Ohdoki hardware FAQ](https://intercom.help/ohdoki/en/articles/9524417-faq).
- S20: [Official Handy SDK utilities](https://gitlab.com/sweettechas/platform/handyfeeling-utils).
- S21: [llama.cpp function calling](https://github.com/ggml-org/llama.cpp/blob/master/docs/function-calling.md).
- S22: [Legacy FW3 Bluetooth](https://ohdoki.notion.site/Bluetooth-control-FW3-11d33fbce0474330b80d3b47beef838c).
- S23: [FW4.0.13 release notes](https://intercom.help/ohdoki/en/articles/10844326-update-v4-0-13).
- S24: [Vendor online security and data-flow explanation](https://intercom.help/ohdoki/en/articles/9034256-online-security-and-the-handy).
