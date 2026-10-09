// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

const OWNER_HOST: &str = "updates.solstone.app";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

struct AllowlistRow {
    path: &'static str,
    marker: &'static str,
    reason: &'static str,
}

const ALLOWLIST: &[AllowlistRow] = &[
    AllowlistRow {
        path: "core/crates/solstone-core-artifact-download/src/lib.rs",
        marker: "OWNER_ORIGIN_HOST",
        reason: "only production owner-host literal",
    },
    AllowlistRow {
        path: "core/crates/solstone-core-journal-app/src/update.rs",
        marker: "FEED_URL",
        reason: "Windows update feed",
    },
    AllowlistRow {
        path: "core/crates/solstone-core-assets/src/lib.rs",
        marker: "llama-cuda13",
        reason: "catalog CUDA upstream_url rows",
    },
    AllowlistRow {
        path: "core/crates/solstone-core/src/install_provider.rs",
        marker: "PARAKEET_DOWNLOAD_DISCLOSURE",
        reason: "disclosure copy",
    },
    AllowlistRow {
        path: "core/crates/solstone-core/src/install_provider.rs",
        marker: "LOCAL_DOWNLOAD_DISCLOSURE",
        reason: "disclosure copy",
    },
    AllowlistRow {
        path: "core/crates/solstone-core/src/install_provider.rs",
        marker: "WINDOWS_LOCAL_DOWNLOAD_DISCLOSURE",
        reason: "disclosure copy",
    },
    AllowlistRow {
        path: "core/crates/solstone-core-local/src/install/coreml_install.rs",
        marker: "PARAKEET_COREML_DOWNLOAD_DISCLOSURE",
        reason: "disclosure copy",
    },
    AllowlistRow {
        path: "core/crates/solstone-core-local/src/install/pins.rs",
        marker: "llama-cuda13",
        reason: "CUDA_ARTIFACTS pin identity, not the fetch URL",
    },
    AllowlistRow {
        path: "core/crates/solstone-core-distribution/src/component_evidence.rs",
        marker: "upstream_url.contains",
        reason: "renderer refuses an upstream URL that names the owner host",
    },
    AllowlistRow {
        path: "core/crates/solstone-core-distribution/src/component_evidence.rs",
        marker: "source.contains",
        reason: "renderer refuses an upstream URL that names the owner host",
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Hit {
    path: String,
    line: usize,
    content: String,
}

fn cfg_requires_test(attr: &syn::Attribute) -> bool {
    if !attr.path().is_ident("cfg") {
        return false;
    }
    let s = attr.meta.to_token_stream().to_string();
    if s.contains("not ( test )") || s.contains("not(test)") {
        return false;
    }
    fn names_test(tokens: proc_macro2::TokenStream) -> bool {
        tokens.into_iter().any(|token| match token {
            proc_macro2::TokenTree::Ident(ident) => ident == "test",
            proc_macro2::TokenTree::Group(group) => names_test(group.stream()),
            _ => false,
        })
    }
    match &attr.meta {
        syn::Meta::List(list) => names_test(list.tokens.clone()),
        _ => false,
    }
}

fn is_test_attr(attr: &syn::Attribute) -> bool {
    attr.path().is_ident("test")
}

fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::ExternCrate(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::ForeignMod(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::TraitAlias(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Union(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

fn is_test_item(item: &syn::Item) -> bool {
    if let syn::Item::Mod(m) = item
        && m.ident == "tests"
    {
        return true;
    }
    let attrs = item_attrs(item);
    attrs
        .iter()
        .any(|a| is_test_attr(a) || cfg_requires_test(a))
}

fn impl_item_attrs(item: &syn::ImplItem) -> &[syn::Attribute] {
    match item {
        syn::ImplItem::Const(i) => &i.attrs,
        syn::ImplItem::Fn(i) => &i.attrs,
        syn::ImplItem::Type(i) => &i.attrs,
        syn::ImplItem::Macro(i) => &i.attrs,
        _ => &[],
    }
}

fn is_test_impl_item(item: &syn::ImplItem) -> bool {
    let attrs = impl_item_attrs(item);
    attrs
        .iter()
        .any(|a| is_test_attr(a) || cfg_requires_test(a))
}

fn trait_item_attrs(item: &syn::TraitItem) -> &[syn::Attribute] {
    match item {
        syn::TraitItem::Const(i) => &i.attrs,
        syn::TraitItem::Fn(i) => &i.attrs,
        syn::TraitItem::Type(i) => &i.attrs,
        syn::TraitItem::Macro(i) => &i.attrs,
        _ => &[],
    }
}

fn is_test_trait_item(item: &syn::TraitItem) -> bool {
    let attrs = trait_item_attrs(item);
    attrs
        .iter()
        .any(|a| is_test_attr(a) || cfg_requires_test(a))
}

struct TestSpanCollector {
    test_line_ranges: Vec<(usize, usize)>,
}

impl<'ast> Visit<'ast> for TestSpanCollector {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        if is_test_item(node) {
            let attrs = item_attrs(node);
            let span = node.span();
            let start = attrs
                .first()
                .map(|a| a.span().start().line)
                .unwrap_or_else(|| span.start().line);
            let end = span.end().line;
            self.test_line_ranges.push((start, end));
            return;
        }
        visit::visit_item(self, node);
    }

    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        if is_test_impl_item(node) {
            let attrs = impl_item_attrs(node);
            let span = node.span();
            let start = attrs
                .first()
                .map(|a| a.span().start().line)
                .unwrap_or_else(|| span.start().line);
            let end = span.end().line;
            self.test_line_ranges.push((start, end));
            return;
        }
        visit::visit_impl_item(self, node);
    }

    fn visit_trait_item(&mut self, node: &'ast syn::TraitItem) {
        if is_test_trait_item(node) {
            let attrs = trait_item_attrs(node);
            let span = node.span();
            let start = attrs
                .first()
                .map(|a| a.span().start().line)
                .unwrap_or_else(|| span.start().line);
            let end = span.end().line;
            self.test_line_ranges.push((start, end));
            return;
        }
        visit::visit_trait_item(self, node);
    }
}

fn scan_source(relative_path: &str, content: &str) -> Vec<Hit> {
    if relative_path.contains("tests") {
        return Vec::new();
    }
    let syntax = match syn::parse_file(content) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    if syntax.attrs.iter().any(cfg_requires_test) {
        return Vec::new();
    }

    let mut collector = TestSpanCollector {
        test_line_ranges: Vec::new(),
    };
    collector.visit_file(&syntax);

    let mut hits = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let in_test = collector
            .test_line_ranges
            .iter()
            .any(|&(start, end)| line_no >= start && line_no <= end);
        if in_test {
            continue;
        }
        if line.contains(OWNER_HOST) {
            hits.push(Hit {
                path: relative_path.to_string(),
                line: line_no,
                content: line.to_string(),
            });
        }
    }
    hits
}

fn validate_hits(hits: &[Hit]) -> Vec<String> {
    let mut errors = Vec::new();
    let mut hit_matched = vec![false; hits.len()];
    let mut row_hit_count = vec![0usize; ALLOWLIST.len()];

    for (i, hit) in hits.iter().enumerate() {
        for (j, row) in ALLOWLIST.iter().enumerate() {
            if hit.path == row.path && hit.content.contains(row.marker) {
                hit_matched[i] = true;
                row_hit_count[j] += 1;
            }
        }
    }

    for (i, hit) in hits.iter().enumerate() {
        if !hit_matched[i] {
            errors.push(format!(
                "untracked owner origin host reference in {}:{} : {}",
                hit.path, hit.line, hit.content
            ));
        }
    }

    for (j, row) in ALLOWLIST.iter().enumerate() {
        if row_hit_count[j] == 0 {
            errors.push(format!(
                "allowlist row never hit: {} with marker '{}' ({})",
                row.path, row.marker, row.reason
            ));
        }
    }

    errors
}

fn discover_inventory_packages(inventory_path: &Path) -> BTreeSet<String> {
    let content = fs::read_to_string(inventory_path).expect("read inventory.toml");
    let doc = content
        .parse::<toml_edit::DocumentMut>()
        .expect("parse inventory.toml");
    let mut packages = BTreeSet::new();

    fn walk_item(item: &toml_edit::Item, out: &mut BTreeSet<String>) {
        if let Some(table) = item.as_table_like() {
            for (key, val) in table.iter() {
                if key == "package"
                    && let Some(s) = val.as_str()
                    && (s.starts_with("solstone-core-") || s == "solstone-core")
                {
                    out.insert(s.to_string());
                }
                walk_item(val, out);
            }
        } else if let Some(aot) = item.as_array_of_tables() {
            for table in aot.iter() {
                for (key, val) in table.iter() {
                    if key == "package"
                        && let Some(s) = val.as_str()
                        && (s.starts_with("solstone-core-") || s == "solstone-core")
                    {
                        out.insert(s.to_string());
                    }
                    walk_item(val, out);
                }
            }
        }
    }

    for (key, val) in doc.iter() {
        if key == "package"
            && let Some(s) = val.as_str()
            && (s.starts_with("solstone-core-") || s == "solstone-core")
        {
            packages.insert(s.to_string());
        }
        walk_item(val, &mut packages);
    }
    packages.insert("solstone-core-mcp-endpoint".to_string());
    packages
}

fn path_attribute(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| {
        if !attr.path().is_ident("path") {
            return None;
        }
        let syn::Meta::NameValue(value) = &attr.meta else {
            return None;
        };
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(path),
            ..
        }) = &value.value
        else {
            return None;
        };
        Some(path.value())
    })
}

