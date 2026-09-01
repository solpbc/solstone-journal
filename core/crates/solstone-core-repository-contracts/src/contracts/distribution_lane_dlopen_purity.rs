// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Structural guard for the invariant Brief D found broken by inspection
//! only: a `musl-static`-lane binary must never carry a runtime `dlopen`
//! dependency.
//!
//! `musl-static` is deliberate -- it is what makes the journal portable
//! across distributions with no dynamic loader in the process at all. A
//! crate that resolves a shared library at runtime (`libloading::Library::new`,
//! i.e. `dlopen`) can never work from inside such a binary: there is no
//! in-process loader to satisfy the call. That was exactly CED's defect
//! (`solstone-core-ced-sys` `dlopen`s `libced.so`, and reached
//! `solstone-core`'s dependency graph through
//! `solstone-core-sound-tags`/`solstone-core-local` unnoticed), and it is
//! invisible to `solstone-core-distribution`'s existing per-lane ELF policy
//! (`produce.rs::inspect_bin`, `elf.rs::inspect_core_family`): a runtime-
//! resolved `dlopen` target is loaded from a path string discovered at
//! runtime, so it leaves no static `DT_NEEDED` entry for an ELF scan to see.
//! `inspect_core_family` already requires zero `DT_NEEDED` entries and no
//! `PT_INTERP` -- CED's shipped `solstone-core` passed that check cleanly
//! while still being unconditionally broken.
//!
//! This contract closes the blind spot from the other side: it walks the
//! real Cargo dependency graph (`cargo metadata`, the same instrument
//! `workspace_reachability.rs` already uses in this crate) from every
//! `musl-static`-lane binary named in `core/distribution/inventory.toml` and
//! refuses if `libloading` -- the crate every runtime `dlopen` in this
//! workspace goes through (`solstone-core-ced-sys`, `solstone-core-vulkan-probe`,
//! `solstone-core-pdf` all depend on it directly) -- is reachable through an
//! ordinary (non-dev, non-build) edge. `solstone-core-vulkan-probe` and
//! `solstone-core-pdf` are already `zig-gnu-2.27`-lane binaries, so they
//! trip nothing here; the point is that CED's owning crates
//! (`solstone-core-sound-tags`,
//! `solstone-core-local`) must never again reach `libloading` from a
//! musl-static binary's graph without this test catching it before a
//! release ships.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

const INVENTORY: &str = "core/distribution/inventory.toml";
const WORKSPACE: &str = "core/Cargo.toml";
const MUSL_STATIC_LANE: &str = "musl-static";
/// Every runtime `dlopen` in this workspace goes through this crate.
const RUNTIME_DLOPEN_CRATE: &str = "libloading";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    resolve: Resolve,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
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

/// id-keyed dependency graph. Unlike `workspace_reachability.rs`'s graph,
/// this one is not restricted to workspace members: `libloading` is an
/// external crate, so the walk must cross into the full resolved graph.
struct Graph {
    id_by_name: BTreeMap<String, String>,
    name_by_id: BTreeMap<String, String>,
    /// Non-dev (normal or build) dependency edges, by package id.
    edges: BTreeMap<String, BTreeSet<String>>,
}

fn build_graph(metadata: &Metadata) -> Graph {
    let id_by_name = metadata
        .packages
        .iter()
        .map(|package| (package.name.clone(), package.id.clone()))
        .collect();
    let name_by_id = metadata
        .packages
        .iter()
        .map(|package| (package.id.clone(), package.name.clone()))
        .collect();
    let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for node in &metadata.resolve.nodes {
        let confers = node
            .deps
            .iter()
            .filter(|dep| {
                // Deliberately stricter than `workspace_reachability.rs`'s
                // "was this crate's source built" edge (which counts `build`
                // as conferring): this guard asks whether the *shipped
                // binary's own runtime code* can dlopen, so only a `None`
                // (ordinary, statically-linked-in) edge counts. A `build`
                // edge is a host-side compile-time tool -- e.g.
                // `ffmpeg-sys-next`'s build script pulls in `bindgen` ->
                // `clang-sys` -> `libloading` purely to generate FFI bindings
                // on the machine doing the build; none of that code, or its
                // dlopen call, ships inside `solstone-core`. Counting it
                // would false-positive on every musl-static binary that
                // depends on `solstone-core-observe-audio` (ffmpeg) today.
                dep.dep_kinds.iter().any(|kind| kind.kind.is_none())
            })
            .map(|dep| dep.pkg.clone())
            .collect();
        edges.insert(node.id.clone(), confers);
    }
    Graph {
        id_by_name,
        name_by_id,
        edges,
    }
}

