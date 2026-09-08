// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real tool acceptance using disposable journals and a loopback S3 server.

use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{Datelike, Utc};
use minisign::KeyPair;
use serde_json::json;
use sha2::{Digest, Sha256};
use solstone_core_backup::{
    Destination, HostedBinding, generate_and_store_keys, get_backup_config, get_keys,
    set_destination, set_enabled, set_offload, set_recovery_key_confirmed,
};
use solstone_core_backup_runtime::{
    BackupServices, Clock, HostedCredentials, NativeJournalMaintenance, NativeRestoreRecorder,
    SystemToolRunner, ToolOutput, ToolRequest, ToolRunner, UreqHttpTransport,
    hosted_append_only_session, init_repository, restore_journal, rotate_recovery_key, run_backup,
    run_prune, run_restic, run_verification, validate_destination,
};
use solstone_core_distribution::windows_payload::{
    WINDOWS_PAYLOAD_MANIFEST, WINDOWS_PAYLOAD_SIGNATURE, render_windows_payload_manifest,
    verify_windows_payload,
};
use solstone_core_offload::{restore_all_offload, run_offload};
use solstone_core_retention::{Anchor, Days, Policy, Rule, marks, remove_marked::remove_marked};

struct WallClock;
struct ShortDeadline;
impl ToolRunner for ShortDeadline {
    fn run(&self, request: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
        let mut request = request.clone();
        request.timeout = Some(Duration::from_secs(2));
        SystemToolRunner.run(&request)
    }
}
impl Clock for WallClock {
    fn now_unix(&self) -> i64 {
        Utc::now().timestamp()
    }
    fn iso_week(&self) -> u8 {
        Utc::now().iso_week().week() as u8
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        self.0.wait().expect("reap fixture server");
    }
}

fn required_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} is required")))
}

fn suffix() -> &'static str {
    if cfg!(windows) { ".exe" } else { "" }
}

fn stage_payload(test_name: &str, fake_tools: bool) {
    let tmp = tempfile::tempdir().expect("staging directory");
    let root = tmp.path().join(if cfg!(windows) {
        "Backup package café owner's path"
    } else {
        "backup-package"
    });
    fs::create_dir_all(root.join("bin")).unwrap();
    let fixture = root.join(format!("bin/backup-native{}", suffix()));
    fs::copy(std::env::current_exe().unwrap(), &fixture).unwrap();
    for (tool, env, digest) in [
        (
            "restic",
            "SOLSTONE_NATIVE_RESTIC",
            "40576f77c1d40245a9f4af92a0b37b0d2514e6be0dffbf16ca8855820c13693e",
        ),
        (
            "rclone",
            "SOLSTONE_NATIVE_RCLONE",
            "492648a3867dbc620188a305e05ff3216aecbf4622bf1a6b5b978ed9c939e18c",
        ),
    ] {
        let source = if fake_tools {
            fixture.clone()
        } else {
            required_path(env)
        };
        if cfg!(windows) && !fake_tools {
            assert_eq!(
                format!("{:x}", Sha256::digest(fs::read(&source).unwrap())),
                digest
            );
        }
        fs::copy(source, root.join(format!("bin/{tool}{}", suffix()))).unwrap();
    }
    let commit = std::env::var("SOLSTONE_NATIVE_SOURCE_COMMIT").expect("source commit");
    let lock = std::env::var("SOLSTONE_NATIVE_LOCK_SHA256").expect("Cargo.lock digest");
    let manifest = render_windows_payload_manifest(&root, &commit, &lock).unwrap();
    let KeyPair { pk, sk } = KeyPair::generate_unencrypted_keypair().unwrap();
    let pin = tmp.path().join("fixture.pub");
    fs::write(&pin, pk.to_box().unwrap().to_bytes()).unwrap();
    let signature = minisign::sign(
        Some(&pk),
        &sk,
        Cursor::new(&manifest),
        None,
        Some("native fixture"),
    )
    .unwrap();
    fs::create_dir_all(root.join("share/provenance")).unwrap();
    fs::write(root.join(WINDOWS_PAYLOAD_MANIFEST), manifest).unwrap();
    fs::write(
        root.join(WINDOWS_PAYLOAD_SIGNATURE),
        signature.into_string(),
    )
    .unwrap();
    #[cfg(unix)]
    let status = Command::new(&fixture)
        .args(["--exact", test_name, "--ignored", "--nocapture"])
        .env("SOLSTONE_NATIVE_BACKUP_STAGED", "1")
        .env("SOLSTONE_JOURNAL_MINISIGN_PIN", &pin)
        .status()
        .unwrap();
    #[cfg(unix)]
    assert!(status.success(), "staged native fixture failed: {status}");
    #[cfg(windows)]
    {
        use solstone_core_system::process::{
            BoundedHelperBudget, BoundedHelperRequest, run_bounded_helper,
        };
        let mut environment = BTreeMap::new();
        for name in [
            "SystemRoot",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "APPDATA",
            "LOCALAPPDATA",
        ] {
            if let Some(value) = std::env::var_os(name) {
                environment.insert(name.into(), value);
            }
        }
        environment.insert("SOLSTONE_NATIVE_BACKUP_STAGED".into(), "1".into());
        environment.insert("SOLSTONE_JOURNAL_MINISIGN_PIN".into(), pin.into_os_string());
        let output = run_bounded_helper(BoundedHelperRequest {
            package_root: root.clone(),
            executable: fixture,
            current_directory: root,
            arguments: ["--exact", test_name, "--ignored", "--nocapture"]
                .map(str::to_owned)
                .to_vec(),
            environment,
            stdin: vec![],
            resource_limits: None,
            budget: BoundedHelperBudget {
                timeout: Duration::from_secs(240),
                stdin_limit_bytes: 1,
                stdout_limit_bytes: 4 * 1024 * 1024,
                stderr_limit_bytes: 4 * 1024 * 1024,
            },
        })
        .expect("bounded staged fixture; Job cleanup includes fixture server and descendants");
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.quiescent, "staged fixture did not become quiescent");
        assert_eq!(output.exit_code, 0, "staged native fixture failed");
    }
}

