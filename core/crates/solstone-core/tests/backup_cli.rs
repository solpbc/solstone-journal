// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

fn command(journal: &tempfile::TempDir, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command.args(["backup"]);
    command.args(args);
    command.env("SOLSTONE_JOURNAL", journal.path());
    command
}

fn output(journal: &tempfile::TempDir, args: &[&str]) -> Output {
    command(journal, args)
        .output()
        .expect("backup command runs")
}

fn output_with_stdin(journal: &tempfile::TempDir, args: &[&str], input: &str) -> Output {
    let mut child = command(journal, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("backup command starts");
    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(input.as_bytes())
        .expect("stdin writes");
    child.wait_with_output().expect("backup command completes")
}

fn write_config(journal: &tempfile::TempDir, bytes: &[u8]) -> std::path::PathBuf {
    let path = journal.path().join("config/journal.json");
    fs::create_dir_all(path.parent().expect("config parent")).expect("config directory");
    fs::write(&path, bytes).expect("config writes");
    path
}

#[cfg(unix)]
#[test]
fn restore_process_reports_tool_resolution_failure_without_pinning_restic_version() {
    let journal = tempfile::tempdir().expect("journal");
    let home = tempfile::tempdir().expect("home");
    let missing_bundle = home.path().join("missing-restic.bz2");
    let input = r#"{"recovery_key":"0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ","repository":"s3:unreachable.example.invalid/journal","backend":"s3","credentials":{"access_key_id":"access","secret_access_key":"secret"}}"#;
    let restore = |args: &[&str]| {
        let mut child = command(&journal, args)
            .env("HOME", home.path())
            .env("SOLSTONE_RESTIC_BUNDLE", &missing_bundle)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("restore starts");
        child
            .stdin
            .take()
            .expect("restore stdin")
            .write_all(input.as_bytes())
            .expect("restore input");
        child.wait_with_output().expect("restore completes")
    };

    let human = restore(&["restore"]);
    assert_eq!(human.status.code(), Some(1));
    assert!(human.stdout.is_empty());
    assert_eq!(
        String::from_utf8(human.stderr).expect("human error"),
        "Error: Restore failed: restic_unavailable; files_expected=unknown, files_restored=unknown, bytes_expected=unknown, bytes_restored=unknown.\n"
    );

    let json = restore(&["restore", "--json"]);
    assert_eq!(json.status.code(), Some(1));
    assert!(json.stderr.is_empty());
    let rendered: Value = serde_json::from_slice(&json.stdout).expect("restore JSON");
    assert_eq!(
        rendered,
        json!({
            "bytes_expected": null,
            "bytes_restored": null,
            "files_expected": null,
            "files_restored": null,
            "integrity_ok": false,
            "reason_code": "restic_unavailable",
            "recording_failure": null,
            "resumable": false,
            "status": "error",
        })
    );
}

#[test]
fn status_is_read_only_and_usage_is_backup_owned() {
    let journal = tempfile::tempdir().expect("journal");
    let status = output(&journal, &["status"]);
    assert_eq!(status.status.code(), Some(0));
    assert!(status.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&status.stdout).unwrap()["enabled"],
        false
    );
    assert_eq!(fs::read_dir(journal.path()).unwrap().count(), 0);

    let invalid = output(&journal, &["--nonsense"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    assert_eq!(
        String::from_utf8(invalid.stderr).unwrap(),
        "usage: journal backup <command> [options]\njournal backup: error: unrecognized arguments: --nonsense\n"
    );
}

#[test]
fn config_read_failures_are_visible_and_read_only() {
    for bytes in [b"not json".as_slice(), b"[]".as_slice()] {
        let journal = tempfile::tempdir().expect("journal");
        let config = write_config(&journal, bytes);
        let original = fs::read(&config).unwrap();
        let modified = fs::metadata(&config).unwrap().modified().unwrap();
        for args in [
            ["status"].as_slice(),
            ["destination", "show"].as_slice(),
            ["recovery-key", "show"].as_slice(),
        ] {
            let result = output(&journal, args);
            assert_eq!(result.status.code(), Some(1));
            assert!(result.stdout.is_empty());
            let stderr = String::from_utf8(result.stderr).unwrap();
            assert!(stderr.starts_with("Error: I couldn't read your settings file at "));
            assert!(stderr.contains("Your settings were NOT changed."));
            assert_eq!(fs::read(&config).unwrap(), original);
            assert_eq!(fs::metadata(&config).unwrap().modified().unwrap(), modified);
        }
    }

    let journal = tempfile::tempdir().expect("journal");
    let config = journal.path().join("config/journal.json");
    fs::create_dir_all(&config).unwrap();
    let modified = fs::metadata(&config).unwrap().modified().unwrap();
    let result = output(&journal, &["status"]);
    assert_eq!(result.status.code(), Some(1));
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .contains("Your settings were NOT changed.")
    );
    assert!(fs::metadata(&config).unwrap().is_dir());
    assert_eq!(fs::metadata(&config).unwrap().modified().unwrap(), modified);
}

#[test]
fn unreadable_config_fails_each_read_verb_without_changing_metadata() {
    let journal = tempfile::tempdir().expect("journal");
    let config = journal.path().join("config/journal.json");
    fs::create_dir_all(&config).unwrap();
    let modified = fs::metadata(&config).unwrap().modified().unwrap();
    for args in [
        ["status"].as_slice(),
        ["destination", "show"].as_slice(),
        ["recovery-key", "show"].as_slice(),
    ] {
        let result = output(&journal, args);
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        assert!(
            String::from_utf8(result.stderr)
                .unwrap()
                .contains("Your settings were NOT changed.")
        );
        assert!(fs::metadata(&config).unwrap().is_dir());
        assert_eq!(fs::metadata(&config).unwrap().modified().unwrap(), modified);
    }
}

#[test]
fn destination_show_matches_redacted_status_destination() {
    let journal = tempfile::tempdir().expect("journal");
    write_config(&journal, serde_json::to_string(&json!({
        "backup": {"destination": {"repository": "s3:bucket/repo", "backend": "s3", "credentials": {"access_key_id": "secret"}}}
    })).unwrap().as_bytes());
    let status = output(&journal, &["status"]);
    let destination = output(&journal, &["destination", "show"]);
    assert_eq!(status.status.code(), Some(0));
    assert_eq!(destination.status.code(), Some(0));
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    let destination: Value = serde_json::from_slice(&destination.stdout).unwrap();
    assert_eq!(destination, status["destination"]);
    assert_eq!(destination["credentials_set"], true);
    assert!(!destination.to_string().contains("secret"));
}

#[test]
fn status_reports_scalar_destination_as_a_clean_runtime_error() {
    let journal = tempfile::tempdir().expect("journal");
    let config = write_config(
        &journal,
        serde_json::to_string(&json!({"backup": {"destination": "not an object"}}))
            .unwrap()
            .as_bytes(),
    );
    let before = fs::read(&config).unwrap();
    let modified = fs::metadata(&config).unwrap().modified().unwrap();

    let result = output(&journal, &["status"]);

    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert_eq!(stderr, "Error: backup destination must be a JSON object\n");
    assert!(!stderr.contains("panicked"));
    assert_eq!(fs::read(&config).unwrap(), before);
    assert_eq!(fs::metadata(&config).unwrap().modified().unwrap(), modified);
}

#[test]
fn status_redacts_config_and_hosted_binding_secrets() {
    let journal = tempfile::tempdir().expect("journal");
    let recovery = "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    write_config(&journal, serde_json::to_string(&json!({
        "backup": {
            "daily_key": "DAILY_SECRET",
            "recovery_key": recovery,
            "destination": {"repository": "repo", "backend": "s3", "credentials": {"access_key_id": "CREDENTIAL_SECRET"}}
        }
    })).unwrap().as_bytes());
    let binding = journal.path().join("backup/hosted/binding.json");
    fs::create_dir_all(binding.parent().unwrap()).unwrap();
    fs::write(&binding, serde_json::to_vec(&json!({
        "broker_endpoint": "BROKER_ENDPOINT_SECRET", "account_id": "ACCOUNT_SECRET", "instance_id": "INSTANCE_SECRET", "bucket": "bucket", "prefix": "prefix", "broker_token": "BROKER_TOKEN_SECRET"
    })).unwrap()).unwrap();
    let result = output(&journal, &["status"]);
    assert_eq!(result.status.code(), Some(0));
    let stdout = String::from_utf8(result.stdout).unwrap();
    for secret in [
        "DAILY_SECRET",
        recovery,
        "CREDENTIAL_SECRET",
        "BROKER_ENDPOINT_SECRET",
        "ACCOUNT_SECRET",
        "INSTANCE_SECRET",
        "BROKER_TOKEN_SECRET",
    ] {
        assert!(!stdout.contains(secret), "status leaked {secret}");
    }
}