/// Depth-first walk from `start_id` over the graph's ordinary edges (dev and
/// build edges already dropped by [`build_graph`]). Returns the id chain
/// from `start_id` down to `RUNTIME_DLOPEN_CRATE` when reachable, so a
/// violation report can show the path rather than just naming the binary.
fn dlopen_path(graph: &Graph, start_id: &str) -> Option<Vec<String>> {
    fn walk(
        graph: &Graph,
        current: &str,
        seen: &mut BTreeSet<String>,
        path: &mut Vec<String>,
    ) -> bool {
        if !seen.insert(current.to_owned()) {
            return false;
        }
        path.push(current.to_owned());
        if graph.name_by_id.get(current).map(String::as_str) == Some(RUNTIME_DLOPEN_CRATE) {
            return true;
        }
        if let Some(deps) = graph.edges.get(current) {
            for dep in deps {
                if walk(graph, dep, seen, path) {
                    return true;
                }
            }
        }
        path.pop();
        false
    }

    let mut seen = BTreeSet::new();
    let mut path = Vec::new();
    walk(graph, start_id, &mut seen, &mut path).then(|| {
        path.iter()
            .map(|id| {
                graph
                    .name_by_id
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| id.clone())
            })
            .collect()
    })
}

/// Package names from every `[[entry]] kind = "bin"` block in
/// `inventory.toml` whose `lane = "musl-static"`.
fn musl_static_bin_packages(inventory: &str) -> BTreeSet<String> {
    let mut packages = BTreeSet::new();
    let mut kind_bin = false;
    let mut package: Option<String> = None;
    let mut lane: Option<String> = None;
    let mut flush = |kind_bin: bool, package: &mut Option<String>, lane: &mut Option<String>| {
        if kind_bin
            && lane.as_deref() == Some(MUSL_STATIC_LANE)
            && let Some(name) = package.take()
        {
            packages.insert(name);
        }
        *package = None;
        *lane = None;
    };
    for line in inventory.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[[") {
            flush(kind_bin, &mut package, &mut lane);
            kind_bin = false;
            continue;
        }
        if trimmed == "kind = \"bin\"" {
            kind_bin = true;
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("package = ") {
            package = Some(value.trim().trim_matches('"').to_owned());
        }
        if let Some(value) = trimmed.strip_prefix("lane = ") {
            lane = Some(value.trim().trim_matches('"').to_owned());
        }
    }
    flush(kind_bin, &mut package, &mut lane);
    packages
}

fn format_violations(violations: &BTreeMap<String, Vec<String>>) -> String {
    let mut lines = vec!["unexpected:".to_owned()];
    for (package, path) in violations {
        lines.push(format!(
            "  {package} reaches {RUNTIME_DLOPEN_CRATE} via {}",
            path.join(" -> ")
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_from(edges: &[(&str, &[&str])], names: &[&str]) -> Graph {
        let id_by_name = names
            .iter()
            .map(|name| (name.to_string(), name.to_string()))
            .collect();
        let name_by_id = names
            .iter()
            .map(|name| (name.to_string(), name.to_string()))
            .collect();
        let edges = edges
            .iter()
            .map(|(from, to)| {
                (
                    from.to_string(),
                    to.iter().map(|item| item.to_string()).collect(),
                )
            })
            .collect();
        Graph {
            id_by_name,
            name_by_id,
            edges,
        }
    }

    #[test]
    fn parses_lane_and_package_regardless_of_field_order() {
        let inventory = r#"
[[entry]]
kind = "bin"
package = "solstone-core"
bin = "solstone-core"
lane = "musl-static"

[[entry]]
kind = "bin"
package = "solstone-core-pdf"
bin = "solstone-core-pdf"
lane = "zig-gnu-2.27"

[[entry]]
kind = "model-asset"
source = "core/models/assets/x.onnx"
"#;
        let packages = musl_static_bin_packages(inventory);
        assert_eq!(packages, BTreeSet::from(["solstone-core".to_owned()]));
    }

    #[test]
    fn last_entry_in_the_file_is_still_flushed() {
        let inventory = "[[entry]]\nkind = \"bin\"\npackage = \"solstone-core-depict\"\nlane = \"musl-static\"\n";
        let packages = musl_static_bin_packages(inventory);
        assert_eq!(
            packages,
            BTreeSet::from(["solstone-core-depict".to_owned()])
        );
    }

    /// The guard must fire, not merely stay quiet on a clean graph: this
    /// reproduces the exact CED shape (a musl-static bin reaching
    /// `libloading` two hops down through a ced-sys-like leaf).
    #[test]
    fn dlopen_path_fires_on_the_ced_shaped_violation() {
        let graph = graph_from(
            &[
                ("solstone-core", &["solstone-core-sound-tags"]),
                ("solstone-core-sound-tags", &["solstone-core-ced-sys"]),
                ("solstone-core-ced-sys", &["libloading"]),
            ],
            &[
                "solstone-core",
                "solstone-core-sound-tags",
                "solstone-core-ced-sys",
                "libloading",
            ],
        );
        let path = dlopen_path(&graph, "solstone-core").expect("must find the dlopen edge");
        assert_eq!(
            path,
            vec![
                "solstone-core",
                "solstone-core-sound-tags",
                "solstone-core-ced-sys",
                "libloading",
            ]
        );
    }

    #[test]
    fn dlopen_path_is_none_on_a_clean_graph() {
        let graph = graph_from(
            &[("solstone-core", &["solstone-core-journal"])],
            &["solstone-core", "solstone-core-journal", "libloading"],
        );
        assert_eq!(dlopen_path(&graph, "solstone-core"), None);
    }

    #[test]
    fn a_dev_only_edge_does_not_confer_the_dlopen_dependency() {
        // Mirrors the live case: solstone-core-vad-analyze depends on
        // solstone-core-transcribe only as a dev-dependency. This fixture
        // builds the graph as `build_graph` would after dropping dev edges --
        // i.e. the edge is simply absent -- confirming the walk correctly
        // reports no path rather than one that should not exist.
        let graph = graph_from(
            &[("solstone-core-vad-analyze", &[])],
            &[
                "solstone-core-vad-analyze",
                "solstone-core-transcribe",
                "libloading",
            ],
        );
        assert_eq!(dlopen_path(&graph, "solstone-core-vad-analyze"), None);
    }

    #[test]
    fn build_graph_drops_dev_and_build_edges_from_live_cargo_metadata_shape() {
        // Reproduces the exact false positive this guard hit against the
        // live workspace: `ffmpeg-sys-next`'s [build-dependencies] pull in
        // `libloading` (via bindgen/clang-sys) purely to generate FFI
        // bindings on the build host -- that code never ships inside the
        // musl-static binary, so a `build`-kind edge must not confer the
        // dlopen dependency, exactly like a `dev`-kind edge.
        let metadata = Metadata {
            packages: vec![
                Package {
                    id: "a".to_owned(),
                    name: "root".to_owned(),
                },
                Package {
                    id: "b".to_owned(),
                    name: "dev-only".to_owned(),
                },
                Package {
                    id: "d".to_owned(),
                    name: "build-only".to_owned(),
                },
                Package {
                    id: "c".to_owned(),
                    name: "libloading".to_owned(),
                },
            ],
            resolve: Resolve {
                nodes: vec![
                    Node {
                        id: "a".to_owned(),
                        deps: vec![
                            Dep {
                                pkg: "b".to_owned(),
                                dep_kinds: vec![DepKind {
                                    kind: Some("dev".to_owned()),
                                }],
                            },
                            Dep {
                                pkg: "d".to_owned(),
                                dep_kinds: vec![DepKind {
                                    kind: Some("build".to_owned()),
                                }],
                            },
                        ],
                    },
                    Node {
                        id: "b".to_owned(),
                        deps: vec![Dep {
                            pkg: "c".to_owned(),
                            dep_kinds: vec![DepKind { kind: None }],
                        }],
                    },
                    Node {
                        id: "d".to_owned(),
                        deps: vec![Dep {
                            pkg: "c".to_owned(),
                            dep_kinds: vec![DepKind { kind: None }],
                        }],
                    },
                    Node {
                        id: "c".to_owned(),
                        deps: vec![],
                    },
                ],
            },
        };
        let graph = build_graph(&metadata);
        assert_eq!(dlopen_path(&graph, "a"), None);
        assert_eq!(
            dlopen_path(&graph, "b"),
            Some(vec!["dev-only".to_owned(), "libloading".to_owned()])
        );
        assert_eq!(
            dlopen_path(&graph, "d"),
            Some(vec!["build-only".to_owned(), "libloading".to_owned()])
        );
    }

    #[test]
    fn live_workspace_has_no_musl_static_binary_that_reaches_libloading() {
        let root = repository_root();
        let inventory = fs::read_to_string(root.join(INVENTORY)).expect("read inventory");
        let bin_packages = musl_static_bin_packages(&inventory);
        assert!(
            !bin_packages.is_empty(),
            "instrument found zero musl-static bin packages in {INVENTORY} -- inventory parsing broke"
        );
        for required in [
            "solstone-core",
            "solstone-core-journal-bin",
            "solstone-core-depict",
        ] {
            assert!(
                bin_packages.contains(required),
                "{required} must parse as a musl-static bin package -- inventory parsing broke"
            );
        }
        let metadata = cargo_metadata(&root.join(WORKSPACE)).expect("cargo metadata");
        let graph = build_graph(&metadata);
        let mut violations = BTreeMap::new();
        for package in &bin_packages {
            let Some(id) = graph.id_by_name.get(package) else {
                panic!(
                    "musl-static package {package} from {INVENTORY} is not a resolved workspace package"
                );
            };
            if let Some(path) = dlopen_path(&graph, id) {
                violations.insert(package.clone(), path);
            }
        }
        assert!(violations.is_empty(), "{}", format_violations(&violations));
    }
}
