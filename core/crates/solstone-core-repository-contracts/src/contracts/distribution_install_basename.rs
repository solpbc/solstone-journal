// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
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

fn inventory_targets(text: &str) -> Vec<(String, String, String)> {
    let mut targets = Vec::new();
    let mut in_target = false;
    let mut id = None;
    let mut os = None;
    let mut arch = None;
    let flush = |targets: &mut Vec<(String, String, String)>,
                 id: &mut Option<String>,
                 os: &mut Option<String>,
                 arch: &mut Option<String>| {
        if let (Some(id), Some(os), Some(arch)) = (id.take(), os.take(), arch.take()) {
            targets.push((id, os, arch));
        } else {
            id.take();
            os.take();
            arch.take();
        }
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_target {
                flush(&mut targets, &mut id, &mut os, &mut arch);
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
        if let Some(value) = trimmed.strip_prefix("os = ") {
            os = Some(value.trim().trim_matches('"').to_owned());
        }
        if let Some(value) = trimmed.strip_prefix("arch = ") {
            arch = Some(value.trim().trim_matches('"').to_owned());
        }
    }
    if in_target {
        flush(&mut targets, &mut id, &mut os, &mut arch);
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

fn install_targets(text: &str) -> Vec<String> {
    let mut names = Vec::new();
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
            names.push(value.to_owned());
        }
    }
    names
}

fn render_template(template: &str, version: &str, os: &str, arch: &str) -> String {
    template
        .replace("{version}", version)
        .replace("{os}", os)
        .replace("{arch}", arch)
}

fn install_basename(product: &str, version: &str, target: &str) -> String {
    format!("{product}-{version}-{target}")
}

fn drift(inventory: &str, install: &str) -> BTreeSet<String> {
    let mut unexpected = BTreeSet::new();
    let inv_product = inventory_product(inventory);
    let template = inventory_basename_template(inventory);
    let inst_product = install_product(install);

    if inv_product.is_none() {
        unexpected.insert("inventory product".to_owned());
    }
    if template.is_none() {
        unexpected.insert("inventory basename".to_owned());
    }
    if inst_product.is_none() {
        unexpected.insert("install PRODUCT".to_owned());
    }
    if let (Some(product), Some(install_product)) = (inv_product, inst_product)
        && product != install_product
    {
        unexpected.insert(format!("product {product} {install_product}"));
    }
    if install_base_formula(install) != Some("${PRODUCT}-${VERSION}-${TARGET}") {
        unexpected.insert("install _base formula".to_owned());
    }

    let targets = inventory_targets(inventory);
    let version = "VERSION";
    if let (Some(template), Some(install_product)) = (template, inst_product) {
        for (id, os, arch) in &targets {
            let from_inventory = render_template(template, version, os, arch);
            let from_install = install_basename(install_product, version, id);
            if from_inventory != from_install {
                unexpected.insert(format!("{from_inventory} {from_install}"));
            }
        }
    }

    let parsed_targets = install_targets(install);
    let mut seen_counts = BTreeMap::new();
    for t in &parsed_targets {
        *seen_counts.entry(t.clone()).or_insert(0) += 1;
    }
    for (t, count) in &seen_counts {
        if *count > 1 {
            unexpected.insert(format!("duplicate TARGET {t}"));
        }
    }

    let parsed_set: BTreeSet<String> = parsed_targets.into_iter().collect();

    // macOS remains an artifact target for the native-app / apple-native lane;
    // Journal bootstrap has no macOS TARGET= arm after 783a431e1;
    // required arm set is inventory os == "linux", exact equality with parsed TARGET= arms.
    let required_linux_targets: BTreeSet<String> = targets
        .into_iter()
        .filter(|(_, os, _)| os == "linux")
        .map(|(id, _, _)| id)
        .collect();

    for id in &required_linux_targets {
        if !parsed_set.contains(id) {
            unexpected.insert(format!("missing required TARGET {id}"));
        }
    }
    for id in &parsed_set {
        if !required_linux_targets.contains(id) {
            unexpected.insert(format!("unexpected TARGET {id}"));
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
    let inventory = "product = \"solstone-journal\"\n[artifact]\nbasename = \"solstone-journal-{version}-{os}-{arch}\"\n[[target]]\nid = \"linux-x86_64\"\nos = \"linux\"\narch = \"x86_64\"\n";
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

#[test]
fn declared_os_is_not_inferred_from_target_id() {
    let inventory = "product = \"solstone-journal\"\n[artifact]\nbasename = \"solstone-journal-{version}-{os}-{arch}\"\n[[target]]\nid = \"linux-x86_64\"\nos = \"macos\"\narch = \"x86_64\"\n";
    let install = "PRODUCT=solstone-journal\n_base=${PRODUCT}-${VERSION}-${TARGET}\n";
    let unexpected = drift(inventory, install);
    assert_eq!(
        unexpected,
        BTreeSet::from([
            "solstone-journal-VERSION-macos-x86_64 solstone-journal-VERSION-linux-x86_64"
                .to_owned()
        ])
    );
}

#[test]
fn linux_target_arms_require_exact_inventory_parity() {
    let two_linux_inventory = "\
product = \"solstone-journal\"
[artifact]
basename = \"solstone-journal-{version}-{os}-{arch}\"
[[target]]
id = \"linux-x86_64\"
os = \"linux\"
arch = \"x86_64\"
[[target]]
id = \"linux-aarch64\"
os = \"linux\"
arch = \"aarch64\"
";
    let matching_install = "\
PRODUCT=solstone-journal
_base=${PRODUCT}-${VERSION}-${TARGET}
TARGET=linux-x86_64
TARGET=linux-aarch64
";
    assert_eq!(
        drift(two_linux_inventory, matching_install),
        BTreeSet::new()
    );

    let dropped_x86 = "\
PRODUCT=solstone-journal
_base=${PRODUCT}-${VERSION}-${TARGET}
TARGET=linux-aarch64
";
    assert_eq!(
        drift(two_linux_inventory, dropped_x86),
        BTreeSet::from(["missing required TARGET linux-x86_64".to_owned()])
    );

    let dropped_aarch64 = "\
PRODUCT=solstone-journal
_base=${PRODUCT}-${VERSION}-${TARGET}
TARGET=linux-x86_64
";
    assert_eq!(
        drift(two_linux_inventory, dropped_aarch64),
        BTreeSet::from(["missing required TARGET linux-aarch64".to_owned()])
    );

    let three_linux_inventory = "\
product = \"solstone-journal\"
[artifact]
basename = \"solstone-journal-{version}-{os}-{arch}\"
[[target]]
id = \"linux-x86_64\"
os = \"linux\"
arch = \"x86_64\"
[[target]]
id = \"linux-aarch64\"
os = \"linux\"
arch = \"aarch64\"
[[target]]
id = \"linux-riscv64\"
os = \"linux\"
arch = \"riscv64\"
";
    assert_eq!(
        drift(three_linux_inventory, matching_install),
        BTreeSet::from(["missing required TARGET linux-riscv64".to_owned()])
    );
}

#[test]
fn non_linux_and_duplicate_target_arms_are_rejected() {
    let multi_platform_inventory = "\
product = \"solstone-journal\"
[artifact]
basename = \"solstone-journal-{version}-{os}-{arch}\"
[[target]]
id = \"linux-x86_64\"
os = \"linux\"
arch = \"x86_64\"
[[target]]
id = \"linux-aarch64\"
os = \"linux\"
arch = \"aarch64\"
[[target]]
id = \"macos-arm64\"
os = \"macos\"
arch = \"arm64\"
[[target]]
id = \"windows-x86_64\"
os = \"windows\"
arch = \"x86_64\"
";
    let matching_install = "\
PRODUCT=solstone-journal
_base=${PRODUCT}-${VERSION}-${TARGET}
TARGET=linux-x86_64
TARGET=linux-aarch64
";
    assert_eq!(
        drift(multi_platform_inventory, matching_install),
        BTreeSet::new()
    );

    let macos_arm_install = format!("{matching_install}TARGET=macos-arm64\n");
    assert_eq!(
        drift(multi_platform_inventory, &macos_arm_install),
        BTreeSet::from(["unexpected TARGET macos-arm64".to_owned()])
    );

    let windows_arm_install = format!("{matching_install}TARGET=windows-x86_64\n");
    assert_eq!(
        drift(multi_platform_inventory, &windows_arm_install),
        BTreeSet::from(["unexpected TARGET windows-x86_64".to_owned()])
    );

    let unknown_arm_install = format!("{matching_install}TARGET=unknown-arch\n");
    assert_eq!(
        drift(multi_platform_inventory, &unknown_arm_install),
        BTreeSet::from(["unexpected TARGET unknown-arch".to_owned()])
    );

    let duplicate_arm_install = format!("{matching_install}TARGET=linux-x86_64\n");
    assert_eq!(
        drift(multi_platform_inventory, &duplicate_arm_install),
        BTreeSet::from(["duplicate TARGET linux-x86_64".to_owned()])
    );
}

#[test]
fn planted_base_formula_and_template_mismatches_are_detected() {
    let multi_platform_inventory = "\
product = \"solstone-journal\"
[artifact]
basename = \"solstone-journal-{version}-{os}-{arch}\"
[[target]]
id = \"linux-x86_64\"
os = \"linux\"
arch = \"x86_64\"
[[target]]
id = \"macos-arm64\"
os = \"macos\"
arch = \"arm64\"
[[target]]
id = \"windows-x86_64\"
os = \"windows\"
arch = \"x86_64\"
";
    let matching_install = "\
PRODUCT=solstone-journal
_base=${PRODUCT}-${VERSION}-${TARGET}
TARGET=linux-x86_64
";

    // 1. Planted _base formula mismatch
    let bad_base_install = "\
PRODUCT=solstone-journal
_base=${PRODUCT}-${VERSION}
TARGET=linux-x86_64
";
    assert_eq!(
        drift(multi_platform_inventory, bad_base_install),
        BTreeSet::from(["install _base formula".to_owned()])
    );

    // 2. Planted template mismatch on Linux, macOS, and Windows
    let bad_linux_template = "\
product = \"solstone-journal\"
[artifact]
basename = \"solstone-journal-{version}-{os}-{arch}\"
[[target]]
id = \"linux-x86_64\"
os = \"linux\"
arch = \"x86_64_broken\"
";
    assert_eq!(
        drift(bad_linux_template, matching_install),
        BTreeSet::from([
            "solstone-journal-VERSION-linux-x86_64_broken solstone-journal-VERSION-linux-x86_64"
                .to_owned()
        ])
    );

    let bad_macos_template = "\
product = \"solstone-journal\"
[artifact]
basename = \"solstone-journal-{version}-{os}-{arch}\"
[[target]]
id = \"macos-arm64\"
os = \"darwin\"
arch = \"arm64\"
";
    let no_arm_install = "\
PRODUCT=solstone-journal
_base=${PRODUCT}-${VERSION}-${TARGET}
";
    assert_eq!(
        drift(bad_macos_template, no_arm_install),
        BTreeSet::from([
            "solstone-journal-VERSION-darwin-arm64 solstone-journal-VERSION-macos-arm64".to_owned()
        ])
    );

    let bad_windows_template = "\
product = \"solstone-journal\"
[artifact]
basename = \"solstone-journal-{version}-{os}-{arch}\"
[[target]]
id = \"windows-x86_64\"
os = \"win\"
arch = \"x86_64\"
";
    assert_eq!(
        drift(bad_windows_template, no_arm_install),
        BTreeSet::from([
            "solstone-journal-VERSION-win-x86_64 solstone-journal-VERSION-windows-x86_64"
                .to_owned()
        ])
    );

    // 3. Orthogonality: planted product/base/template failure + matching arms fails identity checks
    let broken_identity_matching_arms = "\
PRODUCT=solstone-other
_base=${PRODUCT}-${VERSION}
TARGET=linux-x86_64
";
    let single_linux_inv = "\
product = \"solstone-journal\"
[artifact]
basename = \"solstone-journal-{version}-{os}-{arch}\"
[[target]]
id = \"linux-x86_64\"
os = \"linux\"
arch = \"x86_64\"
";
    let drift_identity = drift(single_linux_inv, broken_identity_matching_arms);
    assert_eq!(
        drift_identity,
        BTreeSet::from([
            "install _base formula".to_owned(),
            "product solstone-journal solstone-other".to_owned(),
            "solstone-journal-VERSION-linux-x86_64 solstone-other-VERSION-linux-x86_64".to_owned(),
        ])
    );

    // 4. Orthogonality: planted arm failure + matching identity fails arm checks
    let broken_arms_matching_identity = "\
PRODUCT=solstone-journal
_base=${PRODUCT}-${VERSION}-${TARGET}
TARGET=linux-x86_64
TARGET=linux-x86_64
TARGET=unknown-target
";
    let drift_arms = drift(single_linux_inv, broken_arms_matching_identity);
    assert_eq!(
        drift_arms,
        BTreeSet::from([
            "duplicate TARGET linux-x86_64".to_owned(),
            "unexpected TARGET unknown-target".to_owned(),
        ])
    );
}
