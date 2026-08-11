// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_symlink() {
            continue;
        }
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn staged_root() -> TempDir {
    let temp = TempDir::new().unwrap();
    copy_tree(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../solstone")
            .as_path(),
        &temp.path().join("solstone"),
    );
    copy_tree(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/fixtures/journal")
            .as_path(),
        &temp.path().join("tests/fixtures/journal"),
    );
    temp
}

fn command(binary: &Path, cwd: &Path, root: Option<&Path>) -> Command {
    let mut command = Command::new(binary);
    command.current_dir(cwd).env_clear().env("PATH", "");
    command.arg("contract");
    if let Some(root) = root {
        command.args(["build", "--root"]).arg(root);
    } else {
        command.arg("build");
    }
    command
}

#[test]
fn contract_process_isolation_and_missing_layout_diagnostics() {
    let root = staged_root();
    let outside = TempDir::new().unwrap();
    let marker = root.path().join("poisoned");
    let poison_dir = root.path().join(".venv/bin");
    fs::create_dir_all(&poison_dir).unwrap();
    for name in ["python", "python3"] {
        let path = poison_dir.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf poisoned > {}\nexit 97\n",
                marker.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let binary = Path::new(env!("CARGO_BIN_EXE_solstone-core"));
    let build = command(binary, outside.path(), Some(root.path()))
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let actual = fs::read(
        root.path()
            .join("solstone/talent/journal/contract/bundle.json"),
    )
    .unwrap();
    let expected = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../solstone/talent/journal/contract/bundle.json"),
    )
    .unwrap();
    assert_eq!(actual, expected);
    let check = Command::new(binary)
        .current_dir(outside.path())
        .env_clear()
        .env("PATH", "")
        .args(["contract", "check", "--root"])
        .arg(root.path())
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let count = String::from_utf8_lossy(&check.stdout)
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok());
    assert!(count.is_some_and(|count| count > 0));

    let current = command(binary, outside.path(), Some(root.path()))
        .arg("--check")
        .output()
        .unwrap();
    assert!(current.status.success());
    let artifact = root
        .path()
        .join("solstone/talent/journal/contract/bundle.json");
    fs::write(
        &artifact,
        [fs::read(&artifact).unwrap(), b" ".to_vec()].concat(),
    )
    .unwrap();
    let stale = command(binary, outside.path(), Some(root.path()))
        .arg("--check")
        .output()
        .unwrap();
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("is stale"));

    fs::write(&artifact, &expected).unwrap();
    let empty_journal = TempDir::new().unwrap();
    let empty = Command::new(binary)
        .current_dir(outside.path())
        .env_clear()
        .env("PATH", "")
        .args(["contract", "check", "--root"])
        .arg(root.path())
        .args(["--journal"])
        .arg(empty_journal.path())
        .output()
        .unwrap();
    assert!(empty.status.success());
    assert!(String::from_utf8_lossy(&empty.stderr).contains("no contract-covered files found"));
    assert!(!marker.exists());

    fs::remove_file(
        root.path()
            .join("solstone/talent/journal/contract/bundle.json"),
    )
    .unwrap();
    let missing = Command::new(binary)
        .current_dir(outside.path())
        .env_clear()
        .env("PATH", "")
        .args(["contract", "check", "--root"])
        .arg(root.path())
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("bundle artifact missing"));

    let no_fixture = staged_root();
    fs::remove_dir_all(no_fixture.path().join("tests")).unwrap();
    let fixture = Command::new(binary)
        .current_dir(outside.path())
        .env_clear()
        .env("PATH", "")
        .args(["contract", "check", "--root"])
        .arg(no_fixture.path())
        .output()
        .unwrap();
    assert!(!fixture.status.success());
    assert!(String::from_utf8_lossy(&fixture.stderr).contains("journal tree not found"));
}

#[test]
fn copied_binary_without_checkout_root_fails_cleanly() {
    let outside = TempDir::new().unwrap();
    let copied = outside.path().join("solstone-core");
    fs::copy(env!("CARGO_BIN_EXE_solstone-core"), &copied).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&copied, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = command(&copied, outside.path(), None).output().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("could not locate installed solstone package")
    );
}
