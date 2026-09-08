// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Run this ignored test executable from bin in a separately signed fixture.
//! The six real ONNX package members and both exact test executables must be
//! present before manifest render/sign. This is not production payload closure.

use super::*;
use solstone_core_system::process::{
    BoundedHelperBudget, BoundedHelperRequest, BoundedHelperResources,
    HelperCleanupObservationFault, HelperCleanupStatus,
    run_bounded_helper_with_observation_fault_for_test,
};
use std::sync::Arc;
use std::time::Instant;

#[test]
#[ignore = "native signed-fixture generation/resource receipt"]
fn windows_generation_cleanup_bag_receipt() {
    for name in [
        GENERATION_ENV_KEY,
        GENERATION_TOKEN_ENV_KEY,
        GENERATION_FD_ENV_KEY,
    ] {
        assert!(
            env::var_os(name).is_none(),
            "receipt root must have no inherited generation markers"
        );
    }
    // This runs the real signed helper/model admission before touching generation
    // state. Missing staging/pin/runtime is a prerequisite failure, never a skip.
    installation_proof().expect("real signed Windows ONNX fixture admission");
    let journal = Arc::new(tempfile::tempdir().unwrap());
    let generation = Arc::new(
        enter_speakers_analyze_generation(journal.path(), SpeakersAnalyzeOwnerRole::Convey, None)
            .expect("ordinary free Convey root acquires the existing singleton"),
    );
    assert!(
        enter_speakers_analyze_generation(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Maintenance,
            None,
        )
        .is_err_and(|error| error
            .message()
            .is_some_and(|message| message.starts_with("generation-lease-contended:"))),
        "ordinary unrelated maintenance root must refuse a live holder"
    );

    let weak = Arc::downgrade(&generation);
    let executable = env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join("solstone-system-test-child.exe");
    assert!(
        executable.is_file(),
        "signed fixture must contain the exact system test child"
    );
    let bin = executable.parent().unwrap().to_path_buf();
    let mut resources = BoundedHelperResources::new();
    resources.retain(generation); // sole generation owner: no context clone or child grant
    resources.retain(journal.clone());
    let fault = HelperCleanupObservationFault::new();
    let result = run_bounded_helper_with_observation_fault_for_test(
        BoundedHelperRequest {
            executable,
            current_directory: bin.clone(),
            package_root: bin,
            arguments: vec!["sleep".into(), "30".into()],
            environment: BTreeMap::from([(
                OsString::from("SystemRoot"),
                env::var_os("SystemRoot").unwrap(),
            )]),
            stdin: Vec::new(),
            budget: BoundedHelperBudget {
                timeout: Duration::from_secs(5),
                stdin_limit_bytes: 1,
                stdout_limit_bytes: 1024,
                stderr_limit_bytes: 1024,
            },
            resource_limits: None,
            resources,
        },
        &fault,
    );
    let failure = result.expect_err("actual launched helper must return retained cleanup");
    let cleanup = failure.cleanup().expect("real pending native owner");
    // Gather assertions before releasing, but always perform the bounded recovery
    // before asserting their results, so a failing control does not strand its Job.
    let pending = cleanup.observe() == HelperCleanupStatus::Pending;
    let generation_retained = weak.upgrade().is_some();
    let refusal = enter_speakers_analyze_generation(
        journal.path(),
        SpeakersAnalyzeOwnerRole::Maintenance,
        None,
    );
    let contended = refusal
        .as_ref()
        .err()
        .and_then(CliError::message)
        .is_some_and(|message| message.starts_with("generation-lease-contended:"));
    drop(refusal);
    fault.release();
    let settled = cleanup.retry_until(Instant::now() + Duration::from_secs(5));
    assert!(
        pending && generation_retained && contended,
        "request bag did not exclusively retain generation through actual pending cleanup"
    );
    assert_eq!(
        settled,
        HelperCleanupStatus::Quiescent,
        "same original Job and I/O must settle"
    );
    assert!(
        weak.upgrade().is_none(),
        "completed error must release the last generation owner"
    );
    let reacquired = enter_speakers_analyze_generation(
        journal.path(),
        SpeakersAnalyzeOwnerRole::Maintenance,
        None,
    )
    .expect("ordinary maintenance reacquires after actual cleanup, while Failure still lives");
    assert!(
        failure.cleanup().is_some(),
        "completed Failure remains available during reacquisition"
    );
    drop(reacquired);
    println!("JOURNAL_WIN_CI_GENERATION_BAG=PASS");
}
