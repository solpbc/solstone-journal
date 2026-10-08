// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse HEAD must succeed");
    if !output.status.success() {
        panic!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let commit = String::from_utf8(output.stdout)
        .expect("commit is UTF-8")
        .trim()
        .to_owned();
    if commit.is_empty() {
        panic!("git rev-parse HEAD produced an empty commit");
    }
    println!("cargo:rustc-env=SOLSTONE_DISTRIBUTION_SOURCE_COMMIT={commit}");

    // Worktree support: git rev-parse --git-path HEAD resolves worktree HEAD
    let head_path_output = Command::new("git")
        .args(["rev-parse", "--git-path", "HEAD"])
        .output()
        .expect("git rev-parse --git-path HEAD must succeed");
    if !head_path_output.status.success() {
        panic!(
            "git rev-parse --git-path HEAD failed: {}",
            String::from_utf8_lossy(&head_path_output.stderr)
        );
    }
    let head_path_str = String::from_utf8(head_path_output.stdout)
        .expect("path is UTF-8")
        .trim()
        .to_owned();
    let head_path = PathBuf::from(&head_path_str);
    let head_path_abs = if head_path.is_absolute() {
        head_path
    } else {
        std::env::current_dir().expect("cwd").join(head_path)
    };
    println!("cargo:rerun-if-changed={}", head_path_abs.display());

    let head_content = fs::read_to_string(&head_path_abs).expect("read HEAD file");
    let trimmed = head_content.trim();
    if let Some(ref_name) = trimmed.strip_prefix("ref: ") {
        let ref_output = Command::new("git")
            .args(["rev-parse", "--git-path", ref_name.trim()])
            .output()
            .expect("git rev-parse --git-path <ref> must succeed");
        if !ref_output.status.success() {
            panic!(
                "git rev-parse --git-path {} failed: {}",
                ref_name.trim(),
                String::from_utf8_lossy(&ref_output.stderr)
            );
        }
        let ref_path_str = String::from_utf8(ref_output.stdout)
            .expect("ref path is UTF-8")
            .trim()
            .to_owned();
        let ref_path = PathBuf::from(&ref_path_str);
        let ref_path_abs = if ref_path.is_absolute() {
            ref_path
        } else {
            std::env::current_dir().expect("cwd").join(ref_path)
        };
        println!("cargo:rerun-if-changed={}", ref_path_abs.display());
    }
}
