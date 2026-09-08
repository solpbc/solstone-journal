// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_artifact_download::{ByteDownload, ByteDownloadError};
use solstone_core_backup_runtime::{ensure_rclone, ensure_restic};
use solstone_core_system::process::{
    InstanceVerdict, ProcessInstance, ProcessInstanceSource, SystemProcessInstanceSource,
};

pub(super) fn installed_tools(restic: &Path, rclone: &Path, workspace: &Path) {
    struct NoDownload;
    impl ByteDownload for NoDownload {
        fn fetch(&self, _: &str, _: Duration) -> Result<Vec<u8>, ByteDownloadError> {
            panic!("installed Windows tool attempted a download");
        }
    }
    let unrelated = workspace.join("unrelated tool cache");
    for force in [false, true] {
        let found = ensure_restic(&SystemToolRunner, force, Some(&unrelated), &NoDownload).unwrap();
        assert_eq!(
            fs::canonicalize(found).unwrap(),
            fs::canonicalize(restic).unwrap()
        );
        let found = ensure_rclone(&SystemToolRunner, force, Some(&unrelated), &NoDownload).unwrap();
        assert_eq!(
            fs::canonicalize(found).unwrap(),
            fs::canonicalize(rclone).unwrap()
        );
        assert!(!unrelated.exists());
    }
    println!("NATIVE_BACKUP_PACKAGE_TOOLS_NO_DOWNLOAD_OK");
}

pub(super) fn poisoned_payload(restic: &Path, rclone: &Path, repository: &str) {
    use std::io::Write;
    let original_size = fs::metadata(rclone).unwrap().len();
    {
        let mut file = fs::OpenOptions::new().append(true).open(rclone).unwrap();
        file.write_all(b"fixture mutation").unwrap();
    }
    let result = run_restic(
        &SystemToolRunner,
        &["snapshots".into()],
        repository,
        "synthetic-poisoned-payload-secret",
        restic,
        None,
        true,
        None,
        Some(Duration::from_secs(2)),
        &[],
    );
    assert!(
        result.is_err(),
        "mutated admitted payload must fail before tool execution: {result:?}"
    );
    assert!(!format!("{result:?}").contains("synthetic-poisoned-payload-secret"));
    fs::OpenOptions::new()
        .write(true)
        .open(rclone)
        .unwrap()
        .set_len(original_size)
        .unwrap();
    let displaced = restic.with_extension("missing");
    fs::rename(restic, &displaced).unwrap();
    let result = run_restic(
        &SystemToolRunner,
        &["snapshots".into()],
        repository,
        "synthetic-missing-payload-secret",
        restic,
        None,
        true,
        None,
        Some(Duration::from_secs(2)),
        &[],
    );
    fs::rename(displaced, restic).unwrap();
    assert!(
        result.is_err(),
        "missing tool must fail before execution: {result:?}"
    );
    assert!(!format!("{result:?}").contains("synthetic-missing-payload-secret"));
    println!("NATIVE_BACKUP_POISONED_MISSING_PAYLOAD_REFUSED_OK");
}

