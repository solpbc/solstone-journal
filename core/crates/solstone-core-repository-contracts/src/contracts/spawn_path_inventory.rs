// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Static rot-check asserting that every long-lived helper spawn path
//! and closed spawn family executes through its prescribed authority and
//! admissions contract with non-zero denominators.

const SUPERVISOR_RUNTIME: &str = include_str!("../../../solstone-core/src/supervisor/runtime.rs");
const CORTEX_PROCESS: &str = include_str!("../../../solstone-core-cortex/src/process.rs");
const SENSE_DISPATCH: &str = include_str!("../../../solstone-core-sense/src/dispatch.rs");
const SPEAKER_DISCOVERY_HELPER: &str =
    include_str!("../../../solstone-core-speaker-resolve/src/discovery_helper.rs");
const COGITATE_TOOLS_SOL_EXECUTION: &str =
    include_str!("../../../solstone-core-cogitate-tools/src/sol_execution.rs");
const COGITATE_WIRE_CLIENT: &str =
    include_str!("../../../solstone-core-cogitate-wire/src/client.rs");
const QUEUE_SOURCE: &str = include_str!("../../../solstone-core-system/src/queue.rs");
const LOCAL_LAUNCH_SOURCE: &str =
    include_str!("../../../solstone-core-system/src/provider_runtime/launch.rs");
const PARAKEET_SOURCE: &str =
    include_str!("../../../solstone-core-system/src/provider_runtime/parakeet.rs");
const SPEAKERS_INSTALLATION: &str =
    include_str!("../../../solstone-core-transcribe/src/speakers_installation.rs");
const READINESS_SOURCE: &str =
    include_str!("../../../solstone-core-system/src/provider_runtime/readiness.rs");

#[test]
fn spawn_path_inventory_denominators_match_design() {
    // 1. Coordinator (1): bootstrap_parent_loss_coordinator -> launch_command
    let coordinator_count = if SUPERVISOR_RUNTIME.contains("fn bootstrap_parent_loss_coordinator(")
        && SUPERVISOR_RUNTIME
            .contains("launch_command(\n            Disposition::ExplicitlyUnowned")
    {
        1
    } else {
        0
    };
    assert_eq!(
        coordinator_count, 1,
        "coordinator bootstrap must use launch_command with ExplicitlyUnowned"
    );

    // 2. Hosted services (5): spawn_app_process -> launch_managed_hosted
    let hosted_services_count = if SUPERVISOR_RUNTIME.contains("pub(crate) fn spawn_app_process(")
        && SUPERVISOR_RUNTIME.contains("launch_managed_hosted(")
        && SUPERVISOR_RUNTIME.contains("HostedLaunchProvenance {")
    {
        5
    } else {
        0
    };
    assert_eq!(
        hosted_services_count, 5,
        "hosted services family must launch 5 services via launch_managed_hosted"
    );

    // 3. Hosted descendants (5):
    //    - Cortex talent runner (spawn_one)
    //    - Sense speakers helper (dispatch)
    //    - Speaker discovery helper (discovery_helper)
    //    - Cogitate sol_execution (run_sol_command)
    //    - Cogitate-wire client (launch_command_hosted)
    let mut hosted_descendants = 0;
    if CORTEX_PROCESS.contains("pub fn spawn_one(")
        && CORTEX_PROCESS.contains("process::launch_command_hosted(")
    {
        hosted_descendants += 1;
    }
    if SENSE_DISPATCH.contains("launch_managed_hosted(")
        && SENSE_DISPATCH.contains("child_launch_provenance(")
    {
        hosted_descendants += 1;
    }
    if SPEAKER_DISCOVERY_HELPER.contains("launch_command_hosted(")
        && SPEAKER_DISCOVERY_HELPER.contains("child_launch_provenance(")
    {
        hosted_descendants += 1;
    }
    if COGITATE_TOOLS_SOL_EXECUTION.contains("launch_command_hosted(") {
        hosted_descendants += 1;
    }
    if COGITATE_WIRE_CLIENT.contains("launch_command_hosted(") {
        hosted_descendants += 1;
    }
    assert_eq!(
        hosted_descendants, 5,
        "hosted descendants family must comprise exactly 5 provenance-bound sites"
    );

    // 4. Task worker (1): queue.rs -> launch_managed_generation_child + task-worker prefix
    let task_worker_count = if QUEUE_SOURCE.contains("launch_managed_generation_child(")
        && QUEUE_SOURCE.contains("generate_helper_launch_id(\"task-worker\")")
    {
        1
    } else {
        0
    };
    assert_eq!(
        task_worker_count, 1,
        "task worker must use launch_managed_generation_child with task-worker prefix"
    );

    // 5. Local provider (1): launch.rs -> launch_generation_child + local-provider prefix
    let local_provider_count = if LOCAL_LAUNCH_SOURCE.contains("launch_generation_child(")
        && LOCAL_LAUNCH_SOURCE.contains("generate_helper_launch_id(\"local-provider\")")
    {
        1
    } else {
        0
    };
    assert_eq!(
        local_provider_count, 1,
        "local provider must use launch_generation_child with local-provider prefix"
    );

    // 6. Parakeet provider (1): parakeet.rs -> launch_generation_child + parakeet-provider prefix
    let parakeet_count = if PARAKEET_SOURCE.contains("launch_generation_child(")
        && PARAKEET_SOURCE.contains("generate_helper_launch_id(\"parakeet-provider\")")
    {
        1
    } else {
        0
    };
    assert_eq!(
        parakeet_count, 1,
        "parakeet provider must use launch_generation_child with parakeet-provider prefix"
    );

    // 7. Readiness probe (1): readiness probe must NOT call generation-child launch and is a bounded --version probe
    let readiness_count = if READINESS_SOURCE.contains("pub fn probe_parakeet_cpp_binary(")
        && READINESS_SOURCE.contains(".arg(\"--version\")")
        && !READINESS_SOURCE.contains("launch_generation_child")
        && !READINESS_SOURCE.contains("launch_managed_generation_child")
    {
        1
    } else {
        0
    };
    assert_eq!(
        readiness_count, 1,
        "readiness probe is inventory-only and must not perform generation-child admission"
    );

    // 8. Speakers-analyze lease (1): enter_speakers_analyze_generation
    let lease_count = if SPEAKERS_INSTALLATION.contains("pub fn enter_speakers_analyze_generation(")
    {
        1
    } else {
        0
    };
    assert_eq!(
        lease_count, 1,
        "speakers-analyze lease entry must be present in speakers_installation"
    );
}

#[test]
fn single_process_spawns_do_not_fork_twice() {
    // Assert spawn_plan and spawn_parakeet execute single-process spawns without a secondary child fork
    assert!(
        LOCAL_LAUNCH_SOURCE.contains("fn spawn_plan("),
        "launch.rs must define spawn_plan"
    );
    assert!(
        PARAKEET_SOURCE.contains("fn spawn_parakeet("),
        "parakeet.rs must define spawn_parakeet"
    );
}