#[test]
fn recovery_key_show_is_read_only_and_renders_four_rows() {
    let journal = tempfile::tempdir().expect("journal");
    let recovery = "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let config = write_config(
        &journal,
        serde_json::to_string(&json!({
            "backup": {"daily_key": "daily", "recovery_key": recovery}
        }))
        .unwrap()
        .as_bytes(),
    );
    let before = fs::read(&config).unwrap();
    let metadata = fs::metadata(&config).unwrap();
    let modified = metadata.modified().unwrap();
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(&metadata);

    let result = output(&journal, &["recovery-key", "show"]);
    assert_eq!(result.status.code(), Some(0));
    assert!(result.stderr.is_empty());
    assert_eq!(
        String::from_utf8(result.stdout.clone()).unwrap(),
        "0123 4567 89AB CDEF\nGHJK MNPQ RSTV WXYZ\n0123 4567 89AB CDEF\nGHJK MNPQ RSTV WXYZ\n"
    );
    assert!(
        !String::from_utf8(result.stdout.clone())
            .unwrap()
            .contains("daily")
    );
    for line in String::from_utf8(result.stdout).unwrap().lines() {
        let groups = line.split(' ').collect::<Vec<_>>();
        assert_eq!(groups.len(), 4);
        assert!(groups.iter().all(|group| group.len() == 4));
    }
    assert_eq!(fs::read(&config).unwrap(), before);
    let after = fs::metadata(&config).unwrap();
    assert_eq!(after.modified().unwrap(), modified);
    #[cfg(unix)]
    assert_eq!(std::os::unix::fs::MetadataExt::ino(&after), inode);
}

