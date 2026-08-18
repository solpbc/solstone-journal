// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::LoopbackAddr;
use crate::nvidia::{
    ArtifactTrust, Backend, NVIDIA_PROBE_SCHEMA, NvidiaProbe, select_local_backend,
};
use crate::tier::{
    CAPABLE_CONTEXT_TOKENS, CAPABLE_MIN_VRAM_MIB, CAPABLE_PARALLEL_SLOTS, CAPABLE_PROMPT_CACHE_MIB,
    FLOOR_CONTEXT_TOKENS, FLOOR_PARALLEL_SLOTS, FLOOR_PROMPT_CACHE_MIB,
};

const INPUT_SCHEMA: &str = "solstone-local-plan-input-v1";
const LAUNCH_SCHEMA: &str = "solstone-local-launch-plan-v1";
pub const LOCAL_MIN_CONTEXT_TOKENS: u32 = FLOOR_CONTEXT_TOKENS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Linux,
    Darwin,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VulkanDevice {
    pub index: u32,
    pub name: String,
    /// `None` means the producer omitted the Vulkan device type. It is treated as
    /// non-hardware and classifies as `"other"`; the check wave must populate it
    /// from the native enumerator rather than the legacy three-field supervisor payload.
    #[serde(default)]
    pub device_type: Option<u32>,
    pub vram_mib: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanInput {
    pub schema: String,
    pub platform: Platform,
    pub backend_override: Option<PlanBackend>,
    pub bind_address: LoopbackAddr,
    pub port: u16,
    pub desired_fingerprint_json: Value,
    pub desired_fingerprint_sha256: String,
    pub model_id: String,
    pub model_path: String,
    pub mmproj_path: Option<String>,
    pub cuda_binary_path: Option<String>,
    pub vulkan_binary_path: Option<String>,
    pub metal_binary_path: Option<String>,
    pub metal_unified_memory_mib: Option<u64>,
    pub lib_dir: Option<String>,
    pub inherited_ld_library_path: Option<String>,
    pub nvidia_probe: Option<NvidiaProbe>,
    pub cuda_embedded_arch_set: Vec<String>,
    pub cuda_min_driver_version: Option<u32>,
    pub cuda_artifact_trust: Option<ArtifactTrust>,
    pub cuda_persisted_installed_cuda_target: Option<bool>,
    pub vulkan_devices: Option<Vec<VulkanDevice>>,
    pub vulkan_selected_gpu_index: Option<u32>,
    pub vulkan_selected_gpu_name: Option<String>,
    pub vulkan_selected_vram_mib: Option<u64>,
    pub vram_before_mib: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanBackend {
    Cuda,
    Vulkan,
    Metal,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LaunchPlan {
    pub schema: &'static str,
    pub backend: PlanBackend,
    pub desired_fingerprint_json: Value,
    pub desired_fingerprint_sha256: String,
    pub binary_path: Option<String>,
    pub model_path: String,
    pub mmproj_path: Option<String>,
    pub lib_dir: Option<String>,
    pub model_id: String,
    pub gpu_index: Option<u32>,
    pub gpu_name: Option<String>,
    pub gpu_vram_mib: Option<u64>,
    /// Supervisor-supplied pre-launch Vulkan usage; passed through only for post-launch delta logging.
    pub vram_before_mib: Option<u64>,
    pub context_tokens: Option<u32>,
    pub parallel_slots: Option<u32>,
    pub prompt_cache_mib: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metal_tier: Option<MetalTierMetadata>,
    pub visible_devices_env_name: Option<String>,
    pub visible_devices_env_value: Option<String>,
    pub extra_env: BTreeMap<String, String>,
    pub backend_reason: String,
    pub bind_address: LoopbackAddr,
    pub port: u16,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome")]
pub enum PlanOutcome {
    #[serde(rename = "launch")]
    Launch(Box<LaunchPlan>),
    #[serde(rename = "rejected")]
    Rejected { reason: String },
}

#[derive(Clone, Copy)]
pub(crate) struct Tier {
    pub(crate) context_tokens: u32,
    pub(crate) parallel_slots: u32,
    pub(crate) prompt_cache_mib: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MetalTierMetadata {
    pub source: &'static str,
    pub unified_memory_mib: Option<u64>,
}

struct BackendDetails {
    gpu_index: Option<u32>,
    gpu_name: Option<String>,
    gpu_vram_mib: Option<u64>,
    reason: String,
}

fn tier(memory_mib: u64) -> Tier {
    if memory_mib >= CAPABLE_MIN_VRAM_MIB {
        Tier {
            context_tokens: CAPABLE_CONTEXT_TOKENS,
            parallel_slots: CAPABLE_PARALLEL_SLOTS,
            prompt_cache_mib: CAPABLE_PROMPT_CACHE_MIB,
        }
    } else {
        Tier {
            context_tokens: FLOOR_CONTEXT_TOKENS,
            parallel_slots: FLOOR_PARALLEL_SLOTS,
            prompt_cache_mib: FLOOR_PROMPT_CACHE_MIB,
        }
    }
}

pub(crate) fn metal_tier(memory_mib: Option<u64>) -> (Tier, MetalTierMetadata) {
    match memory_mib {
        Some(memory_mib) => (
            tier(memory_mib),
            MetalTierMetadata {
                source: "supplied_measurement",
                unified_memory_mib: Some(memory_mib),
            },
        ),
        None => (
            Tier {
                context_tokens: FLOOR_CONTEXT_TOKENS,
                parallel_slots: FLOOR_PARALLEL_SLOTS,
                prompt_cache_mib: FLOOR_PROMPT_CACHE_MIB,
            },
            MetalTierMetadata {
                source: "no_measurement_floor",
                unified_memory_mib: None,
            },
        ),
    }
}

pub fn plan(input: PlanInput) -> PlanOutcome {
    if input.schema != INPUT_SCHEMA {
        return rejected("unsupported plan input schema");
    }
    match input.platform {
        Platform::Darwin if input.backend_override == Some(PlanBackend::Metal) => plan_metal(input),
        Platform::Darwin => rejected("Metal backend selection is required on Darwin"),
        Platform::Linux => plan_linux(input),
    }
}

fn plan_linux(input: PlanInput) -> PlanOutcome {
    if input
        .nvidia_probe
        .as_ref()
        .is_some_and(|probe| probe.schema != NVIDIA_PROBE_SCHEMA)
    {
        return rejected("unsupported NVIDIA probe schema");
    }
    if let Some(backend) = input.backend_override {
        return match backend {
            PlanBackend::Cuda => {
                let Some(probe) = input.nvidia_probe.as_ref() else {
                    return rejected("CUDA override requires NVIDIA probe");
                };
                let Some(memory) = probe.vram_mib.or(probe.unified_memory_mib) else {
                    return rejected("CUDA override requires usable NVIDIA memory");
                };
                let details = BackendDetails {
                    gpu_index: probe.gpu_index,
                    gpu_name: probe.gpu_name.clone(),
                    gpu_vram_mib: Some(memory),
                    reason: "backend explicitly selected by caller".into(),
                };
                let Some(binary) = input.cuda_binary_path.clone() else {
                    return rejected("cuda binary path is required");
                };
                llama_plan(
                    input,
                    PlanBackend::Cuda,
                    details,
                    binary,
                    tier(memory),
                    Some("CUDA0"),
                    None,
                )
            }
            PlanBackend::Vulkan => plan_vulkan(input, "backend explicitly selected by caller"),
            PlanBackend::Metal => rejected("Metal backend override is not valid on Linux"),
        };
    }
    let cuda_candidate = if let Some(probe) = input.nvidia_probe.as_ref() {
        let arch_set = input
            .cuda_embedded_arch_set
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        if let (Some(min_driver), Some(trust)) =
            (input.cuda_min_driver_version, input.cuda_artifact_trust)
        {
            let choice = select_local_backend(
                probe,
                &arch_set,
                min_driver,
                trust,
                input.cuda_persisted_installed_cuda_target.unwrap_or(false),
            );
            if choice.backend == Backend::Cuda {
                let memory = probe.vram_mib.or(probe.unified_memory_mib);
                if let Some(memory) = memory {
                    Some((
                        probe.gpu_index,
                        probe.gpu_name.clone(),
                        memory,
                        choice.reason,
                    ))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    if let Some((index, name, memory, reason)) = cuda_candidate {
        let Some(binary) = input.cuda_binary_path.clone() else {
            return rejected("cuda binary path is required");
        };
        return llama_plan(
            input,
            PlanBackend::Cuda,
            BackendDetails {
                gpu_index: index,
                gpu_name: name,
                gpu_vram_mib: Some(memory),
                reason,
            },
            binary,
            tier(memory),
            Some("CUDA0"),
            None,
        );
    }
    plan_vulkan(input, "Vulkan selected from Python enumeration")
}

fn plan_vulkan(input: PlanInput, reason: &str) -> PlanOutcome {
    let Some(devices) = input.vulkan_devices.as_ref() else {
        return rejected("no viable CUDA backend and no Vulkan devices");
    };
    let Some(index) = input.vulkan_selected_gpu_index else {
        return rejected("Vulkan selected GPU index is required");
    };
    let Some(device) = devices.iter().find(|device| device.index == index) else {
        return rejected("selected Vulkan GPU is not enumerated");
    };
    if input
        .vulkan_selected_gpu_name
        .as_deref()
        .is_some_and(|name| name != device.name)
        || input
            .vulkan_selected_vram_mib
            .is_some_and(|vram| vram != device.vram_mib)
    {
        return rejected("selected Vulkan GPU does not match enumerated device");
    }
    let device = (device.index, device.name.clone(), device.vram_mib);
    let Some(binary) = input.vulkan_binary_path.clone() else {
        return rejected("vulkan binary path is required");
    };
    llama_plan(
        input,
        PlanBackend::Vulkan,
        BackendDetails {
            gpu_index: Some(device.0),
            gpu_name: Some(device.1),
            gpu_vram_mib: Some(device.2),
            reason: reason.into(),
        },
        binary,
        tier(device.2),
        Some("Vulkan0"),
        None,
    )
}

fn plan_metal(input: PlanInput) -> PlanOutcome {
    let Some(binary) = input.metal_binary_path.clone() else {
        return rejected("metal binary path is required");
    };
    if input.mmproj_path.is_none() {
        return rejected("metal mmproj path is required");
    }
    let (selected_tier, metal_tier) = metal_tier(input.metal_unified_memory_mib);
    let reason = if metal_tier.unified_memory_mib.is_some() {
        "Darwin Metal runtime with supplied unified-memory measurement"
    } else {
        "Darwin Metal runtime; unified-memory measurement was not supplied"
    };
    llama_plan(
        input,
        PlanBackend::Metal,
        BackendDetails {
            gpu_index: None,
            gpu_name: None,
            gpu_vram_mib: None,
            reason: reason.into(),
        },
        binary,
        selected_tier,
        None,
        Some(metal_tier),
    )
}

fn llama_plan(
    input: PlanInput,
    backend: PlanBackend,
    details: BackendDetails,
    binary: String,
    selected_tier: Tier,
    device: Option<&str>,
    metal_tier: Option<MetalTierMetadata>,
) -> PlanOutcome {
    let mut argv = vec![
        binary.clone(),
        "-m".into(),
        input.model_path.clone(),
        "--alias".into(),
        input.model_id.clone(),
        "--host".into(),
        input.bind_address.to_string(),
        "--port".into(),
        input.port.to_string(),
        "--jinja".into(),
        "--n-gpu-layers".into(),
        "999".into(),
        "-c".into(),
        (selected_tier.context_tokens * selected_tier.parallel_slots).to_string(),
        "--parallel".into(),
        selected_tier.parallel_slots.to_string(),
        "--kv-unified".into(),
        "--cache-ram".into(),
        selected_tier.prompt_cache_mib.to_string(),
        "--no-context-shift".into(),
    ];
    if let Some(device) = device {
        argv.extend(["--device".into(), device.into()]);
    }
    if let Some(mmproj) = input.mmproj_path.as_ref() {
        argv.extend(["--mmproj".into(), mmproj.clone()]);
    }
    let inherited_ld_library_path = input.inherited_ld_library_path.clone();
    let mut plan = base_plan(
        input,
        backend,
        details,
        argv,
        Some(selected_tier),
        metal_tier,
    );
    if plan.backend == PlanBackend::Cuda {
        plan.visible_devices_env_name = Some("CUDA_VISIBLE_DEVICES".into());
        plan.visible_devices_env_value = plan.gpu_index.map(|value| value.to_string());
        if let Some(lib_dir) = plan.lib_dir.as_ref() {
            let value = inherited_ld_library_path
                .filter(|value| !value.is_empty())
                .map_or_else(|| lib_dir.clone(), |value| format!("{lib_dir}:{value}"));
            plan.extra_env.insert("LD_LIBRARY_PATH".into(), value);
        }
    } else if plan.backend == PlanBackend::Vulkan {
        plan.visible_devices_env_name = Some("GGML_VK_VISIBLE_DEVICES".into());
        plan.visible_devices_env_value = plan.gpu_index.map(|value| value.to_string());
        if let Some(value) = plan.visible_devices_env_value.clone() {
            plan.extra_env
                .insert("GGML_VK_VISIBLE_DEVICES".into(), value);
        }
        if plan.lib_dir.is_none()
            && let Some(binary) = plan.binary_path.as_deref()
            && let Some(lib_dir) = discover_vulkan_lib_dir(Path::new(binary))
        {
            plan.extra_env
                .insert("LD_LIBRARY_PATH".into(), lib_dir.display().to_string());
        }
    }
    PlanOutcome::Launch(Box::new(plan))
}

fn base_plan(
    input: PlanInput,
    backend: PlanBackend,
    details: BackendDetails,
    argv: Vec<String>,
    selected_tier: Option<Tier>,
    metal_tier: Option<MetalTierMetadata>,
) -> LaunchPlan {
    LaunchPlan {
        schema: LAUNCH_SCHEMA,
        backend,
        desired_fingerprint_json: input.desired_fingerprint_json,
        desired_fingerprint_sha256: input.desired_fingerprint_sha256,
        binary_path: match backend {
            PlanBackend::Cuda => input.cuda_binary_path,
            PlanBackend::Vulkan => input.vulkan_binary_path,
            PlanBackend::Metal => input.metal_binary_path,
        },
        model_path: input.model_path,
        mmproj_path: input.mmproj_path,
        lib_dir: input.lib_dir,
        model_id: input.model_id,
        gpu_index: details.gpu_index,
        gpu_name: details.gpu_name,
        gpu_vram_mib: details.gpu_vram_mib,
        vram_before_mib: input.vram_before_mib,
        context_tokens: selected_tier.map(|tier| tier.context_tokens),
        parallel_slots: selected_tier.map(|tier| tier.parallel_slots),
        prompt_cache_mib: selected_tier.map(|tier| tier.prompt_cache_mib),
        metal_tier,
        visible_devices_env_name: None,
        visible_devices_env_value: None,
        extra_env: BTreeMap::new(),
        backend_reason: details.reason,
        bind_address: input.bind_address,
        port: input.port,
        argv,
    }
}

fn discover_vulkan_lib_dir(binary_path: &Path) -> Option<PathBuf> {
    if !binary_path.is_file() {
        return None;
    }
    let parent = binary_path.parent()?;
    if parent.join("libllama-server-impl.so").is_file() {
        return Some(parent.to_path_buf());
    }
    let mut found = None;
    for entry in fs::read_dir(parent).ok()? {
        let path = entry.ok()?.path();
        if path.is_dir() && path.join("libllama-server-impl.so").is_file() {
            if found.is_some() {
                return None;
            }
            found = Some(path);
        }
    }
    found
}

fn rejected(reason: impl Into<String>) -> PlanOutcome {
    PlanOutcome::Rejected {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvidia::{ArtifactTrust, NvidiaProbe};

    fn probe(memory: Option<u64>, unified: Option<u64>) -> NvidiaProbe {
        NvidiaProbe {
            schema: "solstone-local-nvidia-probe-v1".into(),
            detected: true,
            gpu_index: Some(3),
            gpu_name: Some("GPU".into()),
            compute_cap: Some("8.9".into()),
            arch: Some("sm_89".into()),
            driver_cuda_major: Some(13),
            vram_mib: memory,
            unified_memory_mib: unified,
            probe_error: None,
        }
    }
    fn input(memory: u64) -> PlanInput {
        PlanInput {
            schema: INPUT_SCHEMA.into(),
            platform: Platform::Linux,
            backend_override: None,
            bind_address: LoopbackAddr::IPV4_LOOPBACK,
            port: 4010,
            desired_fingerprint_json: serde_json::json!({"x":1}),
            desired_fingerprint_sha256: "sha".into(),
            model_id: "m".into(),
            model_path: "/model".into(),
            mmproj_path: Some("/mm".into()),
            cuda_binary_path: Some("/cuda-bin".into()),
            vulkan_binary_path: Some("/vulkan-bin".into()),
            metal_binary_path: None,
            metal_unified_memory_mib: None,
            lib_dir: Some("/lib".into()),
            inherited_ld_library_path: Some("/oldlib".into()),
            nvidia_probe: Some(probe(Some(memory), None)),
            cuda_embedded_arch_set: vec!["sm_89".into()],
            cuda_min_driver_version: Some(13),
            cuda_artifact_trust: Some(ArtifactTrust::Trusted),
            cuda_persisted_installed_cuda_target: Some(false),
            vulkan_devices: None,
            vulkan_selected_gpu_index: None,
            vulkan_selected_gpu_name: None,
            vulkan_selected_vram_mib: None,
            vram_before_mib: Some(7),
        }
    }
    fn launch(input: PlanInput) -> LaunchPlan {
        match plan(input) {
            PlanOutcome::Launch(plan) => *plan,
            PlanOutcome::Rejected { reason } => panic!("{reason}"),
        }
    }
    fn assert_rejected(input: PlanInput, expected_reason: &str) {
        match plan(input) {
            PlanOutcome::Rejected { reason } => assert_eq!(reason, expected_reason),
            PlanOutcome::Launch(_) => panic!("expected rejection: {expected_reason}"),
        }
    }
    #[test]
    fn tier_boundaries_and_cuda_argv() {
        for (memory, context, slots) in [
            (15_999, 16_384, 1),
            (16_000, 32_768, 2),
            (16_001, 32_768, 2),
        ] {
            let plan = launch(input(memory));
            assert_eq!(
                (plan.context_tokens, plan.parallel_slots),
                (Some(context), Some(slots))
            );
        }
        let plan = launch(input(16_000));
        assert_eq!(
            plan.argv,
            vec![
                "/cuda-bin",
                "-m",
                "/model",
                "--alias",
                "m",
                "--host",
                "127.0.0.1",
                "--port",
                "4010",
                "--jinja",
                "--n-gpu-layers",
                "999",
                "-c",
                "65536",
                "--parallel",
                "2",
                "--kv-unified",
                "--cache-ram",
                "2048",
                "--no-context-shift",
                "--device",
                "CUDA0",
                "--mmproj",
                "/mm"
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
        );
        assert_eq!(
            plan.extra_env.get("LD_LIBRARY_PATH"),
            Some(&"/lib:/oldlib".into())
        );
    }
    #[test]
    fn gb10_memory_tiers_from_unified_memory() {
        let mut input = input(1);
        input.nvidia_probe = Some(probe(None, Some(16_000)));
        let plan = launch(input);
        assert_eq!(plan.context_tokens, Some(32_768));
    }
    #[test]
    fn persisted_context_is_the_unmultiplied_tier_value() {
        let plan = launch(input(16_000));
        let context = plan.context_tokens.expect("tier context");
        let slots = plan.parallel_slots.expect("tier slots");
        let context_index = plan
            .argv
            .iter()
            .position(|value| value == "-c")
            .expect("context flag");
        let launched = plan.argv[context_index + 1]
            .parse::<u32>()
            .expect("launched context");
        assert_eq!(launched, context * slots);
        assert_ne!(launched, context);
    }
    #[test]
    fn vulkan_argv_is_pinned() {
        let mut input = input(1);
        input.nvidia_probe = None;
        input.vulkan_devices = Some(vec![VulkanDevice {
            index: 2,
            name: "vk".into(),
            device_type: None,
            vram_mib: 16_000,
        }]);
        input.vulkan_selected_gpu_index = Some(2);
        input.vulkan_selected_gpu_name = Some("vk".into());
        input.vulkan_selected_vram_mib = Some(16_000);
        let plan = launch(input);
        assert_eq!(plan.backend, PlanBackend::Vulkan);
        assert_eq!(plan.binary_path.as_deref(), Some("/vulkan-bin"));
        assert!(plan.argv.contains(&"Vulkan0".into()));
        assert!(plan.argv.contains(&"65536".into()));
        assert_eq!(
            plan.visible_devices_env_name.as_deref(),
            Some("GGML_VK_VISIBLE_DEVICES")
        );
        assert_eq!(plan.visible_devices_env_value.as_deref(), Some("2"));
        assert_eq!(
            plan.extra_env.get("GGML_VK_VISIBLE_DEVICES"),
            Some(&"2".into())
        );
    }

    #[test]
    fn vulkan_device_accepts_legacy_and_typed_json() {
        let legacy: VulkanDevice = serde_json::from_value(serde_json::json!({
            "index": 2,
            "name": "vk",
            "vram_mib": 16000
        }))
        .expect("legacy Vulkan device");
        assert_eq!(legacy.device_type, None);
        let typed: VulkanDevice = serde_json::from_value(serde_json::json!({
            "index": 2,
            "name": "vk",
            "device_type": 2,
            "vram_mib": 16000
        }))
        .expect("typed Vulkan device");
        assert_eq!(typed.device_type, Some(2));
    }
    #[test]
    fn cuda_override_skips_backend_decision_inputs() {
        let mut input = input(16_000);
        input.backend_override = Some(PlanBackend::Cuda);
        input.cuda_embedded_arch_set.clear();
        input.cuda_min_driver_version = None;
        input.cuda_artifact_trust = None;
        input.cuda_persisted_installed_cuda_target = None;
        let plan = launch(input);
        assert_eq!(plan.backend, PlanBackend::Cuda);
        assert_eq!(plan.binary_path.as_deref(), Some("/cuda-bin"));
    }
    #[test]
    fn vulkan_override_skips_backend_decision_inputs() {
        let mut input = input(1);
        input.backend_override = Some(PlanBackend::Vulkan);
        input.nvidia_probe = None;
        input.cuda_embedded_arch_set.clear();
        input.cuda_min_driver_version = None;
        input.cuda_artifact_trust = None;
        input.cuda_persisted_installed_cuda_target = None;
        input.vulkan_devices = Some(vec![VulkanDevice {
            index: 2,
            name: "vk".into(),
            device_type: None,
            vram_mib: 16_000,
        }]);
        input.vulkan_selected_gpu_index = Some(2);
        let plan = launch(input);
        assert_eq!(plan.backend, PlanBackend::Vulkan);
        assert_eq!(plan.binary_path.as_deref(), Some("/vulkan-bin"));
    }
    #[test]
    fn cuda_override_rejects_missing_probe_memory_and_binary() {
        let mut missing_probe = input(16_000);
        missing_probe.backend_override = Some(PlanBackend::Cuda);
        missing_probe.nvidia_probe = None;
        assert_rejected(missing_probe, "CUDA override requires NVIDIA probe");

        let mut missing_memory = input(16_000);
        missing_memory.backend_override = Some(PlanBackend::Cuda);
        missing_memory.nvidia_probe = Some(probe(None, None));
        assert_rejected(
            missing_memory,
            "CUDA override requires usable NVIDIA memory",
        );

        let mut missing_binary = input(16_000);
        missing_binary.backend_override = Some(PlanBackend::Cuda);
        missing_binary.cuda_binary_path = None;
        assert_rejected(missing_binary, "cuda binary path is required");
    }
    #[test]
    fn plan_rejects_invalid_or_unknown_nvidia_probe_fields() {
        let mut invalid_schema = input(16_000);
        invalid_schema.backend_override = Some(PlanBackend::Cuda);
        invalid_schema.nvidia_probe.as_mut().expect("probe").schema = "wrong-schema".into();
        assert_rejected(invalid_schema, "unsupported NVIDIA probe schema");

        let mut value = serde_json::to_value(probe(Some(16_000), None)).expect("probe JSON");
        value
            .as_object_mut()
            .expect("probe object")
            .insert("unexpected".into(), serde_json::json!(true));
        assert!(serde_json::from_value::<NvidiaProbe>(value).is_err());
    }
    #[test]
    fn vulkan_override_rejects_invalid_selection_and_missing_binary() {
        let mut no_devices = input(1);
        no_devices.backend_override = Some(PlanBackend::Vulkan);
        no_devices.nvidia_probe = None;
        assert_rejected(no_devices, "no viable CUDA backend and no Vulkan devices");

        let mut no_selected_index = input(1);
        no_selected_index.backend_override = Some(PlanBackend::Vulkan);
        no_selected_index.nvidia_probe = None;
        no_selected_index.vulkan_devices = Some(vec![VulkanDevice {
            index: 2,
            name: "vk".into(),
            device_type: None,
            vram_mib: 16_000,
        }]);
        assert_rejected(no_selected_index, "Vulkan selected GPU index is required");

        let mut unknown_index = input(1);
        unknown_index.backend_override = Some(PlanBackend::Vulkan);
        unknown_index.nvidia_probe = None;
        unknown_index.vulkan_devices = Some(vec![VulkanDevice {
            index: 2,
            name: "vk".into(),
            device_type: None,
            vram_mib: 16_000,
        }]);
        unknown_index.vulkan_selected_gpu_index = Some(3);
        assert_rejected(unknown_index, "selected Vulkan GPU is not enumerated");

        let mut mismatched_name = input(1);
        mismatched_name.backend_override = Some(PlanBackend::Vulkan);
        mismatched_name.nvidia_probe = None;
        mismatched_name.vulkan_devices = Some(vec![VulkanDevice {
            index: 2,
            name: "vk".into(),
            device_type: None,
            vram_mib: 16_000,
        }]);
        mismatched_name.vulkan_selected_gpu_index = Some(2);
        mismatched_name.vulkan_selected_gpu_name = Some("other".into());
        assert_rejected(
            mismatched_name,
            "selected Vulkan GPU does not match enumerated device",
        );

        let mut mismatched_vram = input(1);
        mismatched_vram.backend_override = Some(PlanBackend::Vulkan);
        mismatched_vram.nvidia_probe = None;
        mismatched_vram.vulkan_devices = Some(vec![VulkanDevice {
            index: 2,
            name: "vk".into(),
            device_type: None,
            vram_mib: 16_000,
        }]);
        mismatched_vram.vulkan_selected_gpu_index = Some(2);
        mismatched_vram.vulkan_selected_vram_mib = Some(8_000);
        assert_rejected(
            mismatched_vram,
            "selected Vulkan GPU does not match enumerated device",
        );

        let mut missing_binary = input(1);
        missing_binary.backend_override = Some(PlanBackend::Vulkan);
        missing_binary.nvidia_probe = None;
        missing_binary.vulkan_devices = Some(vec![VulkanDevice {
            index: 2,
            name: "vk".into(),
            device_type: None,
            vram_mib: 16_000,
        }]);
        missing_binary.vulkan_selected_gpu_index = Some(2);
        missing_binary.vulkan_binary_path = None;
        assert_rejected(missing_binary, "vulkan binary path is required");
    }
    #[test]
    fn bind_address_drives_argv_and_plan_input_rejects_non_loopback() {
        let mut ipv6 = input(1);
        ipv6.bind_address = LoopbackAddr::IPV6_LOOPBACK;
        assert_eq!(launch(ipv6).argv[6], "::1");
        let value = serde_json::json!({"schema":INPUT_SCHEMA,"platform":"linux","backend_override":null,"bind_address":"0.0.0.0","port":1,"desired_fingerprint_json":{},"desired_fingerprint_sha256":"x","model_id":"m","model_path":"m","mmproj_path":null,"cuda_binary_path":null,"vulkan_binary_path":null,"metal_binary_path":null,"metal_unified_memory_mib":null,"lib_dir":null,"inherited_ld_library_path":null,"nvidia_probe":null,"cuda_embedded_arch_set":[],"cuda_min_driver_version":null,"cuda_artifact_trust":null,"cuda_persisted_installed_cuda_target":null,"vulkan_devices":null,"vulkan_selected_gpu_index":null,"vulkan_selected_gpu_name":null,"vulkan_selected_vram_mib":null,"vram_before_mib":null});
        assert!(serde_json::from_value::<PlanInput>(value).is_err());
    }
    #[test]
    fn metal_plan_is_darwin_only_omits_device_and_records_tiering() {
        let mut capable = input(1);
        capable.platform = Platform::Darwin;
        capable.backend_override = Some(PlanBackend::Metal);
        capable.model_id = "local/qwen3.5-4b".into();
        capable.metal_binary_path = Some("/metal-bin".into());
        capable.metal_unified_memory_mib = Some(16_000);
        let plan = launch(capable);
        assert_eq!(plan.backend, PlanBackend::Metal);
        assert_eq!(plan.gpu_vram_mib, None);
        assert_eq!(plan.context_tokens, Some(32_768));
        assert_eq!(
            plan.metal_tier,
            Some(MetalTierMetadata {
                source: "supplied_measurement",
                unified_memory_mib: Some(16_000),
            })
        );
        assert!(!plan.argv.iter().any(|arg| arg == "--device"));
        assert!(!plan.argv.iter().any(|arg| arg == "/mlx"));
        assert!(!plan.argv.iter().any(|arg| arg.contains("mlx")));
        assert_eq!(plan.argv.last(), Some(&"/mm".to_owned()));
        assert!(plan.visible_devices_env_name.is_none());
        assert!(plan.extra_env.is_empty());

        let mut floor = input(1);
        floor.platform = Platform::Darwin;
        floor.backend_override = Some(PlanBackend::Metal);
        floor.metal_binary_path = Some("/metal-bin".into());
        floor.metal_unified_memory_mib = Some(15_999);
        assert_eq!(launch(floor).context_tokens, Some(16_384));

        let mut absent = input(1);
        absent.platform = Platform::Darwin;
        absent.backend_override = Some(PlanBackend::Metal);
        absent.metal_binary_path = Some("/metal-bin".into());
        let absent = launch(absent);
        assert_eq!(absent.context_tokens, Some(16_384));
        assert_eq!(
            absent.metal_tier,
            Some(MetalTierMetadata {
                source: "no_measurement_floor",
                unified_memory_mib: None,
            })
        );

        let mut linux = input(1);
        linux.backend_override = Some(PlanBackend::Metal);
        assert_rejected(linux, "Metal backend override is not valid on Linux");

        let mut unspecified = input(1);
        unspecified.platform = Platform::Darwin;
        assert_rejected(unspecified, "Metal backend selection is required on Darwin");
    }
    #[test]
    fn plan_is_repeatable_without_side_effects() {
        let first = launch(input(1));
        let second = launch(input(1));
        assert_eq!(first, second);
    }

    fn vulkan_plan_input(binary: String) -> PlanInput {
        let mut input = input(1);
        input.backend_override = Some(PlanBackend::Vulkan);
        input.nvidia_probe = None;
        input.lib_dir = None;
        input.inherited_ld_library_path = None;
        input.vulkan_binary_path = Some(binary);
        input.vulkan_devices = Some(vec![VulkanDevice {
            index: 0,
            name: "Intel".into(),
            device_type: Some(1),
            vram_mib: 16_000,
        }]);
        input.vulkan_selected_gpu_index = Some(0);
        input.vulkan_selected_gpu_name = Some("Intel".into());
        input.vulkan_selected_vram_mib = Some(16_000);
        input
    }

    fn var_tmp(name: &str) -> std::path::PathBuf {
        let path = std::path::PathBuf::from("/var/tmp")
            .join(format!("solstone-local-plan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("var tmp plan tree");
        path
    }

    #[test]
    fn ac8_vulkan_unflattened_tree_sets_ld_library_path_when_lib_dir_unset() {
        let root = var_tmp("ac8-unflattened");
        let nested = root.join("llama-b10068");
        std::fs::create_dir_all(&nested).expect("nested lib dir");
        let binary = root.join("llama-server");
        std::fs::write(&binary, b"#!/bin/sh\nexit 0\n").expect("vulkan binary");
        std::fs::write(nested.join("libllama-server-impl.so"), b"library").expect("nested lib");
        let plan = launch(vulkan_plan_input(binary.display().to_string()));
        let ld_library_path = plan
            .extra_env
            .get("LD_LIBRARY_PATH")
            .expect("unflattened Vulkan tree must set LD_LIBRARY_PATH");
        assert!(
            ld_library_path.contains("llama-b10068"),
            "LD_LIBRARY_PATH={ld_library_path} must contain the nested llama-b10068 dir"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ac8_vulkan_flattened_sibling_sets_ld_library_path_to_binary_dir() {
        let root = var_tmp("ac8-flattened");
        let binary = root.join("llama-server");
        std::fs::write(&binary, b"#!/bin/sh\nexit 0\n").expect("vulkan binary");
        std::fs::write(root.join("libllama-server-impl.so"), b"library").expect("sibling lib");
        let plan = launch(vulkan_plan_input(binary.display().to_string()));
        let ld_library_path = plan
            .extra_env
            .get("LD_LIBRARY_PATH")
            .expect("flattened sibling tree must set LD_LIBRARY_PATH to the binary directory");
        let binary_dir = root.display().to_string();
        assert!(
            ld_library_path == &binary_dir || ld_library_path.contains(&binary_dir),
            "LD_LIBRARY_PATH={ld_library_path} must contain the binary directory {binary_dir}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ac8_vulkan_missing_binary_path_launches_without_ld_library_path() {
        let missing = std::path::PathBuf::from("/var/tmp/solstone-local-plan-missing-llama-server");
        assert!(!missing.exists());
        let plan = launch(vulkan_plan_input(missing.display().to_string()));
        assert!(
            !plan.extra_env.contains_key("LD_LIBRARY_PATH"),
            "a vulkan_binary_path that is not on disk must still Launch with no LD_LIBRARY_PATH"
        );
    }
}
