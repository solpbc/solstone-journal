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

const ROOT_PROBE_SELECTOR: &str =
    "speakers_installation::windows_generation_tests::windows_generation_unrelated_root_probe";
const ROOT_PROBE_JOURNAL: &str = "SOLSTONE_TEST_GENERATION_ROOT_JOURNAL";
const ROOT_PROBE_MODE: &str = "SOLSTONE_TEST_GENERATION_ROOT_MODE";

#[test]
#[ignore = "child of the native signed-fixture unrelated-root receipt"]
fn windows_generation_unrelated_root_probe() {
    assert!(env::var_os("SOL_WINDOWS_LAUNCH").is_none());
    assert!(env::var_os(GENERATION_FD_ENV_KEY).is_none());
    let journal = PathBuf::from(env::var_os(ROOT_PROBE_JOURNAL).expect("probe journal"));
    let mode = env::var(ROOT_PROBE_MODE).expect("probe mode");
    if mode != "metadata-only" {
        assert!(env::var_os(GENERATION_ENV_KEY).is_none());
        assert!(env::var_os(GENERATION_TOKEN_ENV_KEY).is_none());
    }
    // A missing signed fixture must fail independently of the expected refusal.
    installation_proof().expect("real signed Windows ONNX fixture admission");
    let result =
        enter_speakers_analyze_generation(&journal, SpeakersAnalyzeOwnerRole::Maintenance, None);
    match mode.as_str() {
        "contended" => assert!(result.is_err_and(|error| {
            error
                .message()
                .is_some_and(|message| message.starts_with("generation-lease-contended:"))
        })),
        "metadata-only" => assert!(result.is_err_and(|error| error.message().is_some_and(
            |message| {
                message.contains("generation metadata has no authenticated launch capability")
            }
        ))),
        "free" => drop(result.expect("unrelated ordinary root reacquires the free singleton")),
        _ => panic!("unknown probe mode"),
    }
    println!("\nJOURNAL_WIN_CI_GENERATION_ROOT={mode}:PASS");
}

fn run_unrelated_root_probe(
    journal: &Arc<tempfile::TempDir>,
    generation: Option<&Arc<SpeakersAnalyzeGeneration>>,
    mode: &str,
) {
    let mut resources = BoundedHelperResources::new();
    resources.retain(journal.clone());
    run_unrelated_root_probe_until(
        journal.path(),
        resources,
        generation,
        mode,
        Instant::now() + Duration::from_secs(30),
    );
}

fn run_unrelated_root_probe_until(
    journal: &Path,
    mut resources: BoundedHelperResources,
    generation: Option<&Arc<SpeakersAnalyzeGeneration>>,
    mode: &str,
    deadline: Instant,
) {
    use solstone_core_system::process::run_bounded_helper;
    let executable = env::current_exe().unwrap();
    let bin = executable.parent().unwrap().to_path_buf();
    let mut environment = BTreeMap::from([
        (
            OsString::from("SystemRoot"),
            env::var_os("SystemRoot").unwrap(),
        ),
        (
            OsString::from(ROOT_PROBE_JOURNAL),
            journal.as_os_str().to_owned(),
        ),
        (OsString::from(ROOT_PROBE_MODE), OsString::from(mode)),
        (
            OsString::from("SOLSTONE_JOURNAL_MINISIGN_PIN"),
            env::var_os("SOLSTONE_JOURNAL_MINISIGN_PIN").expect("explicit fixture pin"),
        ),
    ]);
    for name in ["USERPROFILE", "LOCALAPPDATA", "APPDATA", "TEMP", "TMP"] {
        if let Some(value) = env::var_os(name) {
            environment.insert(name.into(), value);
        }
    }
    if let Some(generation) = generation {
        // Parent-side retention only. The helper gets no generation HANDLE or
        // launch protocol; metadata-only deliberately copies diagnostics alone.
        resources.retain(generation.clone());
        if mode == "metadata-only" {
            environment.extend(generation.environment.clone());
        }
    }
    let output = run_bounded_helper(BoundedHelperRequest {
        executable,
        package_root: bin.clone(),
        current_directory: bin,
        arguments: ["--ignored", "--exact", ROOT_PROBE_SELECTOR, "--show-output"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        environment,
        stdin: Vec::new(),
        budget: BoundedHelperBudget {
            timeout: deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(30)),
            stdin_limit_bytes: 1,
            stdout_limit_bytes: 64 * 1024,
            stderr_limit_bytes: 64 * 1024,
        },
        resource_limits: None,
        resources,
    })
    .expect("actual unrelated root must exit with its Job and I/O settled");
    assert_eq!(output.exit_code, 0, "{mode}: {:?}", output.stderr);
    assert!(output.quiescent);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let named = format!("test {ROOT_PROBE_SELECTOR} ... ok");
    let marker = format!("JOURNAL_WIN_CI_GENERATION_ROOT={mode}:PASS");
    assert_eq!(
        stdout.lines().filter(|line| *line == named).count(),
        1,
        "{stdout}"
    );
    assert_eq!(
        stdout.lines().filter(|line| *line == marker).count(),
        1,
        "{stdout}"
    );
    let summaries: Vec<_> = stdout
        .lines()
        .filter(|line| line.starts_with("test result:"))
        .collect();
    assert_eq!(summaries.len(), 1, "{stdout}");
    assert!(
        summaries[0].starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;"),
        "{stdout}"
    );
}

