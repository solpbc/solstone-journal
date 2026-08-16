// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub mod ar;
pub mod archive;
pub mod deb;
pub mod inventory;
pub mod rpm;
pub mod select;
pub mod stage;
pub mod tar;

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use inventory::{
    Inventory, InventoryError, load_inventory, load_payload, repository_inventory_path,
};

pub fn validate_distribution_inventory(inventory_path: &Path) -> Result<Inventory, InventoryError> {
    let inventory = load_inventory(inventory_path)?;
    let _payload = load_payload(inventory_path, &inventory)?;
    Ok(inventory)
}

pub fn discover_and_validate_inventory(start: &Path) -> Result<Inventory, InventoryError> {
    let path = repository_inventory_path(start).ok_or_else(|| {
        InventoryError::new(format!(
            "could not find core/distribution/inventory.toml from {}",
            start.display()
        ))
    })?;
    validate_distribution_inventory(&path)
}

pub fn write_containers(stage: &Path, out_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(out_dir)?;
    tar::write_tar_gz(stage, &out_dir.join("tree.tar.gz"))?;
    deb::write_deb(stage, &out_dir.join("tree.deb"))?;
    rpm::write_rpm(stage, &out_dir.join("tree.rpm"))?;
    Ok(())
}

pub fn compare_manifests(
    _left_label: &str,
    left: &[String],
    right_label: &str,
    right: &[String],
) -> Result<(), String> {
    let left = left.iter().cloned().collect::<BTreeSet<_>>();
    let right = right.iter().cloned().collect::<BTreeSet<_>>();
    let missing = left.difference(&right).cloned().collect::<BTreeSet<_>>();
    let unexpected = right.difference(&left).cloned().collect::<BTreeSet<_>>();
    if missing.is_empty() && unexpected.is_empty() {
        return Ok(());
    }
    let mut sections = Vec::new();
    if !missing.is_empty() {
        sections.push(inventory::format_named_list(
            &format!("missing in {right_label}"),
            &missing,
        ));
    }
    if !unexpected.is_empty() {
        sections.push(inventory::format_named_list(
            &format!("unexpected in {right_label}"),
            &unexpected,
        ));
    }
    Err(sections.join("\n"))
}

#[cfg(test)]
#[test]
fn selection_from_default_cargo_output_names_missing_required_and_admitted_forbidden() {
    use std::fs;
    use std::path::{Path, PathBuf};

    let inventory_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../core/distribution/inventory.toml");
    let inventory = load_inventory(&inventory_path).expect("committed inventory must parse");
    let output = PathBuf::from("/var/tmp/solstone-distribution-default-host-output");
    let _ = fs::remove_dir_all(&output);
    fs::create_dir_all(&output).expect("create fixture dir");

    // Model Makefile RUST_HOST_EXCLUDES: speakers-analyze / speakers-onnx /
    // vad-analyze packages are absent from a default host build.
    const HOST_EXCLUDED: &str = "solstone-core-speakers-analyze";
    const HOST_EXCLUDED_DENIED: &str = "solstone-core-vad-analyze";
    for name in inventory.required_bins() {
        if name != HOST_EXCLUDED {
            fs::write(output.join(&name), []).expect("write required fixture");
        }
    }
    for name in inventory.forbidden_bins() {
        if name != HOST_EXCLUDED_DENIED {
            fs::write(output.join(&name), []).expect("write denied fixture");
        }
    }

    let selection = select::select_from_directory(&inventory, &output).unwrap_or_else(|error| {
        let _ = fs::remove_dir_all(&output);
        panic!("{error}");
    });
    fs::remove_dir_all(&output).expect("cleanup fixture");
    assert!(
        selection
            .admitted
            .contains("solstone-core-speakers-analyze"),
        "missing required:\n  solstone-core-speakers-analyze"
    );
    assert!(
        !selection.admitted.contains("setup-fixture-journal"),
        "admitted forbidden:\n  setup-fixture-journal"
    );
}

#[cfg(test)]
#[test]
fn containers_disagree_on_required_entry() {
    use std::path::PathBuf;

    let root = PathBuf::from("/var/tmp/solstone-distribution-container-stage");
    let out = PathBuf::from("/var/tmp/solstone-distribution-container-out");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&root).expect("create stage");

    stage::write_staged_file(&root, "bin/solstone-core-speakers-analyze", b"helper")
        .expect("stage helper");
    stage::write_staged_file(
        &root,
        "share/solstone/talent/journal/contract/bundle.json",
        b"{}",
    )
    .expect("stage contract bundle");

    write_containers(&root, &out).expect("write containers");
    let tar_manifest = tar::list_tar_gz(&out.join("tree.tar.gz")).expect("list tar");
    let deb_manifest = deb::list_deb(&out.join("tree.deb")).expect("list deb");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&out);
    compare_manifests("tar", &tar_manifest, "deb", &deb_manifest).unwrap_or_else(|error| {
        panic!("{error}");
    });
}