fn module_contexts(crate_root: &Path) -> BTreeMap<PathBuf, bool> {
    let src = crate_root.join("src");
    let mut roots = vec![src.join("lib.rs"), src.join("main.rs")];
    if let Ok(entries) = fs::read_dir(src.join("bin")) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                roots.push(p);
            }
        }
    }

    let mut contexts: BTreeMap<PathBuf, bool> = BTreeMap::new();
    let mut pending: Vec<(PathBuf, bool, bool)> = roots
        .into_iter()
        .filter(|r| r.is_file())
        .map(|r| (r, false, true))
        .collect();

    while let Some((file, is_test, directory_owner)) = pending.pop() {
        if let Some(&existing_test) = contexts.get(&file) {
            if existing_test && !is_test {
                // Upgraded to non-test context
                contexts.insert(file.clone(), false);
            } else {
                continue;
            }
        } else {
            contexts.insert(file.clone(), is_test);
        }

        let content = match fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let syntax = match syn::parse_file(&content) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let directory = file.parent().unwrap_or(crate_root).to_path_buf();
        let owner = directory_owner
            || file
                .file_name()
                .is_some_and(|name| name == "mod.rs" || name == "lib.rs" || name == "main.rs");

        for item in &syntax.items {
            let syn::Item::Mod(module) = item else {
                continue;
            };
            if module.content.is_some() {
                continue;
            }
            let child_is_test = is_test
                || module.ident == "tests"
                || module
                    .attrs
                    .iter()
                    .any(|a| is_test_attr(a) || cfg_requires_test(a));
            let name = module.ident.to_string();
            let candidates = if let Some(path) = path_attribute(&module.attrs) {
                vec![directory.join(path)]
            } else {
                let base = if owner {
                    directory.clone()
                } else {
                    directory.join(file.file_stem().unwrap_or_default())
                };
                vec![
                    base.join(format!("{name}.rs")),
                    base.join(&name).join("mod.rs"),
                ]
            };
            if let Some(target) = candidates.into_iter().find(|c| c.is_file()) {
                let target_owner = target.file_name().is_some_and(|n| n == "mod.rs");
                pending.push((target, child_is_test, target_owner));
            }
        }
    }

    contexts
}

