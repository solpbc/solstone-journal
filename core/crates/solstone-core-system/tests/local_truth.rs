use std::sync::Arc;

use serde_json::json;
use solstone_core_local::Platform;
use solstone_core_system::provider_runtime::{
    LocalRuntimeShared, LocalTruthConfig, LocalTruthSeam, ProviderFence, ProviderName,
    ProviderRuntimeState, ReasonCode, RuntimePhase, TruthObservationSeam,
};

fn fence(attempt: u32) -> ProviderFence {
    ProviderFence {
        incarnation: "test".into(),
        generation: 0,
        fingerprint: None,
        attempt,
    }
}

fn ready_journal() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("solstone-local-truth-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let base = root.join("cache/providers/local/mlx/mlx-community--Qwen3.5-9B-MLX-8bit/84f7c2deea248d8df56240f88102def51c7ed5d6");
    std::fs::create_dir_all(base.join("snapshot")).expect("snapshot");
    let manifest = json!({"schema_version":1,"provider":"local","unit":"mlx-snapshot","target_fingerprint_sha256":"test","created_by_attempt_id":null,"external_root":null,"source":{"pin_identity":{"unit":"mlx-snapshot","model_id":"qwen3.5:9b","repo":"mlx-community/Qwen3.5-9B-MLX-8bit","revision":"84f7c2deea248d8df56240f88102def51c7ed5d6","size_bytes":10453446077u64}},"inventory":[]});
    std::fs::write(base.join("snapshot.manifest.json"), manifest.to_string()).expect("manifest");
    root
}

fn gemma_journal() -> std::path::PathBuf {
    let root =
        std::env::temp_dir().join(format!("solstone-local-truth-gemma-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let base = root.join("cache/providers/local/mlx/mlx-community--gemma-4-26b-a4b-it-4bit/efbeee6e582ebfd06abc9d65e90839c4b5d2116b");
    std::fs::create_dir_all(base.join("snapshot")).expect("snapshot");
    std::fs::create_dir_all(root.join("config")).expect("config");
    std::fs::write(
        root.join("config/journal.json"),
        json!({"providers":{"active":{"model":"gemma-4-26b-a4b-it-mlx-4bit"}}}).to_string(),
    )
    .expect("config");
    let manifest = json!({"schema_version":1,"provider":"local","unit":"mlx-snapshot","target_fingerprint_sha256":"test","created_by_attempt_id":null,"external_root":null,"source":{"pin_identity":{"unit":"mlx-snapshot","model_id":"gemma-4-26b-a4b-it-mlx-4bit","repo":"mlx-community/gemma-4-26b-a4b-it-4bit","revision":"efbeee6e582ebfd06abc9d65e90839c4b5d2116b","size_bytes":15641241224u64}},"inventory":[]});
    std::fs::write(base.join("snapshot.manifest.json"), manifest.to_string()).expect("manifest");
    root
}

fn observe(
    root: &std::path::Path,
    attempt: u32,
) -> solstone_core_system::provider_runtime::ProviderTruthObservation {
    let shared = Arc::new(LocalRuntimeShared::default());
    let mut seam = LocalTruthSeam::with_config(
        shared.clone(),
        LocalTruthConfig {
            journal_path: root.into(),
            platform: Platform::Darwin,
            nvidia_probe: None,
            vulkan_devices: Vec::new(),
        },
    );
    let state = ProviderRuntimeState::new(ProviderName::Local);
    let fence = fence(attempt);
    seam.dispatch_truth(&state, &fence);
    shared.take_truth_result(&fence).unwrap_or_else(|| {
        loop {
            if let Some(value) = shared.take_truth_result(&fence) {
                break value;
            }
            std::thread::yield_now();
        }
    })
}

#[test]
fn ac11_truth_fingerprint_is_stable_and_changes_from_prior_target() {
    let root = ready_journal();
    let first = observe(&root, 1);
    let second = observe(&root, 2);
    assert_eq!(first.phase, RuntimePhase::Starting);
    assert_eq!(
        first.reason_code,
        Some(ReasonCode::known("launch-requested"))
    );
    assert_eq!(first.desired_fingerprint, second.desired_fingerprint);
    let gemma_root = gemma_journal();
    let gemma = observe(&gemma_root, 3);
    assert_eq!(gemma.phase, RuntimePhase::Starting);
    assert_eq!(
        gemma.reason_code,
        Some(ReasonCode::known("launch-requested"))
    );
    assert_ne!(first.desired_fingerprint, gemma.desired_fingerprint);
    std::fs::remove_dir_all(gemma_root).expect("cleanup gemma");
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn ac11_truth_unavailable_on_missing_journal() {
    let root = std::env::temp_dir().join("solstone-local-truth-missing");
    let _ = std::fs::remove_dir_all(&root);
    let result = observe(&root, 3);
    assert_eq!(result.phase, RuntimePhase::StateUnavailable);
    assert_eq!(
        result.reason_code,
        Some(ReasonCode::known("truth-observation-failed"))
    );
    assert!(result.boot_required);
}
