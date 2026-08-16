// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Time-boxed byte-identity gate for BK-1 crate copies of `solstone/` files.
//! The deletion lode removes this module when Python is cut.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

const FILE_PAIRS: &[(&str, &str, &str)] = &[
    (
        "solstone/convey/templates/init.html",
        "core/crates/solstone-core-sol-link/assets/init.html",
        "the native link setup page embeds this HTML",
    ),
    (
        "solstone/observe/describe.md",
        "core/crates/solstone-core-describe/assets/describe.md",
        "native describe embeds this prompt",
    ),
    (
        "solstone/observe/describe.schema.json",
        "core/crates/solstone-core-describe/assets/describe.schema.json",
        "native describe embeds this schema",
    ),
    (
        "solstone/observe/extract.md",
        "core/crates/solstone-core-describe/assets/extract.md",
        "native describe embeds this prompt",
    ),
    (
        "solstone/observe/extract.schema.json",
        "core/crates/solstone-core-describe/assets/extract.schema.json",
        "native describe embeds this schema",
    ),
    (
        "solstone/apps/settings/install_copy.py",
        "core/crates/solstone-core-settings-web/assets/install_copy.py",
        "settings-web build.rs parses install copy constants",
    ),
    (
        "solstone/apps/chat/copy.py",
        "core/crates/solstone-core-settings-web/assets/chat_copy.py",
        "settings-web build.rs parses chat copy constants",
    ),
    (
        "solstone/convey/sol_initiated/copy.py",
        "core/crates/solstone-core-settings-web/assets/sol_initiated_copy.py",
        "settings-web build.rs parses sol-initiated copy constants",
    ),
    (
        "solstone/apps/backup/copy.py",
        "core/crates/solstone-core-settings-web/assets/backup_copy.py",
        "settings-web build.rs parses backup copy constants",
    ),
    (
        "solstone/think/activities.py",
        "core/crates/solstone-core-settings-web/assets/activities.py",
        "settings-web build.rs parses DEFAULT_ACTIVITIES",
    ),
    (
        "solstone/think/link/mark_assets/glyphs.json",
        "core/crates/solstone-core-sol-link/assets/mark_assets/glyphs.json",
        "native link mark derivation embeds these JSON tables",
    ),
    (
        "solstone/think/link/mark_assets/colors.json",
        "core/crates/solstone-core-sol-link/assets/mark_assets/colors.json",
        "native link mark derivation embeds these JSON tables",
    ),
    (
        "solstone/think/link/mark_assets/words.json",
        "core/crates/solstone-core-sol-link/assets/mark_assets/words.json",
        "native link mark derivation embeds these JSON tables",
    ),
    (
        "solstone/apps/network/copy.py",
        "core/crates/solstone-core-convey-shell/assets/network_copy.py",
        "convey-shell build.rs parses network copy constants",
    ),
    (
        "solstone/think/services/outcomes.py",
        "core/crates/solstone-core-convey-shell/assets/outcomes.py",
        "convey-shell build.rs parses SPL outcome strings",
    ),
    (
        "solstone/think/pairing/config.py",
        "core/crates/solstone-core-convey-shell/assets/pairing_config.py",
        "convey-shell build.rs parses home-address copy constants",
    ),
];

const DIR_PAIRS: &[(&str, &str, &str, Option<&str>)] = &[
    (
        "solstone/convey/static",
        "core/crates/solstone-core-convey-shell/assets/static",
        "native convey-shell embeds the Flask static tree",
        None,
    ),
    (
        "solstone/observe/categories",
        "core/crates/solstone-core-describe-categories/assets/categories",
        "native describe-categories embeds category prompts and schemas",
        Some("md,schema.json"),
    ),
];

fn mismatch(source: &str, dest: &str, why: &str, detail: &str) -> String {
    format!(
        "{detail}\n\n\
         Source: {source}\n\
         Crate copy: {dest}\n\n\
         Why: {why} so cargo can compile without reading solstone/. \
This copy disappears when Python is deleted.\n\n\
         Repair with:\n  cp {source} {dest}\n"
    )
}

