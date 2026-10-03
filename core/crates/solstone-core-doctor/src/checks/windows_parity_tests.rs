// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The Windows answers, exercised on every host. Each check's Windows arm is
//! plain logic over the context and the journal tree, so a Linux run proves
//! the decision table; the native Windows gate proves the platform calls.

use std::fs;

use crate::{
    checks::{
        local_bin_solstone_reachable, parakeet_cpp_stt_ready, service_identity, service_running,
        service_status,
    },
    context::{CheckContext, WindowsServiceRegistration},
    registry::{self, Battery},
    vocabulary::{Platform, Severity, Status},
};

use super::test_support::{check, context};

fn windows_context() -> super::test_support::StagedContext {
    let mut staged = context();
    staged.context.platform = Platform::Windows;
    staged
}

fn absent(_: &CheckContext) -> WindowsServiceRegistration {
    WindowsServiceRegistration::Absent
}

fn matching(_: &CheckContext) -> WindowsServiceRegistration {
    WindowsServiceRegistration::Present {
        command: r"C:\Users\owner\AppData\Local\journal\current\bin\journal.exe".into(),
        mismatch: None,
    }
}

fn foreign(_: &CheckContext) -> WindowsServiceRegistration {
    WindowsServiceRegistration::Present {
        command: r"C:\elsewhere\bin\journal.exe".into(),
        mismatch: Some(
            r"C:\elsewhere\bin\journal.exe is registered, expected C:\here\bin\journal.exe".into(),
        ),
    }
}

fn unreadable(_: &CheckContext) -> WindowsServiceRegistration {
    WindowsServiceRegistration::Unreadable("the Task Scheduler did not answer".into())
}

#[test]
fn every_check_runs_on_windows_but_the_two_macos_only_questions() {
    for battery in [Battery::Journal, Battery::JournalReadiness] {
        for entry in registry::entries(battery) {
            let macos_only = matches!(
                entry.check.name,
                "supervisor_conflict" | "launchd_stale_plist"
            );
            assert_eq!(
                entry.check.platforms.contains(&Platform::Windows),
                !macos_only,
                "{} Windows gating",
                entry.check.name
            );
        }
    }
}

/// ⛔ The trap this lane exists for: a healthy Windows resident never creates
/// `health/callosum.sock`, so the socket path cannot be the liveness test.
#[test]
fn windows_liveness_does_not_read_the_unix_socket_path() {
    let staged = windows_context();
    let health = staged.journal_path.join("health");
    fs::create_dir_all(&health).unwrap();
    // A stale socket-shaped file must not make a stopped Windows resident look
    // alive, and its absence is not what establishes that nothing is running.
    fs::write(health.join("callosum.sock"), b"").unwrap();
    assert_eq!(
        service_status::fetch(&staged).unwrap_err(),
        service_status::Unavailable::NoSocket
    );
    fs::write(health.join("supervisor.process_instance"), b"{not json").unwrap();
    assert_eq!(
        service_status::fetch_observe_status(&staged).unwrap_err(),
        service_status::Unavailable::NoSocket
    );
}

#[test]
fn windows_service_running_reads_the_registration_before_the_resident() {
    let check = check("service_running", Severity::Blocker);

    let mut staged = windows_context();
    let row = service_running::run(&staged, check).unwrap();
    assert_eq!(row.status, Status::Skip);
    assert!(
        row.detail
            .starts_with("couldn't read the journal's service registration")
    );

    staged.context.windows_service_probe = Some(absent);
    let row = service_running::run(&staged, check).unwrap();
    assert_eq!(
        (row.status, row.detail.as_str()),
        (Status::Skip, "no local journal service")
    );

    staged.context.windows_service_probe = Some(unreadable);
    let row = service_running::run(&staged, check).unwrap();
    assert_eq!(row.status, Status::Skip);
    assert!(row.detail.ends_with("the Task Scheduler did not answer"));

    // Registered, and no resident has booted: installed but not running.
    staged.context.windows_service_probe = Some(matching);
    let row = service_running::run(&staged, check).unwrap();
    assert_eq!(
        (row.status, row.detail.as_str(), row.fix.as_deref()),
        (
            Status::Warn,
            "service installed but not running",
            Some("run solstone journal service start")
        )
    );
}

#[test]
fn windows_service_identity_is_the_registration_answer() {
    let check = check("service_identity", Severity::Blocker);
    let mut staged = windows_context();

    staged.context.windows_service_probe = Some(absent);
    let row = service_identity::run(&staged, check).unwrap();
    assert_eq!(
        (row.status, row.detail.as_str()),
        (Status::Skip, "no local journal service")
    );

    staged.context.windows_service_probe = Some(matching);
    let row = service_identity::run(&staged, check).unwrap();
    assert_eq!(row.status, Status::Ok);
    assert!(
        row.detail
            .starts_with("service target matches current install: ")
    );

    staged.context.windows_service_probe = Some(foreign);
    let row = service_identity::run(&staged, check).unwrap();
    assert_eq!(row.status, Status::Fail);
    assert!(row.detail.starts_with("service target mismatch: "));
    assert_eq!(
        row.fix.as_deref(),
        Some("run solstone journal setup --force from this install to refresh the service")
    );
}

