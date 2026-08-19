// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core-sol")
}

fn temp_path(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    env::temp_dir().join(format!("solstone-core-skills-{name}-{stamp}"))
}

fn source_checkout_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("workspace checkout root")
        .to_path_buf()
}

fn run_sol(program: &Path, cwd: &Path, home: &Path, args: &[&str]) -> Output {
    for _ in 0..100 {
        match Command::new(program)
            .args(args)
            .current_dir(cwd)
            .env("HOME", home)
            .output()
        {
            Ok(output) => return output,
            Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => {
                sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("fake solstone-core should execute: {error:?}"),
        }
    }
    panic!(
        "fake solstone-core stayed busy after retries: {}",
        program.display()
    )
}

fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create copy destination");
    for entry in fs::read_dir(src).expect("read copy source") {
        let entry = entry.expect("read copy entry");
        let source = entry.path();
        let target = dst.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source).expect("copy source metadata");
        if metadata.is_dir() {
            copy_tree(&source, &target);
        } else if metadata.is_file() {
            fs::copy(&source, &target).expect("copy file");
        }
    }
}

#[test]
fn sol_skills_uses_relocated_installed_payload_root() {
    let temp = temp_path("relocated-installed-layout");
    let staged_root = temp.join("staged");
    let moved_root = temp.join("moved");
    let bin_dir = staged_root.join("bin");
    let site_packages = staged_root
        .join("lib")
        .join("python3.13")
        .join("site-packages");
    fs::create_dir_all(&bin_dir).expect("create staged bin dir");
    fs::create_dir_all(site_packages.join("solstone")).expect("create staged package dir");
    fs::write(site_packages.join("solstone").join("__init__.py"), "").expect("write init");
    copy_tree(
        &source_checkout_root().join("core/payload/solstone/talent"),
        &site_packages.join("solstone/talent"),
    );

    let staged_binary = bin_dir.join("solstone-core");
    fs::copy(bin(), &staged_binary).expect("copy solstone-core binary into fake layout");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged_binary, fs::Permissions::from_mode(0o755))
            .expect("make fake solstone-core executable");
    }

    fs::rename(&staged_root, &moved_root).expect("move fake installed tree");
    let moved_binary = moved_root.join("bin").join("solstone-core");
    let moved_site_packages = moved_root
        .join("lib")
        .join("python3.13")
        .join("site-packages");
    let unrelated_cwd = temp.join("outside-checkout");
    let home = temp.join("home");
    fs::create_dir_all(&unrelated_cwd).expect("create unrelated cwd");
    fs::create_dir_all(&home).expect("create isolated home");

    let root = run_sol(&moved_binary, &unrelated_cwd, &home, &["root"]);

    assert_eq!(root.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(root.stdout).expect("root stdout should be utf-8"),
        format!("{}\n", moved_site_packages.display())
    );
    assert_eq!(
        String::from_utf8(root.stderr).expect("root stderr should be utf-8"),
        ""
    );

    fs::remove_dir_all(moved_site_packages.join("solstone/talent/solstone"))
        .expect("remove moved solstone payload");
    let missing_payload = run_sol(&moved_binary, &unrelated_cwd, &home, &["skills", "list"]);

    assert_ne!(missing_payload.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(missing_payload.stdout).expect("skills stdout should be utf-8"),
        ""
    );
    assert_eq!(
        String::from_utf8(missing_payload.stderr).expect("skills stderr should be utf-8"),
        format!(
            "error: expected bundled umbrella skill at solstone/talent/solstone/SKILL.md ({})\n",
            moved_site_packages
                .join("solstone/talent/solstone/SKILL.md")
                .display()
        )
    );
    fs::remove_dir_all(temp).expect("cleanup relocated installed layout");
}
