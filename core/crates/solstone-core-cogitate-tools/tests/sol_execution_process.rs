// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use solstone_core_cogitate_tools::sol_execution_test_hooks::run_with_timeout;

fn shell(script: &str, extra: &[String]) -> Vec<String> {
    let mut argv = vec!["/bin/sh".to_owned(), "-c".to_owned(), script.to_owned()];
    argv.extend_from_slice(extra);
    argv
}

fn assert_process_exited(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err(),
        "receipt-bearing descendant {pid} survived group cleanup"
    );
}

#[test]
fn timeout_preserves_partial_output_and_cleans_the_process_group() {
    let root = tempfile::Builder::new()
        .prefix("solstone-cogitate-timeout-")
        .tempdir()
        .expect("create process fixture root");
    let receipt = root.path().join("descendant.pid");
    let argv = shell(
        "sleep 5 & child=$!; printf '%s' \"$child\" > \"$1\"; printf partial; printf error >&2; sleep 5",
        &[
            "solstone-cogitate-timeout".to_owned(),
            receipt.display().to_string(),
        ],
    );
    let started = Instant::now();
    let actual =
        run_with_timeout(&argv, root.path(), Duration::from_millis(50)).expect("command handling");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(actual.is_error);
    assert_eq!(
        actual.text,
        "stdout:\npartial\n\nstderr:\nerror\n\ntimeout: command exceeded 30s"
    );
    let descendant = fs::read_to_string(&receipt)
        .expect("read descendant receipt")
        .parse::<i32>()
        .expect("descendant receipt is a PID");
    assert_process_exited(descendant);
}

#[test]
fn exited_root_cleans_a_descendant_that_holds_the_output_pipes() {
    let root = tempfile::Builder::new()
        .prefix("solstone-cogitate-root-exit-")
        .tempdir()
        .expect("create process fixture root");
    let receipt = root.path().join("descendant.pid");
    let argv = shell(
        "sleep 5 & child=$!; printf '%s' \"$child\" > \"$1\"; printf root",
        &[
            "solstone-cogitate-root-exit".to_owned(),
            receipt.display().to_string(),
        ],
    );
    let started = Instant::now();
    let actual = run_with_timeout(&argv, root.path(), Duration::from_secs(2))
        .expect("collect exited root output");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!actual.is_error);
    assert_eq!(actual.text, "stdout:\nroot");

    let descendant = fs::read_to_string(&receipt)
        .expect("read descendant receipt")
        .parse::<i32>()
        .expect("descendant receipt is a PID");
    assert_process_exited(descendant);
}

#[test]
fn real_command_preserves_cwd_environment_output_and_exit_mapping() {
    let root = tempfile::Builder::new()
        .prefix("solstone-cogitate-command-contract-")
        .tempdir()
        .expect("create command fixture root");
    let inherited_home = env::var("HOME").expect("test host provides HOME");
    let argv = shell(
        "printf 'cwd=%s\\npath=%s\\nhome=%s' \"$PWD\" \"${PATH:+set}\" \"$HOME\"; printf error >&2; exit 7",
        &[],
    );
    let actual = run_with_timeout(&argv, root.path(), Duration::from_secs(2))
        .expect("collect command output");
    assert!(actual.is_error);
    assert_eq!(
        actual.text,
        format!(
            "stdout:\ncwd={}\npath=set\nhome={}\n\nstderr:\nerror\n\nexit_code: 7",
            root.path().display(),
            inherited_home
        )
    );
}

#[test]
fn real_command_fully_captures_then_presentation_truncates_each_stream() {
    let argv = shell(
        "i=0; while [ \"$i\" -lt 6001 ]; do printf x; printf y >&2; i=$((i + 1)); done",
        &[],
    );
    let actual = run_with_timeout(&argv, Path::new("."), Duration::from_secs(2))
        .expect("collect bounded large output");
    assert!(!actual.is_error);
    assert_eq!(
        actual.text,
        format!(
            "stdout:\n{}\n... [truncated]\n\nstderr:\n{}\n... [truncated]",
            "x".repeat(6000),
            "y".repeat(6000)
        )
    );
}

fn quote_fixture_path(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\"'\"'"))
}