fn dir_mismatch(
    source_dir: &str,
    dest_dir: &str,
    why: &str,
    missing: &[String],
    extra: &[String],
    changed: &[String],
) -> String {
    let mut lines = Vec::new();
    if !missing.is_empty() {
        lines.push(format!("Missing from crate copy: {}", missing.join(", ")));
    }
    if !extra.is_empty() {
        lines.push(format!("Extra in crate copy: {}", extra.join(", ")));
    }
    if !changed.is_empty() {
        lines.push(format!("Bytes differ: {}", changed.join(", ")));
    }
    format!(
        "{}\n\n\
         Source dir: {source_dir}\n\
         Crate copy dir: {dest_dir}\n\n\
         Why: {why} so cargo can compile without reading solstone/. \
This copy disappears when Python is deleted.\n\n\
         Repair with:\n  cp -a {source_dir}/. {dest_dir}/\n",
        lines.join("\n")
    )
}

fn list_rel_files(root: &Path, filter: Option<&str>) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    walk(root, root, filter, &mut files);
    files
}

fn walk(root: &Path, dir: &Path, filter: Option<&str>, files: &mut BTreeSet<String>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|error| panic!("cannot read {}: {error}", dir.display()));
    for entry in entries {
        let entry = entry
            .unwrap_or_else(|error| panic!("cannot read entry under {}: {error}", dir.display()));
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, filter, files);
            continue;
        }
        if !path.is_file() && !path.is_symlink() {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .expect("walked file is under root")
            .to_string_lossy()
            .replace('\\', "/");
        if let Some(filter) = filter {
            let allowed = filter.split(',').any(|suffix| {
                if suffix == "md" {
                    rel.ends_with(".md")
                } else {
                    rel.ends_with(&format!(".{suffix}"))
                }
            });
            if !allowed {
                continue;
            }
        }
        files.insert(rel);
    }
}

fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|error| panic!("{} is unreadable: {error}", path.display()))
}

#[test]
fn copied_solstone_compile_inputs_match_byte_for_byte() {
    let root = repository_root();
    for (source, dest, why) in FILE_PAIRS {
        let src_path = root.join(source);
        let dst_path = root.join(dest);
        assert!(
            src_path.is_file() || src_path.is_symlink(),
            "{}",
            mismatch(source, dest, why, "source file is missing")
        );
        assert!(
            dst_path.is_file(),
            "{}",
            mismatch(source, dest, why, "crate copy is missing")
        );
        assert!(
            read_bytes(&src_path) == read_bytes(&dst_path),
            "{}",
            mismatch(source, dest, why, "file bytes have diverged")
        );
    }
    for (source_dir, dest_dir, why, filter) in DIR_PAIRS {
        let src_root = root.join(source_dir);
        let dst_root = root.join(dest_dir);
        assert!(
            src_root.is_dir(),
            "{}",
            dir_mismatch(source_dir, dest_dir, why, &[], &[], &[])
        );
        assert!(
            dst_root.is_dir(),
            "{}",
            dir_mismatch(source_dir, dest_dir, why, &[], &[], &[])
        );
        let src_files = list_rel_files(&src_root, *filter);
        let dst_files = list_rel_files(&dst_root, *filter);
        let missing: Vec<String> = src_files.difference(&dst_files).cloned().collect();
        let extra: Vec<String> = dst_files.difference(&src_files).cloned().collect();
        let mut changed = Vec::new();
        for rel in src_files.intersection(&dst_files) {
            if read_bytes(&src_root.join(rel)) != read_bytes(&dst_root.join(rel)) {
                changed.push(rel.clone());
            }
        }
        assert!(
            missing.is_empty() && extra.is_empty() && changed.is_empty(),
            "{}",
            dir_mismatch(source_dir, dest_dir, why, &missing, &extra, &changed)
        );
    }
}