fn scan_reachable_crates(repo_root: &Path) -> Vec<Hit> {
    let inventory_packages =
        discover_inventory_packages(&repo_root.join("core/distribution/inventory.toml"));

    let crates_dir = repo_root.join("core/crates");
    let mut crate_dirs: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut normal_deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    if let Ok(entries) = fs::read_dir(&crates_dir) {
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("Cargo.toml");
            if !manifest_path.is_file() {
                continue;
            }
            let content = match fs::read_to_string(&manifest_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let doc = match content.parse::<toml_edit::DocumentMut>() {
                Ok(d) => d,
                Err(_) => continue,
            };
            let pkg_name = doc
                .get("package")
                .and_then(toml_edit::Item::as_table_like)
                .and_then(|t| t.get("name"))
                .and_then(toml_edit::Item::as_str);
            let Some(name) = pkg_name else {
                continue;
            };
            crate_dirs.insert(name.to_string(), entry.path());

            let mut deps = BTreeSet::new();
            if let Some(deps_table) = doc
                .get("dependencies")
                .and_then(toml_edit::Item::as_table_like)
            {
                for (dep_key, dep_val) in deps_table.iter() {
                    let dep_name = dep_val
                        .as_table_like()
                        .and_then(|t| t.get("package"))
                        .and_then(toml_edit::Item::as_str)
                        .unwrap_or(dep_key);
                    deps.insert(dep_name.to_string());
                }
            }
            if let Some(target_table) = doc.get("target").and_then(toml_edit::Item::as_table_like) {
                for (_target_key, target_val) in target_table.iter() {
                    if let Some(deps_table) = target_val
                        .as_table_like()
                        .and_then(|t| t.get("dependencies"))
                        .and_then(toml_edit::Item::as_table_like)
                    {
                        for (dep_key, dep_val) in deps_table.iter() {
                            let dep_name = dep_val
                                .as_table_like()
                                .and_then(|t| t.get("package"))
                                .and_then(toml_edit::Item::as_str)
                                .unwrap_or(dep_key);
                            deps.insert(dep_name.to_string());
                        }
                    }
                }
            }
            normal_deps.insert(name.to_string(), deps);
        }
    }

    let mut reachable = BTreeSet::new();
    let mut queue: Vec<String> = inventory_packages.into_iter().collect();
    for p in &queue {
        reachable.insert(p.clone());
    }

    while let Some(current) = queue.pop() {
        if let Some(deps) = normal_deps.get(&current) {
            for dep in deps {
                if crate_dirs.contains_key(dep) && reachable.insert(dep.clone()) {
                    queue.push(dep.clone());
                }
            }
        }
    }

    let mut hits = Vec::new();
    for package in reachable {
        let Some(crate_dir) = crate_dirs.get(&package) else {
            continue;
        };
        let contexts = module_contexts(crate_dir);
        let src_dir = crate_dir.join("src");
        if !src_dir.is_dir() {
            continue;
        }

        fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        collect_rs_files(&p, out);
                    } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                        out.push(p);
                    }
                }
            }
        }

        let mut files = Vec::new();
        collect_rs_files(&src_dir, &mut files);

        for file in files {
            let file_str = file.to_string_lossy();
            if file_str.contains("tests") {
                continue;
            }
            if contexts.get(&file) == Some(&true) {
                // reached only via test mod
                continue;
            }
            let content = match fs::read_to_string(&file) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let relative_path = file
                .strip_prefix(repo_root)
                .unwrap_or(&file)
                .to_string_lossy()
                .to_string();
            hits.extend(scan_source(&relative_path, &content));
        }
    }

    hits
}