pub(super) fn refused_restore_roots(
    workspace: &Path,
    services: &BackupServices<'_>,
    destination: &Destination,
    recovery: &str,
) {
    struct NoLaunch;
    impl ToolRunner for NoLaunch {
        fn run(&self, _: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
            panic!("unsupported restore root reached external writer boundary");
        }
    }
    let outside = workspace.join("outside root");
    let link = workspace.join("junction root");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("sentinel"), b"untouched").unwrap();
    let command = required_path("SystemRoot").join("System32").join("cmd.exe");
    assert!(
        Command::new(command)
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .status()
            .unwrap()
            .success()
    );
    let refused_services = BackupServices {
        runner: &NoLaunch,
        http: services.http,
        clock: services.clock,
        restic_path: services.restic_path,
        rclone_path: services.rclone_path,
        version: services.version,
        journal_maintenance: services.journal_maintenance,
    };
    for root in [
        &link,
        &link.join("missing/restore"),
        Path::new(r"\\127.0.0.1\C$\native-backup-refusal"),
    ] {
        let outcome = restore_journal(
            root,
            &refused_services,
            &NativeRestoreRecorder,
            destination.clone(),
            recovery,
        );
        assert_eq!(outcome.status, "error", "{outcome:?}");
        assert!(!outcome.integrity_ok);
    }
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"untouched");
    assert_eq!(
        fs::read_dir(&outside).unwrap().count(),
        1,
        "refusal must not create bookkeeping or child directories"
    );
    fs::remove_dir(&link).unwrap();
    println!("NATIVE_BACKUP_UNSUPPORTED_ROOT_NO_WRITES_OK");

    struct ReplaceAfterListing<'a> {
        target: &'a Path,
        displaced: &'a Path,
        outside: &'a Path,
        ran: std::cell::Cell<bool>,
    }
    impl ToolRunner for ReplaceAfterListing<'_> {
        fn run(&self, request: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
            assert!(
                request.argv.iter().any(|arg| arg == "snapshots"),
                "changed destination reached writer"
            );
            assert!(!self.ran.replace(true), "unexpected second external launch");
            let output = SystemToolRunner.run(request)?;
            assert_eq!(output.returncode, 0);
            fs::rename(self.target, self.displaced).unwrap();
            assert!(
                Command::new(required_path("SystemRoot").join("System32").join("cmd.exe"))
                    .args(["/d", "/c", "mklink", "/J"])
                    .arg(self.target)
                    .arg(self.outside)
                    .status()
                    .unwrap()
                    .success()
            );
            Ok(output)
        }
    }
    let target = workspace.join("binding replacement target");
    let displaced = workspace.join("retained original target");
    fs::create_dir(&target).unwrap();
    let replacer = ReplaceAfterListing {
        target: &target,
        displaced: &displaced,
        outside: &outside,
        ran: std::cell::Cell::new(false),
    };
    let changed_services = BackupServices {
        runner: &replacer,
        ..refused_services
    };
    let outcome = restore_journal(
        &target,
        &changed_services,
        &NativeRestoreRecorder,
        destination.clone(),
        recovery,
    );
    assert!(
        replacer.ran.get(),
        "binding replacement control did not run"
    );
    assert_eq!(outcome.status, "error", "{outcome:?}");
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some("destination_admission_failed")
    );
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"untouched");
    assert_eq!(
        fs::read_dir(&displaced).unwrap().count(),
        0,
        "refusal wrote an attempt record"
    );
    fs::remove_dir(&target).unwrap();
    println!("NATIVE_BACKUP_REPLACED_ROOT_NO_WRITES_OK");
}

pub(super) fn refused_selective_root(
    journal: &Path,
    segment: &Path,
    workspace: &Path,
    services: &BackupServices<'_>,
) {
    struct NoRestoreWriter;
    impl ToolRunner for NoRestoreWriter {
        fn run(&self, request: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
            assert!(
                !request.argv.iter().any(|arg| arg == "restore"),
                "reparse segment reached external restore writer"
            );
            SystemToolRunner.run(request)
        }
    }
    let outside = workspace.join("displaced segment");
    fs::rename(segment, &outside).unwrap();
    fs::write(outside.join("sentinel"), b"unchanged").unwrap();
    let count = fs::read_dir(&outside).unwrap().count();
    let command = required_path("SystemRoot").join("System32").join("cmd.exe");
    assert!(
        Command::new(command)
            .args(["/d", "/c", "mklink", "/J"])
            .arg(segment)
            .arg(&outside)
            .status()
            .unwrap()
            .success()
    );
    let guarded = BackupServices {
        runner: &NoRestoreWriter,
        http: services.http,
        clock: services.clock,
        restic_path: services.restic_path,
        rclone_path: services.rclone_path,
        version: services.version,
        journal_maintenance: services.journal_maintenance,
    };
    let result = restore_all_offload(journal, &guarded);
    assert_eq!(result.status, "error", "{result:?}");
    assert_eq!(fs::read_dir(&outside).unwrap().count(), count);
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"unchanged");
    fs::remove_dir(segment).unwrap();
    fs::remove_file(outside.join("sentinel")).unwrap();
    fs::rename(outside, segment).unwrap();
    println!("NATIVE_BACKUP_SELECTIVE_REPARSE_NO_WRITES_OK");
}

fn identity() -> ProcessInstance {
    let pid = std::process::id();
    let powershell =
        required_path("SystemRoot").join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let output = Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("(Get-Process -Id {pid} -ErrorAction Stop).StartTime.ToFileTimeUtc()"),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "fixture identity query failed");
    let filetime: u64 = String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let instance =
        serde_json::from_value(json!({"pid":pid,"birth":{"kind":"windows","filetime":filetime}}))
            .unwrap();
    assert!(matches!(
        SystemProcessInstanceSource.observe(&instance),
        InstanceVerdict::SameLive { .. }
    ));
    instance
}