#[test]
fn windows_solstone_reachability_follows_path_and_pathext_order() {
    let staged = windows_context();
    let check = check("local_bin_solstone_reachable", Severity::Advisory);
    let bin = &staged.install_bin_dir;
    let other = staged.home_dir.join("other-bin");
    fs::create_dir_all(bin).unwrap();
    fs::create_dir_all(&other).unwrap();
    let pathext = std::ffi::OsString::from(".COM;.EXE;.BAT;.CMD");
    let path = |dirs: &[&std::path::Path]| std::env::join_paths(dirs).unwrap();

    let row =
        local_bin_solstone_reachable::windows_for_test(&staged, check, &path(&[bin]), &pathext);
    assert_eq!(row.status, Status::Warn);
    assert!(row.detail.ends_with("solstone.exe is missing"));

    fs::write(bin.join("solstone.exe"), b"").unwrap();
    let row =
        local_bin_solstone_reachable::windows_for_test(&staged, check, &path(&[&other]), &pathext);
    assert_eq!(
        (row.status, row.detail.as_str()),
        (Status::Warn, "solstone is not on PATH")
    );

    let row = local_bin_solstone_reachable::windows_for_test(
        &staged,
        check,
        &path(&[&other, bin]),
        &pathext,
    );
    assert_eq!(row.status, Status::Ok, "{}", row.detail);

    // An earlier directory's `solstone.cmd` is what the shell would run.
    fs::write(other.join("solstone.cmd"), b"").unwrap();
    let row = local_bin_solstone_reachable::windows_for_test(
        &staged,
        check,
        &path(&[&other, bin]),
        &pathext,
    );
    assert_eq!(row.status, Status::Warn);
    assert!(row.detail.starts_with("PATH solstone resolves to "));
}

/// The one live observation: a record naming this very test process is a
/// resident that is running, so the probe goes on to connect.
#[cfg(feature = "full-tests")]
#[test]
fn a_record_of_a_live_process_is_a_listening_resident() {
    let staged = windows_context();
    let health = staged.journal_path.join("health");
    fs::create_dir_all(&health).unwrap();
    // By-pid `inspect` is not ported to Windows and answers `Unverifiable`;
    // the resident records its own identity the same way this does.
    let instance = solstone_core_system::process::current_process_identity()
        .expect("this process can name itself");
    fs::write(
        health.join("supervisor.process_instance"),
        serde_json::to_vec(&instance).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        service_status::resident_process(&staged.journal_path),
        solstone_core_system::process::InstanceVerdict::SameLive { .. }
    ));
    // Live, so the doctor does not stop at "nothing is listening"; with no
    // endpoint to answer here, it reports that it could not get a status.
    assert_ne!(
        service_status::fetch(&staged).unwrap_err(),
        service_status::Unavailable::NoSocket
    );
}

/// ⛔ The Windows parakeet server answers only its owner's children, so the
/// doctor reads the resident's own report of the provider rather than probing.
#[test]
fn windows_parakeet_readiness_is_the_residents_report() {
    let check = check("default_stt_ready", Severity::Advisory);
    let status = |phase: &str| {
        serde_json::json!({"services": [
            {"name": "supervisor", "phase": "running"},
            {"name": "parakeet", "phase": phase},
        ]})
    };
    let row = parakeet_cpp_stt_ready::parakeet_phase_result(check, &status("ready"));
    assert_eq!(row.status, Status::Ok);
    let row = parakeet_cpp_stt_ready::parakeet_phase_result(check, &status("starting"));
    assert_eq!(
        (row.status, row.detail.as_str()),
        (
            Status::Warn,
            "parakeet-server not reachable: your journal reports its state as starting"
        )
    );
    let row = parakeet_cpp_stt_ready::parakeet_phase_result(check, &status("observing"));
    assert!(row.detail.ends_with("its state as checking"));
    let row = parakeet_cpp_stt_ready::parakeet_phase_result(
        check,
        &serde_json::json!({"services": [{"name": "supervisor", "phase": "running"}]}),
    );
    assert_eq!(row.status, Status::Warn);
    assert!(row.fix.is_some());
}

/// With no live resident, the Windows readiness answer says so and names the
/// start command, the same as a stopped Linux service.
#[test]
fn windows_parakeet_readiness_with_no_resident_names_the_start() {
    let staged = windows_context();
    let row = parakeet_cpp_stt_ready::windows_resident_for_test(
        &staged,
        check("default_stt_ready", Severity::Advisory),
    );
    assert_eq!(
        (row.status, row.detail.as_str()),
        (
            Status::Warn,
            "parakeet-server not reachable: your journal isn't running"
        )
    );
    assert!(row.fix.is_some());
}