#[test]
fn nested_backup_grammar_reaches_native_destination_offload_and_recovery_bodies() {
    let journal = tempfile::tempdir().expect("journal");
    let hosted = output_with_stdin(
        &journal,
        &["destination", "set-hosted"],
        r#"{"broker_endpoint":"https://broker","account_id":"account","instance_id":"instance","bucket":"bucket","prefix":"prefix","broker_token":"token"}"#,
    );
    assert_eq!(hosted.status.code(), Some(0));
    assert_eq!(
        serde_json::from_slice::<Value>(&hosted.stdout).unwrap()["bound"],
        true
    );

    let offload = output(&journal, &["offload", "status"]);
    assert_eq!(offload.status.code(), Some(0));
    assert!(
        String::from_utf8(offload.stdout)
            .unwrap()
            .starts_with("backup offload: enabled=")
    );

    let recovery = "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    write_config(
        &journal,
        serde_json::to_string(&json!({
            "backup": {"daily_key": "daily", "recovery_key": recovery}
        }))
        .unwrap()
        .as_bytes(),
    );
    let recovery_show = output(&journal, &["recovery-key", "show"]);
    assert_eq!(recovery_show.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(recovery_show.stdout)
            .unwrap()
            .lines()
            .count(),
        4
    );
}

#[test]
fn set_hosted_trims_all_fields_and_redacts_token() {
    let journal = tempfile::tempdir().expect("journal");
    let success = output_with_stdin(
        &journal,
        &["destination", "set-hosted"],
        r#"{"broker_endpoint":" https://b ","account_id":" a ","instance_id":" i ","bucket":" bk ","prefix":" p ","broker_token":" token "}"#,
    );
    assert_eq!(success.status.code(), Some(0));
    let response: Value = serde_json::from_slice(&success.stdout).unwrap();
    assert_eq!(
        response,
        json!({"account_id":"a","bound":true,"broker_endpoint":"https://b","bucket":"bk","instance_id":"i","prefix":"p"})
    );
    assert!(!String::from_utf8(success.stdout).unwrap().contains("token"));
    let binding = journal.path().join("backup/hosted/binding.json");
    assert!(!journal.path().join("config/journal.json").exists());
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::MetadataExt::mode(&fs::metadata(&binding).unwrap()) & 0o777,
        0o600
    );
}

