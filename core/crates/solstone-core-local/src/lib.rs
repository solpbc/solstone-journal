// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local-model runtime primitives with explicit process-boundary records.

pub mod admission;
pub mod bind;
pub mod connect;
mod fixture;
pub mod generate;
pub mod install;
pub mod nvidia;
pub mod plan;
pub(crate) mod tier;

pub use bind::LoopbackAddr;
pub use connect::{ConnectInput, ConnectOutcome, connect};
pub use generate::{
    ContextWindow, GenerateError, GenerateFailure, GenerateInput, GenerateResult, GenerateSuccess,
    GenerateTransport, HttpResponse, Inference, InputBudget, PreparedRequest, RequestBudget,
    ServerInference, UreqTransport, Usage, build_messages, build_request_body, generate,
    generate_with, normalize_finish_reason, parse_response, prepare_bundled_request,
    prepare_local_schema,
};
pub use install::{
    DispatchError as InstallDispatchError, InstallEnvelope, InstallVerb,
    dispatch as dispatch_install,
};
pub use nvidia::{
    ArtifactTrust, Backend, BackendChoice, CUDA_EMBEDDED_ARCH_SET, CUDA_MIN_DRIVER_VERSION,
    NvidiaProbe, hardware_backend_rejection, probe_nvidia_gpu, select_local_backend,
};
pub use plan::{LaunchPlan, PlanInput, PlanOutcome, Platform, VulkanDevice, plan};
