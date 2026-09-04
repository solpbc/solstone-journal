// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local-model runtime primitives with explicit process-boundary records.

pub mod admission;
pub mod bind;
pub mod connect;
pub mod converse;
pub mod endpoint;
mod fixture;
pub mod generate;
pub mod install;
pub mod nvidia;
pub mod plan;
pub(crate) mod tier;
pub mod vulkan;

pub use bind::LoopbackAddr;
pub use connect::{ConnectInput, ConnectOutcome, connect};
pub use converse::{
    LocalConverseError, LocalConverseRequest, LocalConverseResponse, LocalConverseToolCall,
    build_converse_request_body, fit_converse_messages, parse_converse_response,
};
pub use endpoint::{
    ByoEndpoint, LocalEndpointResolution, resolve_local_endpoint,
    served_window_from_models_response,
};
pub use fixture::local_generate_input_schema;
pub use generate::{
    ContextWindow, ExactTextCount, GenerateError, GenerateFailure, GenerateInput, GenerateResult,
    GenerateSuccess, GenerateTransport, HttpResponse, Inference, InputBudget, PreparedRequest,
    RequestBudget, ServerInference, UreqTransport, Usage, build_messages, build_request_body,
    count_image_parts, count_input_tokens, estimate_tokens, fit_contents, generate, generate_with,
    inspect_exact_text_admission, normalize_finish_reason, parse_response, prepare_bundled_request,
    prepare_exact_text_request, prepare_local_schema, serialized_message_text,
};
pub use install::{
    DispatchError as InstallDispatchError, InstallEnvelope, InstallVerb,
    dispatch as dispatch_install,
};
pub use nvidia::{
    ArtifactTrust, Backend, BackendChoice, CUDA_EMBEDDED_ARCH_SET, CUDA_MIN_DRIVER_VERSION,
    MemorySource, NvidiaProbe, hardware_backend_rejection, probe_nvidia_gpu, select_local_backend,
};
pub use plan::{LaunchPlan, PlanInput, PlanOutcome, Platform, VulkanDevice, plan};
pub use vulkan::{
    CPU_PLACEMENT_COPY, VulkanProbeConfig, VulkanProbeProgram, classify, cpu_placement_suffix,
    detect_gpus, discrete_hardware_gpu_count, enumerate_gpus, gpu_probe_ok, is_discrete,
    is_hardware_device, select_device,
};
