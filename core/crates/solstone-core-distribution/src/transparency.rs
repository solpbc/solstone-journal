// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Revision- and executable-pinned bridge to the shared transparency rail.

use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{AtomicWriteOptions, LockOptions, atomic_replace, hold_lock};

use crate::digest::sha256_hex;
use crate::inventory::repository_inventory_path;

const PIN_BYTES: &[u8] = include_bytes!("../../../distribution/solstone-transparency-pin.json");
const PIN_SCHEMA: &str = "solstone-journal/transparency-tool-pin/v1";
const REGISTRY_SCHEMA: &str = "solstone-journal/v2-origin-release-registry/v1";
const MAX_VERIFIER_OUTPUT: usize = 1024 * 1024;
const MAX_PINNED_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolPin {
    schema: String,
    repository: String,
    version: String,
    commit: String,
    executables: Executables,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Executables {
    journal_artifacts: ExecutablePin,
    verify_release: ExecutablePin,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutablePin {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct VerificationOutput {
    ok: bool,
    product: String,
    version: String,
    #[serde(rename = "recordPath")]
    record_path: String,
    #[serde(rename = "recordSha256")]
    record_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct V2ReleaseRegistry {
    schema: String,
    releases: Vec<VerifiedV2Release>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VerifiedV2Release {
    version: String,
    logical_target: String,
    record_sha256: String,
}

fn usage() -> &'static str {
    "usage: solstone-distribution <journal-artifacts --transparency-repo DIR -- ADAPTER_ARGS...|register-v2-origin --transparency-repo DIR --root FILE --version VERSION --store FILE [--metadata-base URL] [--targets-base URL] --apply>"
}

fn lowercase_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn safe_relative(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn load_pin() -> Result<ToolPin, String> {
    let pin = serde_json::from_slice::<ToolPin>(PIN_BYTES)
        .map_err(|error| format!("invalid transparency tool pin: {error}"))?;
    if pin.schema != PIN_SCHEMA
        || pin.repository != "https://github.com/solpbc/solstone-transparency.git"
        || pin.version.is_empty()
        || !lowercase_hex(&pin.commit, 40)
    {
        return Err("invalid transparency tool pin identity".into());
    }
    let mut paths = BTreeSet::new();
    for executable in [
        &pin.executables.journal_artifacts,
        &pin.executables.verify_release,
    ] {
        if !safe_relative(&executable.path)
            || !lowercase_hex(&executable.sha256, 64)
            || !paths.insert(executable.path.clone())
        {
            return Err("invalid transparency executable pin".into());
        }
    }
    Ok(pin)
}

fn git_command(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .stdin(Stdio::null());
    command
}

fn git_output(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = git_command(repo)
        .args(args)
        .output()
        .map_err(|_| "could not execute git for transparency pin verification".to_owned())?;
    if !output.status.success() || output.stdout.len() > MAX_VERIFIER_OUTPUT {
        return Err("transparency checkout could not be verified by git".into());
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim_end().to_owned())
        .map_err(|_| "git returned non-UTF-8 transparency checkout identity".into())
}

fn validate_checkout_identity(head: &str, status: &str, pin: &ToolPin) -> Result<(), String> {
    if head != pin.commit {
        return Err(format!(
            "transparency checkout is not pinned commit {}",
            pin.commit
        ));
    }
    if !status.is_empty() {
        return Err("transparency checkout has tracked modifications".into());
    }
    Ok(())
}

fn unpack_pinned_archive(bytes: Vec<u8>, pin: &ToolPin) -> Result<tempfile::TempDir, String> {
    if bytes.len() > MAX_PINNED_ARCHIVE_BYTES {
        return Err("pinned transparency commit archive is oversized".into());
    }
    let snapshot = tempfile::tempdir()
        .map_err(|_| "could not create a temporary transparency snapshot".to_owned())?;
    tar::Archive::new(Cursor::new(bytes))
        .unpack(snapshot.path())
        .map_err(|_| "could not unpack the pinned transparency commit".to_owned())?;
    for executable in [
        &pin.executables.journal_artifacts,
        &pin.executables.verify_release,
    ] {
        let bytes = std::fs::read(snapshot.path().join(&executable.path))
            .map_err(|_| format!("cannot read pinned executable {}", executable.path))?;
        if sha256_hex(&bytes) != executable.sha256 {
            return Err(format!(
                "pinned executable digest mismatch for {}",
                executable.path
            ));
        }
    }
    Ok(snapshot)
}

fn materialize_pinned_checkout(repo: &Path, pin: &ToolPin) -> Result<tempfile::TempDir, String> {
    let head = git_output(repo, &["rev-parse", "HEAD"])?;
    let status = git_output(repo, &["status", "--porcelain", "--untracked-files=no"])?;
    validate_checkout_identity(&head, &status, pin)?;
    let archive = git_command(repo)
        .args(["archive", "--format=tar", &pin.commit])
        .output()
        .map_err(|_| "could not materialize the pinned transparency commit".to_owned())?;
    if !archive.status.success() {
        return Err("pinned transparency commit archive is unavailable".into());
    }
    unpack_pinned_archive(archive.stdout, pin)
}

fn take_option(args: &[String], name: &str) -> Result<Option<String>, String> {
    let positions = args
        .iter()
        .enumerate()
        .filter(|(_, value)| value.as_str() == name)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match positions.as_slice() {
        [] => Ok(None),
        [index] if index + 1 < args.len() => Ok(Some(args[index + 1].clone())),
        [_] => Err(format!("{name} requires a value")),
        _ => Err(format!("{name} may be supplied only once")),
    }
}

fn safe_coordinate(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn verification_entry(output: &[u8], expected_version: &str) -> Result<VerifiedV2Release, String> {
    if output.len() > MAX_VERIFIER_OUTPUT {
        return Err("transparency verifier output exceeds 1 MiB".into());
    }
    let verified = serde_json::from_slice::<VerificationOutput>(output)
        .map_err(|_| "transparency verifier returned malformed JSON".to_owned())?;
    let expected_path = format!("software/journal/{expected_version}/release-record.json");
    if !verified.ok
        || verified.product != "journal"
        || verified.version != expected_version
        || verified.record_path != expected_path
        || !lowercase_hex(&verified.record_sha256, 64)
    {
        return Err("transparency verifier returned a mismatched release identity".into());
    }
    Ok(VerifiedV2Release {
        version: verified.version,
        logical_target: verified.record_path,
        record_sha256: verified.record_sha256,
    })
}

fn validate_registry(registry: &V2ReleaseRegistry) -> Result<(), String> {
    if registry.schema != REGISTRY_SCHEMA {
        return Err("v2 origin registry has an unsupported schema".into());
    }
    let mut versions = BTreeSet::new();
    for entry in &registry.releases {
        let expected = format!("software/journal/{}/release-record.json", entry.version);
        if !safe_coordinate(&entry.version)
            || entry.logical_target != expected
            || !lowercase_hex(&entry.record_sha256, 64)
            || !versions.insert(entry.version.clone())
        {
            return Err(format!(
                "v2 origin registry has an invalid or duplicate entry for {}",
                entry.version
            ));
        }
    }
    Ok(())
}

fn record_registry(
    path: &Path,
    lock_path: &Path,
    verified: VerifiedV2Release,
) -> Result<bool, String> {
    let _lock = hold_lock(lock_path, LockOptions::default())
        .map_err(|error| format!("cannot lock v2 origin registry: {error}"))?;
    let bytes = std::fs::read(path)
        .map_err(|_| format!("cannot read v2 origin registry {}", path.display()))?;
    let mut registry = serde_json::from_slice::<V2ReleaseRegistry>(&bytes)
        .map_err(|_| format!("cannot parse v2 origin registry {}", path.display()))?;
    validate_registry(&registry)?;
    validate_registry(&V2ReleaseRegistry {
        schema: REGISTRY_SCHEMA.to_owned(),
        releases: vec![verified.clone()],
    })?;
    if let Some(existing) = registry
        .releases
        .iter()
        .find(|entry| entry.version == verified.version)
    {
        if existing == &verified {
            return Ok(false);
        }
        return Err(format!(
            "v2 origin registry already contains a different entry for {}",
            verified.version
        ));
    }
    registry.releases.push(verified);
    registry
        .releases
        .sort_by(|left, right| left.version.cmp(&right.version));
    let mut bytes = serde_json::to_vec_pretty(&registry)
        .map_err(|_| "cannot serialize v2 origin registry".to_owned())?;
    bytes.push(b'\n');
    atomic_replace(path, &bytes, AtomicWriteOptions { mode: Some(0o644) })
        .map_err(|error| format!("cannot atomically replace v2 origin registry: {error}"))?;
    Ok(true)
}

fn registry_path(start: &Path) -> Result<PathBuf, String> {
    let inventory = repository_inventory_path(start).ok_or_else(|| {
        format!(
            "could not find core/distribution/inventory.toml from {}",
            start.display()
        )
    })?;
    let core = inventory
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "distribution inventory has no core directory".to_owned())?;
    Ok(core.join("crates/solstone-core-origin/v2-release-registry.json"))
}

fn registry_lock_path(start: &Path) -> Result<PathBuf, String> {
    let raw = git_output(
        start,
        &[
            "rev-parse",
            "--git-path",
            "solstone-release-locks/v2-origin-registry",
        ],
    )?;
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() {
        path
    } else {
        start.join(path)
    };
    let parent = path
        .parent()
        .ok_or_else(|| "v2 origin registry lock has no parent directory".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|_| "cannot create the v2 origin registry lock directory".to_owned())?;
    Ok(path)
}

fn run_adapter(args: &[String]) -> Result<u8, String> {
    let separator = args
        .iter()
        .position(|value| value == "--")
        .ok_or_else(|| "journal-artifacts requires -- before adapter arguments".to_owned())?;
    let control = &args[..separator];
    if control.len() != 2 || control[0] != "--transparency-repo" {
        return Err(usage().to_owned());
    }
    let repo = PathBuf::from(&control[1]);
    let pin = load_pin()?;
    let snapshot = materialize_pinned_checkout(&repo, &pin)?;
    let status = Command::new("bun")
        .arg(
            snapshot
                .path()
                .join(&pin.executables.journal_artifacts.path),
        )
        .args(&args[separator + 1..])
        .status()
        .map_err(|_| "could not execute the pinned journal artifact adapter".to_owned())?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(2))
}

fn run_register(args: &[String]) -> Result<u8, String> {
    let allowed = [
        "--transparency-repo",
        "--root",
        "--version",
        "--store",
        "--metadata-base",
        "--targets-base",
    ];
    let mut consumed = BTreeSet::new();
    for name in allowed {
        if let Some(position) = args.iter().position(|value| value == name) {
            consumed.insert(position);
            consumed.insert(position + 1);
        }
    }
    let apply_positions = args
        .iter()
        .enumerate()
        .filter(|(_, value)| value.as_str() == "--apply")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if apply_positions.len() != 1 {
        return Err("register-v2-origin requires exactly one --apply".into());
    }
    consumed.insert(apply_positions[0]);
    if consumed.len() != args.len() {
        return Err(usage().to_owned());
    }
    let repo = PathBuf::from(
        take_option(args, "--transparency-repo")?
            .ok_or_else(|| "--transparency-repo is required".to_owned())?,
    );
    let root = take_option(args, "--root")?.ok_or_else(|| "--root is required".to_owned())?;
    let version =
        take_option(args, "--version")?.ok_or_else(|| "--version is required".to_owned())?;
    let store = take_option(args, "--store")?.ok_or_else(|| "--store is required".to_owned())?;
    if !safe_coordinate(&version) {
        return Err("--version is not a safe release coordinate".into());
    }
    let pin = load_pin()?;
    let snapshot = materialize_pinned_checkout(&repo, &pin)?;
    let mut command = Command::new("bun");
    command
        .arg(snapshot.path().join(&pin.executables.verify_release.path))
        .args([
            "--root",
            &root,
            "--product",
            "journal",
            "--version",
            &version,
            "--store",
            &store,
            "--json",
        ])
        .stdin(Stdio::null());
    if let Some(value) = take_option(args, "--metadata-base")? {
        command.args(["--metadata-base", &value]);
    }
    if let Some(value) = take_option(args, "--targets-base")? {
        command.args(["--targets-base", &value]);
    }
    let output = command
        .output()
        .map_err(|_| "could not execute the pinned transparency verifier".to_owned())?;
    if !output.status.success() {
        return Err("the pinned transparency verifier rejected this release".into());
    }
    let verified = verification_entry(&output.stdout, &version)?;
    let start = std::env::current_dir()
        .map_err(|_| "cannot determine the journal checkout directory".to_owned())?;
    let registry = registry_path(&start)?;
    let lock = registry_lock_path(&start)?;
    let changed = record_registry(&registry, &lock, verified)?;
    println!(
        "v2 origin registry {} for the journal release {version}",
        if changed {
            "updated"
        } else {
            "already matched"
        }
    );
    Ok(0)
}

pub fn run_cli(command: &str, args: &[String]) -> Result<u8, String> {
    match command {
        "journal-artifacts" => run_adapter(args),
        "register-v2-origin" => run_register(args),
        _ => Err(usage().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_pin() -> ToolPin {
        ToolPin {
            schema: PIN_SCHEMA.to_owned(),
            repository: "https://github.com/solpbc/solstone-transparency.git".to_owned(),
            version: "fixture".to_owned(),
            commit: "a".repeat(40),
            executables: Executables {
                journal_artifacts: ExecutablePin {
                    path: "bin/adapter.ts".to_owned(),
                    sha256: sha256_hex(b"adapter\n"),
                },
                verify_release: ExecutablePin {
                    path: "bin/verifier.ts".to_owned(),
                    sha256: sha256_hex(b"verifier\n"),
                },
            },
        }
    }

    fn fixture_archive() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut bytes);
            for (path, body) in [
                ("bin/adapter.ts", b"adapter\n".as_slice()),
                ("bin/verifier.ts", b"verifier\n".as_slice()),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder.append_data(&mut header, path, body).unwrap();
            }
            builder.finish().unwrap();
        }
        bytes
    }

    #[test]
    fn committed_pin_has_exact_schema_and_distinct_safe_executable_paths() {
        let pin = load_pin().unwrap();
        assert_eq!(pin.version, "0.1.0");
        assert_eq!(pin.commit.len(), 40);
        assert_ne!(
            pin.executables.journal_artifacts.path,
            pin.executables.verify_release.path
        );
    }

    #[test]
    fn every_git_invocation_disables_replacement_objects() {
        let command = git_command(Path::new("."));
        assert!(command.get_envs().any(|(key, value)| {
            key == "GIT_NO_REPLACE_OBJECTS" && value.is_some_and(|value| value == "1")
        }));
    }

    #[test]
    fn pinned_archive_excludes_checkout_only_content_and_is_immutable_after_unpack() {
        let pin = fixture_pin();
        let snapshot = unpack_pinned_archive(fixture_archive(), &pin).unwrap();
        assert!(!snapshot.path().join("bunfig.toml").exists());
        let checkout = tempfile::tempdir().unwrap();
        std::fs::write(checkout.path().join("adapter.ts"), b"changed\n").unwrap();
        assert_eq!(
            std::fs::read(snapshot.path().join("bin/adapter.ts")).unwrap(),
            b"adapter\n"
        );
    }

    #[test]
    fn pinned_identity_refuses_wrong_commit_dirty_tree_and_executable_digest() {
        let mut pin = fixture_pin();
        assert!(validate_checkout_identity(&"0".repeat(40), "", &pin).is_err());
        assert!(validate_checkout_identity(&pin.commit, " M bin/adapter.ts", &pin).is_err());
        pin.executables.verify_release.sha256 = "0".repeat(64);
        assert!(unpack_pinned_archive(fixture_archive(), &pin).is_err());
    }

    #[test]
    fn verifier_output_retains_authenticated_target_and_record_digest() {
        let output = format!(
            "{{\"ok\":true,\"product\":\"journal\",\"version\":\"2.0.0\",\"recordPath\":\"software/journal/2.0.0/release-record.json\",\"recordSha256\":\"{}\"}}",
            "a".repeat(64)
        );
        assert_eq!(
            verification_entry(output.as_bytes(), "2.0.0").unwrap(),
            VerifiedV2Release {
                version: "2.0.0".into(),
                logical_target: "software/journal/2.0.0/release-record.json".into(),
                record_sha256: "a".repeat(64),
            }
        );
        assert!(verification_entry(output.as_bytes(), "2.0.1").is_err());
    }

    #[test]
    fn registry_writer_is_strict_idempotent_and_conflict_refusing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let lock = dir.path().join("control/registry");
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!("{{\"schema\":\"{REGISTRY_SCHEMA}\",\"releases\":[]}}"),
        )
        .unwrap();
        let entry = VerifiedV2Release {
            version: "2.0.0".into(),
            logical_target: "software/journal/2.0.0/release-record.json".into(),
            record_sha256: "a".repeat(64),
        };
        assert!(record_registry(&path, &lock, entry.clone()).unwrap());
        assert!(!record_registry(&path, &lock, entry.clone()).unwrap());
        let mut conflict = entry;
        conflict.record_sha256 = "b".repeat(64);
        assert!(record_registry(&path, &lock, conflict).is_err());
        assert!(!path.with_extension("json.lock").exists());
    }

    #[test]
    fn concurrent_registry_writers_retain_both_releases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let lock = dir.path().join("control/registry");
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!("{{\"schema\":\"{REGISTRY_SCHEMA}\",\"releases\":[]}}"),
        )
        .unwrap();
        let writers = ["2.0.0", "2.0.1"].map(|version| {
            let path = path.clone();
            let lock = lock.clone();
            std::thread::spawn(move || {
                record_registry(
                    &path,
                    &lock,
                    VerifiedV2Release {
                        version: version.to_owned(),
                        logical_target: format!("software/journal/{version}/release-record.json"),
                        record_sha256: "a".repeat(64),
                    },
                )
                .unwrap();
            })
        });
        for writer in writers {
            writer.join().unwrap();
        }
        let registry: V2ReleaseRegistry =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            registry
                .releases
                .into_iter()
                .map(|entry| entry.version)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["2.0.0".to_owned(), "2.0.1".to_owned()])
        );
    }
}