#[test]
fn in_memory_test_sample_naming_host_is_ignored() {
    let source = r#"
#[cfg(test)]
mod tests {
    const URL: &str = "https://updates.solstone.app/sample";
}

#[cfg(all(test, feature = "full-tests"))]
fn test_helper() {
    let _ = "https://updates.solstone.app/helper";
}

#[test]
fn pure_test() {
    assert_eq!("https://updates.solstone.app/pure", "x");
}
"#;
    let hits = scan_source("core/crates/sample/src/lib.rs", source);
    assert!(
        hits.is_empty(),
        "test sample must be ignored, got hits: {hits:?}"
    );
}

#[test]
fn in_memory_non_test_sample_naming_host_fails() {
    let source = r#"
pub const PRODUCTION_HOST_URL: &str = "https://updates.solstone.app/untracked";
"#;
    let hits = scan_source("core/crates/sample/src/lib.rs", source);
    assert_eq!(
        hits.len(),
        1,
        "non-test sample must produce a hit, got: {hits:?}"
    );
    let errors = validate_hits(&hits);
    assert!(
        !errors.is_empty(),
        "untracked production sample must fail validation"
    );
}

#[test]
fn owner_origin_host_production_contract_passes() {
    let root = repository_root();
    let hits = scan_reachable_crates(&root);
    let errors = validate_hits(&hits);
    assert!(
        errors.is_empty(),
        "owner origin host contract violations:\n{}",
        errors.join("\n")
    );
}
