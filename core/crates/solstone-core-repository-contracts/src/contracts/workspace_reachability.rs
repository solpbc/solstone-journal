// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Workspace reachability: a crate that compiles and tests is not finished
//! until a shipped path executes it. Mirrors the org-side model in
//! rust-engineering-standard § 7a. The org Python tool is not wired into
//! `make ci` — that gate is Rust-only.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

const WORKSPACE: &str = "core/Cargo.toml";
const ALLOWLIST: &str = "core/reachability-allow.toml";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Findings {
    forgotten: BTreeSet<String>,
    stale: BTreeSet<String>,
    absent: BTreeSet<String>,
}

impl Findings {
    fn is_clean(&self) -> bool {
        self.forgotten.is_empty() && self.stale.is_empty() && self.absent.is_empty()
    }
}

fn load_allowlist(text: &str) -> Result<BTreeMap<String, String>, String> {
    let document = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("allowlist is not TOML: {error}"))?;
    let Some(allow) = document.get("allow").and_then(toml_edit::Item::as_table) else {
        return Ok(BTreeMap::new());
    };
    let mut map = BTreeMap::new();
    let mut blank = BTreeSet::new();
    for (name, value) in allow.iter() {
        let reason = value.as_str().unwrap_or("").trim();
        if reason.is_empty() {
            blank.insert(name.to_owned());
            continue;
        }
        map.insert(name.to_owned(), reason.to_owned());
    }
    if !blank.is_empty() {
        return Err(format!(
            "allowlist entries need a stated reason, these are blank: {}",
            blank.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    Ok(map)
}

fn evaluate(
    members: &BTreeSet<String>,
    bin_roots: &BTreeSet<String>,
    prod_edges: &BTreeMap<String, BTreeSet<String>>,
    allow: &BTreeMap<String, String>,
) -> Result<Findings, String> {
    if members.is_empty() {
        return Err(
            "zero workspace members discovered — the instrument failed to measure".to_owned(),
        );
    }
    if bin_roots.is_empty() {
        return Err(
            "no member declares a bin target, so nothing can be a reachability root".to_owned(),
        );
    }
    let mut reach = bin_roots.clone();
    let mut stack: Vec<String> = bin_roots.iter().cloned().collect();
    while let Some(current) = stack.pop() {
        let Some(next) = prod_edges.get(&current) else {
            continue;
        };
        for dep in next {
            if reach.insert(dep.clone()) {
                stack.push(dep.clone());
            }
        }
    }
    let unreached: BTreeSet<String> = members.difference(&reach).cloned().collect();
    Ok(Findings {
        forgotten: unreached
            .iter()
            .filter(|name| !allow.contains_key(*name))
            .cloned()
            .collect(),
        stale: allow
            .keys()
            .filter(|name| reach.contains(*name))
            .cloned()
            .collect(),
        absent: allow
            .keys()
            .filter(|name| !members.contains(*name))
            .cloned()
            .collect(),
    })
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    resolve: Resolve,
    workspace_members: Vec<String>,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    targets: Vec<Target>,
}

#[derive(Deserialize)]
struct Target {
    kind: Vec<String>,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    deps: Vec<Dep>,
}

#[derive(Deserialize)]
struct Dep {
    pkg: String,
    dep_kinds: Vec<DepKind>,
}

#[derive(Deserialize)]
struct DepKind {
    kind: Option<String>,
}

fn graph_from_metadata(
    metadata: &Metadata,
) -> (
    BTreeSet<String>,
    BTreeSet<String>,
    BTreeMap<String, BTreeSet<String>>,
) {
    let members: BTreeSet<String> = metadata.workspace_members.iter().cloned().collect();
    let packages: BTreeMap<&str, &Package> = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect();
    let mut bin_roots = BTreeSet::new();
    let mut names = BTreeMap::new();
    for id in &members {
        let Some(package) = packages.get(id.as_str()) else {
            continue;
        };
        names.insert(id.clone(), package.name.clone());
        if package
            .targets
            .iter()
            .any(|target| target.kind.iter().any(|kind| kind == "bin"))
        {
            bin_roots.insert(package.name.clone());
        }
    }
    let mut prod: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for node in &metadata.resolve.nodes {
        if !members.contains(&node.id) {
            continue;
        }
        let Some(from) = names.get(&node.id) else {
            continue;
        };
        for dep in &node.deps {
            if !members.contains(&dep.pkg) {
                continue;
            }
            let Some(to) = names.get(&dep.pkg) else {
                continue;
            };
            let confers = dep.dep_kinds.iter().any(|kind| match kind.kind.as_deref() {
                None | Some("build") => true,
                Some("dev") => false,
                Some(_) => false,
            });
            if confers {
                prod.entry(from.clone()).or_default().insert(to.clone());
            }
        }
    }
    let member_names = names.values().cloned().collect();
    (member_names, bin_roots, prod)
}

fn cargo_metadata(manifest: &Path) -> Result<Metadata, String> {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--offline",
            "--locked",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .map_err(|error| format!("cargo metadata could not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed (exit {}): {}",
            output.status.code().unwrap_or(2),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata JSON did not parse: {error}"))
}

fn format_named_list(label: &str, names: &BTreeSet<String>) -> String {
    let mut lines = vec![format!("{label}:")];
    for name in names {
        lines.push(format!("  {name}"));
    }
    lines.join("\n")
}

#[test]
fn blank_allowlist_reason_is_refused() {
    let error = load_allowlist("[allow]\nforgotten = \"\"\n").unwrap_err();
    assert!(error.contains("blank"));
    assert!(error.contains("forgotten"));
}

#[test]
fn forgotten_crate_is_a_finding() {
    let members = BTreeSet::from(["root".to_owned(), "orphan".to_owned()]);
    let roots = BTreeSet::from(["root".to_owned()]);
    let findings = evaluate(&members, &roots, &BTreeMap::new(), &BTreeMap::new()).unwrap();
    assert_eq!(findings.forgotten, BTreeSet::from(["orphan".to_owned()]));
    assert!(findings.stale.is_empty());
}

#[test]
fn allowlisted_crate_that_is_now_reachable_is_stale() {
    let members = BTreeSet::from(["root".to_owned(), "lib".to_owned()]);
    let roots = BTreeSet::from(["root".to_owned()]);
    let mut edges = BTreeMap::new();
    edges.insert("root".to_owned(), BTreeSet::from(["lib".to_owned()]));
    let mut allow = BTreeMap::new();
    allow.insert("lib".to_owned(), "under construction".to_owned());
    let findings = evaluate(&members, &roots, &edges, &allow).unwrap();
    assert!(findings.forgotten.is_empty());
    assert_eq!(findings.stale, BTreeSet::from(["lib".to_owned()]));
}

#[test]
fn allowlist_name_absent_from_workspace_is_a_finding() {
    let members = BTreeSet::from(["root".to_owned()]);
    let roots = BTreeSet::from(["root".to_owned()]);
    let mut allow = BTreeMap::new();
    allow.insert("renamed".to_owned(), "was a real crate".to_owned());
    let findings = evaluate(&members, &roots, &BTreeMap::new(), &allow).unwrap();
    assert_eq!(findings.absent, BTreeSet::from(["renamed".to_owned()]));
}

#[test]
fn dev_edges_do_not_confer_reachability() {
    // The live cargo-metadata graph already drops dev edges. This pins the
    // evaluate contract: an orphan with no prod edge stays forgotten.
    let members = BTreeSet::from(["root".to_owned(), "test-only".to_owned()]);
    let roots = BTreeSet::from(["root".to_owned()]);
    let findings = evaluate(&members, &roots, &BTreeMap::new(), &BTreeMap::new()).unwrap();
    assert!(findings.forgotten.contains("test-only"));
}

#[test]
fn live_workspace_matches_the_committed_allowlist() {
    let root = repository_root();
    let allow_text = fs::read_to_string(root.join(ALLOWLIST)).expect("read reachability allowlist");
    let allow = load_allowlist(&allow_text).expect("allowlist parses");
    let metadata = cargo_metadata(&root.join(WORKSPACE)).expect("cargo metadata");
    let (members, roots, edges) = graph_from_metadata(&metadata);
    let findings = evaluate(&members, &roots, &edges, &allow).expect("evaluate");
    assert!(
        findings.is_clean(),
        "{}\n{}\n{}",
        format_named_list("forgotten", &findings.forgotten),
        format_named_list("stale allowlist", &findings.stale),
        format_named_list("allowlist names not in workspace", &findings.absent)
    );
}