fn serve_s3(rclone: &Path, directory: &Path) -> (OwnedChild, String) {
    fs::create_dir_all(directory.join("fixture-bucket")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let child = Command::new(rclone)
        .args(["serve", "s3"])
        .arg(directory)
        .args([
            "--no-cleanup",
            "--addr",
            &address.to_string(),
            "--config",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        ])
        .env(
            "RCLONE_AUTH_KEY",
            "\"native-fixture-access,native-fixture-secret\"",
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("fixture S3 server");
    let mut owned = OwnedChild(child);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            owned.0.try_wait().unwrap().is_none(),
            "S3 fixture exited before readiness"
        );
        if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "S3 fixture readiness deadline");
        std::thread::sleep(Duration::from_millis(50));
    }
    (owned, format!("http://{address}"))
}

fn raw_fixture(journal: &Path, day: &str, content: &[u8]) -> PathBuf {
    let raw = journal.join(format!("chronicle/{day}/010000_001/raw.webm"));
    fs::create_dir_all(raw.parent().unwrap()).unwrap();
    fs::write(&raw, content).unwrap();
    let header = json!({"_solstone_processing": {
        "schema":"solstone.processing.v1", "state":"empty",
        "reason_code":"no_decodable_frames", "handler":"describe",
        "attempted_at":"2026-01-01T00:00:00Z", "input_size":content.len()
    }});
    fs::write(raw.with_extension("jsonl"), format!("{header}\n")).unwrap();
    raw
}

