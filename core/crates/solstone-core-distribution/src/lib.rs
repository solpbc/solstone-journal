// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub mod archive;
pub mod inventory;
pub mod select;

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