#[test]
#[ignore = "native signed-fixture unrelated-root and metadata refusal receipt"]
fn windows_generation_unrelated_root_receipt() {
    for name in [
        GENERATION_ENV_KEY,
        GENERATION_TOKEN_ENV_KEY,
        GENERATION_FD_ENV_KEY,
        "SOL_WINDOWS_LAUNCH",
    ] {
        assert!(
            env::var_os(name).is_none(),
            "receipt root has inherited markers"
        );
    }
    installation_proof().expect("real signed Windows ONNX fixture admission");
    let journal = Arc::new(tempfile::tempdir().unwrap());
    let generation = Arc::new(
        enter_speakers_analyze_generation(journal.path(), SpeakersAnalyzeOwnerRole::Convey, None)
            .expect("ordinary free Convey root acquires the singleton"),
    );
    run_unrelated_root_probe(&journal, Some(&generation), "contended");
    run_unrelated_root_probe(&journal, Some(&generation), "metadata-only");
    drop(generation);
    run_unrelated_root_probe(&journal, None, "free");
    println!("JOURNAL_WIN_CI_GENERATION_UNRELATED_ROOT=PASS");
}

const DESCENDANT_SELECTOR: &str =
    "speakers_installation::windows_generation_tests::windows_generation_descendant_probe";
const DESCENDANT_INDEX: &str = "SOLSTONE_TEST_GENERATION_DESCENDANT_INDEX";