fn valid_hosted_payload() -> serde_json::Map<String, Value> {
    serde_json::Map::from_iter([
        ("broker_endpoint".into(), json!("broker")),
        ("account_id".into(), json!("account")),
        ("instance_id".into(), json!("instance")),
        ("bucket".into(), json!("bucket")),
        ("prefix".into(), json!("prefix")),
        ("broker_token".into(), json!("PAYLOAD_SECRET")),
    ])
}

#[test]
fn set_hosted_rejects_json_boundaries_without_creating_binding() {
    for (input, boundary) in [
        ("", "expected JSON object"),
        ("{", "invalid JSON"),
        ("[]", "expected JSON object"),
    ] {
        let journal = tempfile::tempdir().unwrap();
        let result = output_with_stdin(&journal, &["destination", "set-hosted"], input);
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        assert!(String::from_utf8(result.stderr).unwrap().contains(boundary));
        assert!(!journal.path().join("backup").exists());
    }
}

#[test]
fn set_hosted_rejects_each_missing_non_string_and_blank_field_without_leaking_payload() {
    for field in [
        "broker_endpoint",
        "account_id",
        "instance_id",
        "bucket",
        "prefix",
        "broker_token",
    ] {
        for replacement in [None, Some(json!(7)), Some(json!("  "))] {
            let journal = tempfile::tempdir().unwrap();
            let mut payload = valid_hosted_payload();
            match replacement {
                Some(value) => {
                    payload.insert(field.into(), value);
                }
                None => {
                    payload.remove(field);
                }
            }
            let result = output_with_stdin(
                &journal,
                &["destination", "set-hosted"],
                &Value::Object(payload).to_string(),
            );
            assert_eq!(result.status.code(), Some(1), "{field}");
            assert!(result.stdout.is_empty());
            let stderr = String::from_utf8(result.stderr).unwrap();
            assert!(stderr.contains(field));
            assert!(!stderr.contains("PAYLOAD_SECRET"));
            assert!(!journal.path().join("backup").exists());
        }
    }
}

#[test]
fn set_hosted_rejects_extra_arguments_without_changing_existing_binding() {
    let journal = tempfile::tempdir().unwrap();
    let first = output_with_stdin(
        &journal,
        &["destination", "set-hosted"],
        &Value::Object(valid_hosted_payload()).to_string(),
    );
    assert_eq!(first.status.code(), Some(0));
    let binding = journal.path().join("backup/hosted/binding.json");
    let before = fs::read(&binding).unwrap();
    let result = output_with_stdin(
        &journal,
        &["destination", "set-hosted", "extra"],
        "PAYLOAD_SECRET",
    );
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    assert!(
        !String::from_utf8(result.stderr)
            .unwrap()
            .contains("PAYLOAD_SECRET")
    );
    assert_eq!(fs::read(&binding).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn set_hosted_write_failure_has_no_success_output_or_secret_leak() {
    use std::os::unix::fs::PermissionsExt;

    let journal = tempfile::tempdir().unwrap();
    let binding = journal.path().join("backup/hosted/binding.json");
    fs::create_dir_all(binding.parent().unwrap()).unwrap();
    fs::write(&binding, b"prior binding bytes").unwrap();
    let directory = binding.parent().unwrap();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o500)).unwrap();
    let result = output_with_stdin(
        &journal,
        &["destination", "set-hosted"],
        &Value::Object(valid_hosted_payload()).to_string(),
    );
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    assert!(
        !String::from_utf8(result.stderr)
            .unwrap()
            .contains("PAYLOAD_SECRET")
    );
    assert_eq!(fs::read(&binding).unwrap(), b"prior binding bytes");
}
