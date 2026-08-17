// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

const INVENTORY: &str = "core/distribution/inventory.toml";
const INSTALL_SH: &str = "core/distribution/install.sh";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn format_named_list(label: &str, names: &BTreeSet<String>) -> String {
    let mut lines = vec![format!("{label}:")];
    for name in names {
        lines.push(format!("  {name}"));
    }
    lines.join("\n")
}

fn quoted_assignment<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix(key) else {
            continue;
        };
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('=').unwrap_or(rest).trim();
        return Some(rest.trim_matches('"'));
    }
    None
}

fn inventory_product(text: &str) -> Option<&str> {
    quoted_assignment(text, "product")
}

fn inventory_basename_template(text: &str) -> Option<&str> {
    let mut in_artifact = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_artifact = trimmed == "[artifact]";
            continue;
        }
        if in_artifact && let Some(value) = trimmed.strip_prefix("basename") {
            let value = value.trim().trim_start_matches('=').trim();
            return Some(value.trim_matches('"'));
        }
    }
    None
}

fn inventory_targets(text: &str) -> Vec<(String, String)> {
    let mut targets = Vec::new();
    let mut in_target = false;
    let mut id = None;
    let mut arch = None;
    let flush = |targets: &mut Vec<(String, String)>,
                 id: &mut Option<String>,
                 arch: &mut Option<String>| {
        if let (Some(id), Some(arch)) = (id.take(), arch.take()) {
            targets.push((id, arch));
        } else {
            id.take();
            arch.take();
        }
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_target {
                flush(&mut targets, &mut id, &mut arch);
            }
            in_target = trimmed == "[[target]]";
            continue;
        }
        if !in_target {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("id = ") {
            id = Some(value.trim().trim_matches('"').to_owned());
        }
        if let Some(value) = trimmed.strip_prefix("arch = ") {
            arch = Some(value.trim().trim_matches('"').to_owned());
        }
    }
    if in_target {
        flush(&mut targets, &mut id, &mut arch);
    }
    targets
}

fn install_product(text: &str) -> Option<&str> {
    quoted_assignment(text, "PRODUCT")
}

fn install_base_formula(text: &str) -> Option<&str> {
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("_base=") else {
            continue;
        };
        return Some(rest);
    }
    None
}

fn install_targets(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in text.lines() {
        let Some((_, rest)) = line.split_once("TARGET=") else {
            continue;
        };
        let value = rest
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches(';');
        if !value.is_empty() {
            names.insert(value.to_owned());
        }
    }
    names
}

fn render_template(template: &str, version: &str, arch: &str) -> String {
    template
        .replace("{version}", version)
        .replace("{arch}", arch)
}

fn install_basename(product: &str, version: &str, target: &str) -> String {
    format!("{product}-{version}-{target}")
}

fn drift(inventory: &str, install: &str) -> BTreeSet<String> {
    let mut unexpected = BTreeSet::new();
    let Some(product) = inventory_product(inventory) else {
        unexpected.insert("inventory product".to_owned());
        return unexpected;
    };
    let Some(template) = inventory_basename_template(inventory) else {
        unexpected.insert("inventory basename".to_owned());
        return unexpected;
    };
    let Some(install_product) = install_product(install) else {
        unexpected.insert("install PRODUCT".to_owned());
        return unexpected;
    };
    if product != install_product {
        unexpected.insert(format!("product {product} {install_product}"));
    }
    if install_base_formula(install) != Some("${PRODUCT}-${VERSION}-${TARGET}") {
        unexpected.insert("install _base formula".to_owned());
    }
    let install_targets = install_targets(install);
    let version = "VERSION";
    for (id, arch) in inventory_targets(inventory) {
        if !install_targets.contains(&id) {
            unexpected.insert(format!("install TARGET {id}"));
        }
        let from_inventory = render_template(template, version, &arch);
        let from_install = install_basename(install_product, version, &id);
        if from_inventory != from_install {
            unexpected.insert(format!("{from_inventory} {from_install}"));
        }
    }
    unexpected
}

#[test]
fn install_basename_matches_inventory_template() {
    let root = repository_root();
    let inventory = fs::read_to_string(root.join(INVENTORY)).expect("read inventory");
    let install = fs::read_to_string(root.join(INSTALL_SH)).expect("read install.sh");
    let unexpected = drift(&inventory, &install);
    assert!(
        unexpected.is_empty(),
        "{}",
        format_named_list("unexpected", &unexpected)
    );
}

#[test]
fn planted_basename_mismatch_is_detected() {
    let inventory = "product = \"solstone-journal\"\n[artifact]\nbasename = \"solstone-journal-{version}-linux-{arch}\"\n[[target]]\nid = \"linux-x86_64\"\narch = \"x86_64\"\n";
    let matching =
        "PRODUCT=solstone-journal\nTARGET=linux-x86_64\n_base=${PRODUCT}-${VERSION}-${TARGET}\n";
    assert!(drift(inventory, matching).is_empty());
    let planted =
        "PRODUCT=solstone-other\nTARGET=linux-x86_64\n_base=${PRODUCT}-${VERSION}-${TARGET}\n";
    let unexpected = drift(inventory, planted);
    assert!(
        unexpected
            .iter()
            .any(|item| item.contains("solstone-other")),
        "{unexpected:?}"
    );
}
