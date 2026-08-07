// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local-model runtime primitives with explicit process-boundary records.

pub mod bind;
pub mod connect;
pub mod install;
pub mod nvidia;
pub mod plan;
pub(crate) mod tier;

pub use bind::LoopbackAddr;
pub use connect::{ConnectInput, ConnectOutcome, connect};
pub use install::{
    DispatchError as InstallDispatchError, InstallEnvelope, InstallVerb,
    dispatch as dispatch_install,
};
pub use nvidia::{
    ArtifactTrust, Backend, BackendChoice, CUDA_EMBEDDED_ARCH_SET, CUDA_MIN_DRIVER_VERSION,
    NvidiaProbe, hardware_backend_rejection, probe_nvidia_gpu, select_local_backend,
};
pub use plan::{LaunchPlan, PlanInput, PlanOutcome, Platform, VulkanDevice, plan};