#[test]
#[ignore = "requires pinned restic/rclone artifacts and exclusive native host capacity"]
fn backup_native_round_trip() {
    if std::env::var_os("SOLSTONE_NATIVE_BACKUP_STAGED").is_none() {
        stage_payload("backup_native_round_trip", false);
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let package = exe.parent().unwrap().parent().unwrap();
    verify_windows_payload(package).expect("signed fixture admission");
    let restic = package.join(format!("bin/restic{}", suffix()));
    let rclone = package.join(format!("bin/rclone{}", suffix()));
    // Restic's existing .tmp* exclusion also matches ancestor directories.
    let tmp = tempfile::Builder::new()
        .prefix("native-backup-data-")
        .tempdir()
        .unwrap();
    let (mut server, endpoint) = serve_s3(&rclone, &tmp.path().join("s3-storage"));
    #[cfg(windows)]
    process::installed_tools(&restic, &rclone, tmp.path());
    let destination = Destination {
        repository: format!("s3:{endpoint}/fixture-bucket/byo"),
        backend: "s3".into(),
        credentials: json!({"access_key_id":"native-fixture-access", "secret_access_key":"native-fixture-secret"}).as_object().unwrap().clone(),
    };
    let journal = tmp.path().join("journal café");
    fs::create_dir(&journal).unwrap();
    fs::write(
        journal.join("mcp-endpoint"),
        b"synthetic endpoint excluded from backup",
    )
    .unwrap();
    let raw = raw_fixture(&journal, "20260101", b"native original one");
    let original = fs::read(&raw).unwrap();
    let runner = SystemToolRunner;
    let clock = WallClock;
    let services = BackupServices {
        runner: &runner,
        http: &UreqHttpTransport,
        clock: &clock,
        restic_path: Some(&restic),
        rclone_path: Some(&rclone),
        version: "native-fixture",
        journal_maintenance: &NativeJournalMaintenance,
    };
    set_destination(&journal, &destination).unwrap();
    let keys = generate_and_store_keys(&journal).unwrap();
    init_repository(
        &runner,
        &destination,
        &keys.daily_key,
        &keys.recovery_key,
        &restic,
        Some(Duration::from_secs(60)),
    )
    .expect("init and piped recovery key");
    set_enabled(&journal, true).unwrap();
    set_recovery_key_confirmed(&journal, true).unwrap();
    let backup = run_backup(&journal, &services);
    assert_eq!(backup.status, "ok", "{backup:?}");
    let verification = run_verification(&journal, &services, &clock);
    assert_eq!(verification.status, "ok", "{verification:?}");
    let rotated = rotate_recovery_key(&journal, &services);
    assert_eq!(rotated.status, "ok", "{rotated:?}");
    let active_keys = get_keys(&journal).unwrap().unwrap();
    assert_ne!(keys.recovery_key, active_keys.recovery_key);
    assert_eq!(
        validate_destination(
            &runner,
            &destination,
            &keys.recovery_key,
            &restic,
            Some(Duration::from_secs(30))
        )
        .unwrap()
        .reason_code,
        "auth_failed"
    );
    set_recovery_key_confirmed(&journal, true).unwrap();

    let fresh = tmp.path().join("fresh recovery café");
    assert!(!fresh.exists());
    #[cfg(unix)]
    fs::create_dir(&fresh).unwrap();
    let restored = restore_journal(
        &fresh,
        &services,
        &NativeRestoreRecorder,
        destination.clone(),
        &active_keys.recovery_key,
    );
    assert_eq!(restored.status, "ok", "{restored:?}");
    assert!(restored.integrity_ok);
    assert_eq!(
        fs::read(fresh.join(raw.strip_prefix(&journal).unwrap())).unwrap(),
        original
    );
    assert!(
        !fresh.join("mcp-endpoint").exists(),
        "endpoint exclusion must survive Windows path normalization"
    );
    println!("NATIVE_BACKUP_BYO_RECOVERY_OK");
    #[cfg(windows)]
    process::refused_restore_roots(
        tmp.path(),
        &services,
        &destination,
        &active_keys.recovery_key,
    );

    set_offload(
        &journal,
        json!({"enabled":true,"budget_bytes":1,"floor_bytes":1})
            .as_object()
            .unwrap(),
    )
    .unwrap();
    let marked = run_offload(&journal, &services, false);
    assert_eq!(marked.status, "ok", "{marked:?}");
    assert_eq!(marked.files_marked, 1, "{marked:?}");
    assert_eq!(
        fs::read(&raw).unwrap(),
        original,
        "marking must preserve local originals"
    );
    let register = marks::load(&journal).unwrap();
    let ids = register.marks.keys().cloned().collect::<Vec<_>>();
    assert_eq!(ids.len(), 1);
    let prune = run_prune(&journal, &services);
    assert_eq!(prune.status, "ok", "{prune:?}");
    let approved = marks::preflight(&journal, &ids).unwrap();
    let policy = Policy {
        default_rule: Rule {
            anchor: Anchor::Captured,
            period: Some(Days(1)),
            priority: 0,
        },
        empty_audio_rule: Rule {
            anchor: Anchor::Captured,
            period: Some(Days(1)),
            priority: 0,
        },
        enabled: true,
        ..Policy::default()
    };
    let mut errors = vec![];
    let removed = remove_marked(
        &journal,
        &approved,
        &policy,
        Utc::now().date_naive(),
        Utc::now(),
        &mut errors,
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert!(removed.halted.is_none(), "{removed:?}");
    assert!(
        !raw.exists(),
        "owner-approved fixture removal must execute: {removed:?}"
    );
    #[cfg(windows)]
    process::refused_selective_root(&journal, raw.parent().unwrap(), tmp.path(), &services);
    let ledger = solstone_core_offload::ledger_path_for_day(&journal, "20260101").unwrap();
    let witness = fs::read_to_string(&ledger).unwrap();
    let expected_digest = format!("{:x}", Sha256::digest(&original));
    assert!(witness.contains(&expected_digest));
    fs::write(&ledger, witness.replace(&expected_digest, &"0".repeat(64))).unwrap();
    let rejected = restore_all_offload(&journal, &services);
    assert_eq!(rejected.status, "error", "{rejected:?}");
    assert!(
        !raw.exists(),
        "digest mismatch must roll back newly restored bytes"
    );
    fs::write(&ledger, witness).unwrap();
    let restored = restore_all_offload(&journal, &services);
    assert_eq!(restored.status, "ok", "{restored:?}");
    assert_eq!(
        fs::read(&raw).unwrap(),
        original,
        "archive survived prune and restored exact bytes"
    );
    println!("NATIVE_BACKUP_OFFLOAD_PRUNE_RESTORE_OK");

    // Exercise the actual restic -> rclone parser and no-config mechanism with
    // a synthetic binding. Broker consent is a separate integration proof.
    let binding = HostedBinding {
        broker_endpoint: "http://127.0.0.1/unused".into(),
        account_id: "fixture".into(),
        instance_id: "fixture".into(),
        bucket: "fixture-bucket".into(),
        prefix: "byo".into(),
        broker_token: "fixture-token".into(),
    };
    let credentials = HostedCredentials {
        access_key_id: "native-fixture-access".into(),
        secret_access_key: "native-fixture-secret".into(),
        session_token: String::new(),
        endpoint,
        expires_at: "2099-01-01T00:00:00Z".into(),
    };
    let hosted = hosted_append_only_session(&binding, &credentials, &rclone).unwrap();
    let env = hosted
        .backend_env
        .iter()
        .map(|(k, v)| (k.clone(), Some(v.clone())))
        .collect::<BTreeMap<_, _>>();
    let mut args = hosted.global_options.clone();
    args.push("snapshots".into());
    let via_rclone = run_restic(
        &runner,
        &args,
        &hosted.destination.repository,
        &active_keys.daily_key,
        &restic,
        Some(&env),
        true,
        None,
        Some(Duration::from_secs(60)),
        &[],
    )
    .unwrap();
    assert_eq!(
        via_rclone.returncode, 0,
        "{via_rclone:?}; scrubbed stderr: {}",
        via_rclone.stderr
    );
    println!("NATIVE_BACKUP_RCLONE_PATH_NUL_OK");

    let blackhole = TcpListener::bind("127.0.0.1:0").unwrap();
    let blocked_credentials = HostedCredentials {
        endpoint: format!("http://{}", blackhole.local_addr().unwrap()),
        ..credentials
    };
    let blocked = hosted_append_only_session(&binding, &blocked_credentials, &rclone).unwrap();
    let blocked_env = blocked
        .backend_env
        .iter()
        .map(|(key, value)| (key.clone(), Some(value.clone())))
        .collect::<BTreeMap<_, _>>();
    let mut blocked_args = blocked.global_options;
    blocked_args.push("snapshots".into());
    let timed_out = run_restic(
        &runner,
        &blocked_args,
        &blocked.destination.repository,
        &active_keys.daily_key,
        &restic,
        Some(&blocked_env),
        true,
        None,
        Some(Duration::from_secs(2)),
        &[],
    )
    .unwrap();
    assert_eq!(timed_out.returncode, 124, "{timed_out:?}");
    drop(blackhole);
    println!("NATIVE_BACKUP_RCLONE_TIMEOUT_OK");

    let confirmed_again = run_offload(&journal, &services, false);
    assert_eq!(confirmed_again.status, "ok", "{confirmed_again:?}");
    assert_eq!(confirmed_again.files_marked, 1, "{confirmed_again:?}");
    // The older unconfirmed segment is the first archive attempt on the next run.
    let pending = raw_fixture(&journal, "20251231", b"never confirmed remotely");
    let pending_bytes = fs::read(&pending).unwrap();
    server.0.kill().unwrap();
    server.0.wait().unwrap();
    let before = marks::load(&journal).unwrap();
    assert_eq!(before.marks.len(), 1, "retain the earlier confirmed mark");
    let failing_services = BackupServices {
        runner: &ShortDeadline,
        ..services
    };
    let failed = run_offload(&journal, &failing_services, false);
    assert_eq!(failed.status, "stalled", "{failed:?}");
    assert_eq!(failed.files_marked, 0, "{failed:?}");
    assert_eq!(fs::read(&pending).unwrap(), pending_bytes);
    assert_eq!(marks::load(&journal).unwrap().marks, before.marks);
    assert!(
        get_backup_config(&journal)
            .unwrap()
            .contains_key("last_offload")
    );
    println!("NATIVE_BACKUP_UNVERIFIED_NO_MARK_OR_REMOVAL_OK");
    drop(server);
    #[cfg(windows)]
    process::poisoned_payload(&restic, &rclone, &destination.repository);
}

#[cfg(windows)]
#[path = "backup_native/process.rs"]
mod process;
