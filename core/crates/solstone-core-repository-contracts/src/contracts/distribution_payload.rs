// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::payload_inventory::{declared_paths, is_payload_path, payload_src_root};

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
    // payload.txt is `payload_src_root`-relative, so confinement is checked
    // against that root rather than against the repository root.
    let payload_root = root.join(
        payload_src_root(
            &fs::read_to_string(root.join("core/distribution/inventory.toml"))
                .expect("read distribution inventory"),
        )
        .expect("inventory declares payload_src_root"),
    );
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
            .all(|path| is_confined_regular_file(&payload_root, path)),
        "every declared payload path must exist as a regular, non-symlink file under {}",
        payload_root.display()
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

/// The producer reads the payload root from the inventory and the runtime
/// resolver carries it as a constant. They describe the same directory, so a
/// change to either without the other is drift that no other gate would see:
/// the producer would keep staging from a directory the binary no longer reads.
#[test]
fn declared_payload_root_matches_the_resolver_constant() {
    let root = repository_root();
    let declared = payload_src_root(
        &fs::read_to_string(root.join("core/distribution/inventory.toml"))
            .expect("read distribution inventory"),
    )
    .expect("inventory declares payload_src_root");
    assert_eq!(
        declared,
        solstone_core_journal::CHECKOUT_PAYLOAD_ROOT,
        "core/distribution/inventory.toml payload_src_root and \
         solstone_core_journal::CHECKOUT_PAYLOAD_ROOT must name the same directory"
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
