// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Actual provider contention in separate processes under a disposable owner.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use solstone_core_installation_identity::{
    ArtifactBindingEvidence, GuardFields, InstallationBinding, LegacyManifestEvidence, OwnerBase,
    RootToken, SetupAdmissionRequest, admit_setup, journal_token_from_path,
    load_installation_binding, owner_base, parse_service_guard_environment, root_token_from_path,
    service_guard_environment, windows_lock_contention_count_for_test,
};
use solstone_core_system::process::{
    Disposition, HelperAdmissionStatus, ManagedLaunchRequest, ManagedProcess, OutputStream,
    ProcessEvent, ProcessEventSink, SpawnOptions, launch_managed_request,
    observe_windows_launch_cleanup, retry_windows_launch_cleanup_until,
};

const JOURNAL: &str = "SOLSTONE_TEST_IDENTITY_JOURNAL";
const MODE: &str = "SOLSTONE_TEST_IDENTITY_PROCESS_MODE";
const CHILD: &str = "identity_process::windows_identity_process_child";
const CONTENDED: &str = "JOURNAL_WIN_CI_IDENTITY_LOAD=actual-lock-violation";

fn coordinates() -> (OwnerBase, RootToken, PathBuf) {
    assert!(
        env::var_os("HOME").is_none(),
        "use actual Windows profile, not HOME"
    );
    let journal = PathBuf::from(env::var_os(JOURNAL).expect("explicit disposable fixture journal"));
    assert!(journal.is_absolute());
    let executable = env::current_exe().unwrap();
    let root = executable
        .parent()
        .and_then(solstone_core_journal::resolve_identity_root_from_executable_dir)
        .expect("test executable is inside actual setup-bound fixture");
    (
        owner_base().expect("native owner authority"),
        root_token_from_path(&root).unwrap(),
        journal,
    )
}

fn loaded(owner: &OwnerBase, root: &RootToken, journal: &std::path::Path) -> InstallationBinding {
    let binding = load_installation_binding(owner, root).expect("actual adopted binding");
    assert_eq!(&binding.root_token, root);
    assert_eq!(
        binding.platform,
        solstone_core_installation_identity::PlatformTag::Windows
    );
    assert_eq!(
        binding.journal_token,
        journal_token_from_path(journal).unwrap()
    );
    binding
}

fn line(text: &str) {
    println!("{text}");
    std::io::stdout().flush().unwrap();
}