fn wait_identity(path: &Path, driver: &mut Child) -> ProcessInstance {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(bytes) = fs::read(path)
            && let Ok(instance) = serde_json::from_slice::<ProcessInstance>(&bytes)
        {
            assert!(matches!(
                SystemProcessInstanceSource.observe(&instance),
                InstanceVerdict::SameLive { .. }
            ));
            return instance;
        }
        assert!(
            driver.try_wait().unwrap().is_none(),
            "driver exited before child readiness"
        );
        assert!(
            Instant::now() < deadline,
            "child readiness deadline: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn assert_exited(instance: &ProcessInstance) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match SystemProcessInstanceSource.observe(instance) {
            InstanceVerdict::NotSameOrExited => return,
            InstanceVerdict::SameLive { .. } if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            other => panic!("owned fixture process survived cleanup: {instance:?}: {other:?}"),
        }
    }
}

fn wait_driver(driver: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        if let Some(status) = driver.try_wait().unwrap() {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "fixture driver exceeded its deadline"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "subprocess of backup_native_job_cleanup"]
fn backup_native_job_helper() {
    let directory = required_path("RESTIC_REPOSITORY");
    let exe = std::env::current_exe().unwrap();
    let name = exe.file_stem().unwrap().to_str().unwrap();
    assert!(matches!(name, "restic" | "rclone"));
    fs::write(
        directory.join(format!("{name}.json")),
        serde_json::to_vec(&identity()).unwrap(),
    )
    .unwrap();
    let _child = if name == "restic" {
        Some(OwnedChild(
            Command::new(exe.with_file_name("rclone.exe"))
                .args([
                    "--exact",
                    "process::backup_native_job_helper",
                    "--ignored",
                    "--nocapture",
                ])
                .stdin(Stdio::null())
                .spawn()
                .unwrap(),
        ))
    } else {
        None
    };
    std::thread::sleep(Duration::from_secs(120));
    panic!("fixture should have been stopped by Job ownership");
}

#[test]
#[ignore = "subprocess of backup_native_job_cleanup"]
fn backup_native_job_driver() {
    let directory = required_path("SOLSTONE_NATIVE_BACKUP_JOB_DIR");
    let exe = std::env::current_exe().unwrap();
    let timeout = if std::env::var("SOLSTONE_NATIVE_BACKUP_JOB_MODE").unwrap() == "timeout" {
        10
    } else {
        90
    };
    let result = run_restic(
        &SystemToolRunner,
        &[
            "--exact".into(),
            "process::backup_native_job_helper".into(),
            "--ignored".into(),
            "--nocapture".into(),
        ],
        directory.to_str().unwrap(),
        "synthetic-job-password",
        &exe.with_file_name("restic.exe"),
        None,
        false,
        None,
        Some(Duration::from_secs(timeout)),
        &[],
    )
    .expect("admitted fake tool launch");
    assert_eq!(result.returncode, 124, "{result:?}");
}

#[test]
#[ignore = "requires exclusive Windows native host capacity"]
fn backup_native_job_cleanup() {
    if std::env::var_os("SOLSTONE_NATIVE_BACKUP_STAGED").is_none() {
        stage_payload("process::backup_native_job_cleanup", true);
        return;
    }
    for mode in ["timeout", "parent_death"] {
        let directory = tempfile::tempdir().unwrap();
        let mut driver = OwnedChild(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "process::backup_native_job_driver",
                    "--ignored",
                    "--nocapture",
                ])
                .env("SOLSTONE_NATIVE_BACKUP_JOB_DIR", directory.path())
                .env("SOLSTONE_NATIVE_BACKUP_JOB_MODE", mode)
                .spawn()
                .unwrap(),
        );
        let root = wait_identity(&directory.path().join("restic.json"), &mut driver.0);
        let descendant = wait_identity(&directory.path().join("rclone.json"), &mut driver.0);
        assert_ne!(root, descendant);
        if mode == "parent_death" {
            driver.0.kill().unwrap();
            let status = wait_driver(&mut driver.0);
            assert!(!status.success());
        } else {
            assert!(wait_driver(&mut driver.0).success());
        }
        assert_exited(&root);
        assert_exited(&descendant);
        println!(
            "NATIVE_BACKUP_JOB_{}_ROOT_DESCENDANT_CLEANUP_OK",
            mode.to_uppercase()
        );
    }
}