fn write_journal_fixture(directory: &Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let helper = directory.join("journal");
    fs::write(&helper, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    helper
}

#[test]
fn hosted_journal_tool_fixture() {
    use solstone_core_system::lifecycle::acknowledge_hosted_child_admission;
    let Ok(role) = env::var("SOLSTONE_TEST_JOURNAL_TOOL_ROLE") else {
        return;
    };
    let journal = env::var_os("SOLSTONE_JOURNAL").unwrap();
    if !matches!(role.as_str(), "unacknowledged" | "foreign") {
        acknowledge_hosted_child_admission(Path::new(&journal)).unwrap();
    }
    if role == "helper" {
        println!("journal-result");
        return;
    }
    let helper = env::var("SOLSTONE_TEST_JOURNAL_TOOL_HELPER").unwrap();
    let actual = run_with_timeout(&[helper], Path::new(&journal), Duration::from_secs(2)).unwrap();
    if matches!(role.as_str(), "sealed" | "unacknowledged" | "foreign") {
        assert!(
            actual.is_error,
            "inadmissible parent launched journal: {}",
            actual.text
        );
    } else {
        assert!(
            !actual.is_error && actual.text.contains("journal-result"),
            "{}",
            actual.text
        );
    }
}

#[test]
fn journal_tool_has_distinct_admission_and_cannot_launch_after_seal() {
    use solstone_core_system::lifecycle::{
        AdmissionAcknowledgement, AdmissionIdentity, AdmissionResult, AdmissionResultState,
        HostedServiceKind, ParentLossLedger, acknowledge_parent_loss_admission,
    };
    use solstone_core_system::process::{
        InspectResult, ProcessInstanceSource, SystemProcessInstanceSource,
    };

    for role in ["parent", "sealed", "unacknowledged", "foreign"] {
        let temporary = tempfile::tempdir().unwrap();
        let journal = temporary.path().join("journal-data");
        fs::create_dir(&journal).unwrap();
        let ledger = ParentLossLedger::open(&journal).unwrap();
        let InspectResult::Present { instance, uid, .. } =
            SystemProcessInstanceSource.inspect(std::process::id())
        else {
            panic!("fixture process identity unavailable");
        };
        let active = ledger
            .reserve_generation(instance, [HostedServiceKind::Cortex])
            .unwrap();
        ledger.initialize_record(&active).unwrap();
        ledger
            .persist_coordinator_identity(active.generation, instance)
            .unwrap();
        ledger.mark_admitting(active.generation, instance).unwrap();
        if role == "sealed" {
            ledger.seal(active.generation, instance).unwrap();
        }
        if role == "foreign" {
            acknowledge_parent_loss_admission(
                &journal,
                AdmissionIdentity {
                    generation: active.generation,
                    launch_id: "talent-parent".to_owned(),
                    instance,
                    uid,
                    parent_launch_id: None,
                },
            )
            .unwrap();
        }
        let executable = std::env::current_exe().unwrap();
        let helper = write_journal_fixture(
            temporary.path(),
            &format!(
                "export SOLSTONE_TEST_JOURNAL_TOOL_ROLE=helper\nexec {} --exact hosted_journal_tool_fixture --nocapture",
                quote_fixture_path(&executable)
            ),
        );
        let output = std::process::Command::new(&executable)
            .args(["--exact", "hosted_journal_tool_fixture", "--nocapture"])
            .env("SOLSTONE_TEST_JOURNAL_TOOL_ROLE", role)
            .env("SOLSTONE_TEST_JOURNAL_TOOL_HELPER", helper)
            .env("SOLSTONE_JOURNAL", &journal)
            .env("SOL_PARENT_LOSS_GENERATION", active.generation.to_string())
            .env("SOL_PARENT_LOSS_LAUNCH_ID", "talent-parent")
            .env_remove("SOL_PARENT_LOSS_PARENT_LAUNCH_ID")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let admissions = ledger.generation_path(active.generation).join("admissions");
        if role == "unacknowledged" {
            assert!(!admissions.exists());
            continue;
        }
        let children: Vec<_> = fs::read_dir(&admissions)
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.file_name() != "talent-parent")
            .collect();
        if role != "parent" {
            assert!(children.is_empty());
        } else {
            let parent: AdmissionAcknowledgement = serde_json::from_slice(
                &fs::read(admissions.join("talent-parent/acknowledgement.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(children.len(), 1);
            let child: AdmissionAcknowledgement = serde_json::from_slice(
                &fs::read(children[0].path().join("acknowledgement.json")).unwrap(),
            )
            .unwrap();
            let result: AdmissionResult =
                serde_json::from_slice(&fs::read(children[0].path().join("result.json")).unwrap())
                    .unwrap();
            assert_eq!(
                child.identity.parent_launch_id.as_deref(),
                Some("talent-parent")
            );
            assert_ne!(child.identity.instance, parent.identity.instance);
            assert_ne!(child.identity.launch_id, parent.identity.launch_id);
            assert_eq!(result.identity.as_ref(), Some(&child.identity));
            assert!(matches!(result.state, AdmissionResultState::Admitted));
        }
    }
}