#[test]
#[ignore = "child of the source-bound native provider interprocess receipt"]
fn windows_identity_process_child() {
    let (owner, root, journal) = coordinates();
    let environment: BTreeMap<String, String> = env::vars().collect();
    let expected = parse_service_guard_environment(&environment)
        .unwrap()
        .expect("all original guards");
    let mode = env::var(MODE).expect("exact child mode");
    assert!(matches!(mode.as_str(), "free" | "held"));
    // No other load runs in this selected child. The observation counts only
    // actual ERROR_LOCK_VIOLATION results in the provider's native retry loop.
    let baseline = windows_lock_contention_count_for_test();
    let binding = if mode == "held" {
        let loader = std::thread::Builder::new()
            .name("identity-native-load".into())
            .spawn(move || loaded(&owner, &root, &journal))
            .expect("start isolated load thread");
        let deadline = Instant::now() + Duration::from_secs(15);
        while windows_lock_contention_count_for_test() == baseline {
            assert!(
                !loader.is_finished(),
                "load finished without actual lock contention"
            );
            assert!(
                Instant::now() < deadline,
                "provider never reached native lock violation"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !loader.is_finished(),
            "parent admission must still exclude this load"
        );
        line(CONTENDED);
        // The enclosing original managed Job/deadline owns this process while
        // the native provider call waits. Joining is not cancellation of that call.
        loader
            .join()
            .expect("load returns after original parent admission release")
    } else {
        let binding = loaded(&owner, &root, &journal);
        assert_eq!(
            windows_lock_contention_count_for_test(),
            baseline,
            "free positive was contended"
        );
        binding
    };
    assert_eq!(GuardFields::from_binding(&binding), expected);
    line(&format!("JOURNAL_WIN_CI_IDENTITY_CHILD={mode}:PASS"));
}

#[derive(Default)]
struct Output {
    lines: std::sync::Mutex<Vec<String>>,
    bytes: AtomicUsize,
    invalid: AtomicBool,
}
impl ProcessEventSink for Output {
    fn emit(&self, event: ProcessEvent) {
        if let ProcessEvent::Line { stream, line, .. } = event {
            let previous = self
                .bytes
                .fetch_add(line.len().saturating_add(1), Ordering::SeqCst);
            if previous > 64 * 1024 || line.len() > 64 * 1024 - previous {
                self.invalid.store(true, Ordering::SeqCst);
                return;
            }
            if stream == OutputStream::Stdout {
                match self.lines.lock() {
                    Ok(mut lines) => lines.push(line),
                    Err(_) => self.invalid.store(true, Ordering::SeqCst),
                }
            }
        }
    }
}
impl Output {
    fn contains(&self, text: &str) -> bool {
        self.lines.lock().unwrap().iter().any(|line| line == text)
    }
}

fn child(
    binding: &InstallationBinding,
    journal: &std::path::Path,
    mode: &str,
    output: Arc<Output>,
    deadline: Instant,
) -> ManagedProcess {
    let mut environment: BTreeMap<OsString, OsString> =
        service_guard_environment(&GuardFields::from_binding(binding))
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
    for key in [
        "SystemRoot",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "TEMP",
        "TMP",
    ] {
        if let Some(value) = env::var_os(key) {
            environment.insert(key.into(), value);
        }
    }
    environment.insert(JOURNAL.into(), journal.as_os_str().to_owned());
    environment.insert(MODE.into(), mode.into());
    launch_managed_request(
        Disposition::IndependentBoundedHelper {
            timeout: deadline.saturating_duration_since(Instant::now()),
        },
        ManagedLaunchRequest {
            command: vec![
                env::current_exe().unwrap().to_str().unwrap().into(),
                "--ignored".into(),
                "--exact".into(),
                CHILD.into(),
                "--nocapture".into(),
                "--test-threads=2".into(),
            ],
            options: SpawnOptions {
                journal_root: journal.to_path_buf(),
                reference: format!("identity-process-{mode}-{}", std::process::id()),
                day: None,
                sink: Some(output),
                environment,
            },
            read_file_grants: Vec::new(),
        },
    )
    .expect("retain actual child launch")
    .into_managed()
    .expect("original native Job owner")
}

fn wait(process: &mut ManagedProcess, output: &Output, marker: Option<&str>, deadline: Instant) {
    loop {
        let status = process.poll().expect("observe original child");
        if let Some(marker) = marker {
            assert!(status.is_none(), "child exited before witnessed contention");
            if output.contains(marker) {
                return;
            }
        } else if let Some(status) = status {
            assert_eq!(status, 0, "actual child libtest exit");
            return;
        }
        assert!(!output.invalid.load(Ordering::SeqCst));
        assert!(Instant::now() < deadline, "native child deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "actual setup-bound fixture under disposable native Windows owner required"]
fn windows_identity_interprocess_receipt() {
    assert!(env::var_os(MODE).is_none());
    assert!(env::var_os("SOL_WINDOWS_LAUNCH").is_none());
    assert!(matches!(
        observe_windows_launch_cleanup(),
        HelperAdmissionStatus::Ready
    ));
    let (owner, root, journal) = coordinates();
    let binding = loaded(&owner, &root, &journal);
    let namespace = owner
        .path()
        .join("namespaces")
        .join(binding.namespace.as_hex());
    let paths = [namespace.join("record"), namespace.join("adoption.marker")];
    let before: Vec<_> = paths.iter().map(|path| fs::read(path).unwrap()).collect();
    for mode in ["free", "held"] {
        let output = Arc::new(Output::default());
        let mut process = None;
        let mut admission = None;
        // Synchronous provider/FS calls are not deadline-cancellable. The host
        // caller retains its own overall process/capture bound separately.
        let deadline = Instant::now() + Duration::from_secs(35);
        let body_deadline = deadline - Duration::from_secs(10);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if mode == "held" {
                admission = Some(
                    admit_setup(SetupAdmissionRequest {
                        owner: owner.clone(),
                        root_token: root.clone(),
                        journal_token: binding.journal_token.clone(),
                        journal_is_explicit: true,
                        legacy_manifest: LegacyManifestEvidence::Absent,
                        artifacts: ArtifactBindingEvidence::Fresh,
                    })
                    .expect("hold actual parent setup admission"),
                );
                assert_eq!(admission.as_ref().unwrap().binding(), &binding);
            }
            process = Some(child(
                &binding,
                &journal,
                mode,
                output.clone(),
                body_deadline,
            ));
            if mode == "held" {
                wait(
                    process.as_mut().unwrap(),
                    &output,
                    Some(CONTENDED),
                    body_deadline,
                );
                assert!(
                    admission.is_some(),
                    "original parent still owns the exclusion"
                );
                assert!(!output.contains("JOURNAL_WIN_CI_IDENTITY_CHILD=held:PASS"));
                drop(admission.take());
            }
            wait(process.as_mut().unwrap(), &output, None, body_deadline);
        }));
        drop(admission.take());
        let mut settled = true;
        if let Some(process) = process.as_mut() {
            if process.poll().ok().flatten().is_none() {
                let _ = process.terminate_exact_until(deadline);
            }
            settled &= process.cleanup_until(deadline);
            process.detach_after_bounded_shutdown();
        }
        drop(process);
        settled &= matches!(
            retry_windows_launch_cleanup_until(deadline),
            HelperAdmissionStatus::Ready
        );
        assert!(settled, "original child Job/I/O cleanup remains incomplete");
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
        assert!(!output.invalid.load(Ordering::SeqCst));
        let lines = output.lines.lock().unwrap();
        for expected in [
            format!("test {CHILD} ... ok"),
            format!("JOURNAL_WIN_CI_IDENTITY_CHILD={mode}:PASS"),
        ] {
            assert_eq!(
                lines.iter().filter(|line| **line == expected).count(),
                1,
                "{lines:?}"
            );
        }
        let summaries: Vec<_> = lines
            .iter()
            .filter(|line| line.starts_with("test result:"))
            .collect();
        assert_eq!(summaries.len(), 1, "{lines:?}");
        assert!(summaries[0].starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;"));
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.as_str() == CONTENDED)
                .count(),
            usize::from(mode == "held")
        );
    }
    assert_eq!(loaded(&owner, &root, &journal), binding);
    for (path, bytes) in paths.iter().zip(before) {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    // Setup-owned fixture namespace and journal remain for explicit operator cleanup.
    println!("JOURNAL_WIN_CI_IDENTITY_INTERPROCESS=PASS");
}