#[test]
#[ignore = "child of the native signed-fixture descendant receipt"]
fn windows_generation_descendant_probe() {
    let index = env::var(DESCENDANT_INDEX).expect("descendant index");
    assert!(matches!(index.as_str(), "0" | "1"));
    let journal = PathBuf::from(env::var_os(ROOT_PROBE_JOURNAL).expect("descendant journal"));
    let admitted = solstone_core_system::process::receive_windows_launch()
        .expect("real launch transaction")
        .expect("descendant requires authenticated admission");
    let role = if index == "0" {
        SpeakersAnalyzeOwnerRole::Convey
    } else {
        SpeakersAnalyzeOwnerRole::Maintenance
    };
    let generation = enter_speakers_analyze_generation(&journal, role, Some(&admitted))
        .expect("actual signed-package and generation borrowing before app-ready");
    {
        use std::io::Write;
        let mut file = admitted.read_file_grants()[0].file();
        let error = file
            .write_all(b"forbidden write")
            .expect_err("transferred generation grant must be read-only");
        assert_eq!(error.raw_os_error(), Some(5), "expected access denied");
    }
    let wrong = enter_speakers_analyze_generation(
        &journal.join("unrelated-journal"),
        role,
        Some(&admitted),
    );
    assert!(
        wrong.is_err_and(|error| error.message().is_some_and(|message| {
            message.contains("generation launch provenance does not match this root")
        }))
    );
    fs::write(
        journal.join(format!("descendant-{index}.ready")),
        b"borrowed\n",
    )
    .unwrap();
    let release = journal.join(format!("descendant-{index}.release"));
    let deadline = Instant::now() + Duration::from_secs(150);
    loop {
        if release.is_file() {
            break;
        }
        assert!(Instant::now() < deadline, "descendant release deadline");
        assert!(
            !admitted.stop_requested().expect("actual latched stop"),
            "unexpected test stop"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(generation);
    drop(admitted);
    println!("\nJOURNAL_WIN_CI_GENERATION_DESCENDANT={index}:PASS");
}

#[derive(Default)]
struct DescendantOutput {
    lines: std::sync::Mutex<Vec<String>>,
    bytes: std::sync::atomic::AtomicUsize,
    invalid: std::sync::atomic::AtomicBool,
}

impl solstone_core_system::process::ProcessEventSink for DescendantOutput {
    fn emit(&self, event: solstone_core_system::process::ProcessEvent) {
        use solstone_core_system::process::{OutputStream, ProcessEvent};
        use std::sync::atomic::Ordering;
        if let ProcessEvent::Line {
            stream: OutputStream::Stdout,
            line,
            ..
        } = event
        {
            let previous = self
                .bytes
                .fetch_add(line.len().saturating_add(1), Ordering::SeqCst);
            if previous > 64 * 1024 || line.len() > 64 * 1024 - previous {
                self.invalid.store(true, Ordering::SeqCst);
                return;
            }
            match self.lines.lock() {
                Ok(mut lines) => lines.push(line),
                Err(_) => self.invalid.store(true, Ordering::SeqCst),
            }
        }
    }
}

fn launch_generation_descendant(
    journal: &Path,
    generation: &SpeakersAnalyzeGeneration,
    index: usize,
    deadline: Instant,
    output: Arc<DescendantOutput>,
) -> solstone_core_system::process::ManagedProcess {
    use solstone_core_system::process::{
        Disposition, HostedLaunchProvenance, ManagedLaunchRequest, SpawnOptions,
        launch_managed_hosted,
    };
    let context = generation.child_launch_context();
    let mut environment = context.environment;
    for name in [
        "SystemRoot",
        "SOLSTONE_JOURNAL_MINISIGN_PIN",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "TEMP",
        "TMP",
    ] {
        if let Some(value) = env::var_os(name) {
            environment.insert(name.into(), value);
        }
    }
    environment.insert(ROOT_PROBE_JOURNAL.into(), journal.as_os_str().to_owned());
    environment.insert(DESCENDANT_INDEX.into(), index.to_string().into());
    let executable = env::current_exe().unwrap();
    let command = vec![
        executable
            .to_str()
            .expect("signed fixture executable text")
            .to_owned(),
        "--ignored".into(),
        "--exact".into(),
        DESCENDANT_SELECTOR.into(),
        "--show-output".into(),
    ];
    // All request/control/context temporaries are consumed and leave this frame
    // before the parent drops its remaining grant copies in the receipt below.
    launch_managed_hosted(
        Disposition::IndependentLongLived,
        ManagedLaunchRequest {
            command,
            options: SpawnOptions {
                journal_root: journal.to_owned(),
                reference: format!("generation-descendant-{index}"),
                day: None,
                sink: Some(output),
                environment,
            },
            read_file_grants: context.read_file_grants,
        },
        HostedLaunchProvenance {
            journal: journal.to_owned(),
            generation: 1,
            launch_id: format!("generation-descendant-{index}-{}", random_hex().unwrap()),
            service: None,
            parent_launch_id: None,
            acknowledgement_timeout: deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(3)),
        },
    )
    .expect("actual native descendant launch and ACK")
    .into_managed()
    .expect("retained original managed Job")
}

fn wait_descendant(
    process: &mut solstone_core_system::process::ManagedProcess,
    deadline: Instant,
    ready: Option<&Path>,
) {
    loop {
        let exit = process.poll().expect("original native root observation");
        if let Some(path) = ready {
            assert!(exit.is_none(), "descendant exited before actual borrowing");
            if fs::read(path).is_ok_and(|bytes| bytes == b"borrowed\n") {
                return;
            }
        } else if let Some(exit) = exit {
            assert_eq!(exit, 0, "descendant libtest process failed");
            return;
        }
        assert!(Instant::now() < deadline, "descendant wait deadline");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "native signed-fixture authenticated siblings and last-holder receipt"]
fn windows_generation_descendant_receipt() {
    use solstone_core_system::process::{
        HelperAdmissionStatus, observe_bounded_helper_admission, observe_windows_launch_cleanup,
        retry_bounded_helper_admission_until, retry_windows_launch_cleanup_until,
    };
    for name in [
        GENERATION_ENV_KEY,
        GENERATION_TOKEN_ENV_KEY,
        GENERATION_FD_ENV_KEY,
        "SOL_WINDOWS_LAUNCH",
    ] {
        assert!(
            env::var_os(name).is_none(),
            "receipt root has inherited markers"
        );
    }
    assert!(matches!(
        observe_windows_launch_cleanup(),
        HelperAdmissionStatus::Ready
    ));
    assert!(matches!(
        observe_bounded_helper_admission(),
        HelperAdmissionStatus::Ready
    ));
    installation_proof().expect("actual signed Windows fixture admission");
    // Deliberately keep the path until explicit native cleanup succeeds. A
    // failed cleanup retains this exact artifact instead of TempDir::drop
    // removing inputs beneath a still-owned process. It is reported below.
    let journal = tempfile::Builder::new()
        .prefix("generation-descendants-")
        .tempdir()
        .unwrap()
        .keep();
    let deadline = Instant::now() + Duration::from_secs(180);
    let body_deadline = deadline - Duration::from_secs(15);
    let mut children = Vec::new();
    let outputs: Vec<_> = (0..2)
        .map(|_| Arc::new(DescendantOutput::default()))
        .collect();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let generation =
            enter_speakers_analyze_generation(&journal, SpeakersAnalyzeOwnerRole::Supervisor, None)
                .expect("one actual supervisor-owned generation");
        for (index, output) in outputs.iter().enumerate() {
            children.push(launch_generation_descendant(
                &journal,
                &generation,
                index,
                body_deadline,
                output.clone(),
            ));
            wait_descendant(
                &mut children[index],
                body_deadline,
                Some(&journal.join(format!("descendant-{index}.ready"))),
            );
        }
        for child in &mut children {
            child
                .release_parent_read_file_grants_for_test()
                .expect("remove admitted parent grant copies only");
        }
        drop(generation);
        run_unrelated_root_probe_until(
            &journal,
            BoundedHelperResources::new(),
            None,
            "contended",
            body_deadline,
        );
        fs::write(journal.join("descendant-0.release"), b"release\n").unwrap();
        wait_descendant(&mut children[0], body_deadline, None);
        assert!(children[1].poll().unwrap().is_none());
        run_unrelated_root_probe_until(
            &journal,
            BoundedHelperResources::new(),
            None,
            "contended",
            body_deadline,
        );
        fs::write(journal.join("descendant-1.release"), b"release\n").unwrap();
        wait_descendant(&mut children[1], body_deadline, None);
        // Both original ManagedProcess values, including their resource bags,
        // still live here: cleanup cannot hide a leftover parent grant.
        run_unrelated_root_probe_until(
            &journal,
            BoundedHelperResources::new(),
            None,
            "free",
            body_deadline,
        );
    }));
    let mut settled = true;
    for child in &mut children {
        if child.poll().ok().flatten().is_none() {
            let _ = child.terminate_exact_until(deadline);
        }
        settled &= child.cleanup_until(deadline);
        child.detach_after_bounded_shutdown();
    }
    // A launch may fail after native creation, before returning a child value.
    // Its actual original owner is in the existing process-local registry.
    drop(children);
    settled &= matches!(
        retry_windows_launch_cleanup_until(deadline),
        HelperAdmissionStatus::Ready
    );
    settled &= matches!(
        retry_bounded_helper_admission_until(deadline),
        HelperAdmissionStatus::Ready
    );
    if settled {
        fs::remove_dir_all(&journal).expect("remove only the settled receipt journal");
    } else {
        eprintln!(
            "generation descendant cleanup incomplete; retained fixture: {}",
            journal.display()
        );
    }
    assert!(
        settled,
        "original native Jobs and owned I/O must settle before release"
    );
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    for (index, output) in outputs.iter().enumerate() {
        assert!(
            !output.invalid.load(std::sync::atomic::Ordering::SeqCst),
            "bounded output collector failed"
        );
        let lines = output.lines.lock().unwrap();
        let named = format!("test {DESCENDANT_SELECTOR} ... ok");
        let marker = format!("JOURNAL_WIN_CI_GENERATION_DESCENDANT={index}:PASS");
        assert_eq!(
            lines.iter().filter(|line| **line == named).count(),
            1,
            "{lines:?}"
        );
        assert_eq!(
            lines.iter().filter(|line| **line == marker).count(),
            1,
            "{lines:?}"
        );
        let summaries: Vec<_> = lines
            .iter()
            .filter(|line| line.starts_with("test result:"))
            .collect();
        assert_eq!(summaries.len(), 1, "{lines:?}");
        assert!(
            summaries[0].starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;"),
            "{lines:?}"
        );
    }
    println!("JOURNAL_WIN_CI_GENERATION_DESCENDANTS=PASS");
}
