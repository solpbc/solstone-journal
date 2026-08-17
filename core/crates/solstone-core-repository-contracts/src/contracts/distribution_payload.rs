// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::payload_inventory::{declared_paths, is_payload_path};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn is_confined_regular_file(root: &Path, relative: &str) -> bool {
    let mut candidate = root.to_path_buf();
    let mut metadata = None;
    for component in Path::new(relative).components() {
        let Component::Normal(part) = component else {
            return false;
        };
        candidate.push(part);
        let Ok(current) = fs::symlink_metadata(&candidate) else {
            return false;
        };
        if current.file_type().is_symlink() {
            return false;
        }
        metadata = Some(current);
    }
    metadata.is_some_and(|metadata| metadata.file_type().is_file())
}

#[test]
fn payload_txt_is_confined_and_present() {
    let root = repository_root();
    let listed = declared_paths(
        &fs::read_to_string(root.join("core/distribution/payload.txt")).expect("read payload.txt"),
    );
    assert!(!listed.is_empty(), "payload declaration must not be empty");
    assert_eq!(
        listed.len(),
        listed.iter().collect::<BTreeSet<_>>().len(),
        "payload declaration must not contain duplicate paths"
    );
    assert!(
        listed.iter().all(|path| is_payload_path(path.as_bytes())),
        "payload declaration contains a path outside the allowed payload roots"
    );
    assert!(
        listed
            .iter()
            .all(|path| is_confined_regular_file(&root, path)),
        "every declared payload path must exist as a regular, non-symlink file"
    );
    assert!(
        listed
            .iter()
            .any(|path| path == "solstone/talent/daily_schedule.md"),
        "payload declaration must include a known talent positive"
    );
    assert!(
        listed
            .iter()
            .any(|path| path == "solstone/think/contract/layout.json"),
        "payload declaration must include the layout contract anchor"
    );
}

#[cfg(unix)]
#[test]
fn payload_file_check_refuses_symlinks() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().expect("create payload file fixture");
    let root = temporary.path().join("root");
    let regular = root.join("regular");
    let linked = root.join("linked");
    let outside = temporary.path().join("outside");
    let linked_directory = root.join("linked-directory");
    fs::create_dir(&root).expect("create payload root fixture");
    fs::write(&regular, b"payload").expect("write regular payload fixture");
    symlink(&regular, &linked).expect("create payload symlink fixture");
    fs::create_dir(&outside).expect("create outside fixture directory");
    fs::write(outside.join("payload"), b"payload").expect("write outside payload fixture");
    symlink(&outside, &linked_directory).expect("create payload directory symlink fixture");

    assert!(is_confined_regular_file(&root, "regular"));
    assert!(!is_confined_regular_file(&root, "linked"));
    assert!(!is_confined_regular_file(&root, "linked-directory/payload"));
}
