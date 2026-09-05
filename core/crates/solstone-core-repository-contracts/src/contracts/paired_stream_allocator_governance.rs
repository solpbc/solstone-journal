// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;

const ALLOCATOR_IMPORT: &str = "solstone_core_segment::bind_paired_stream";
const BIND_PAIRED_STREAM: &str = "bind_paired_stream";
const BIND_STREAM: &str = "bind_stream";
const INGEST_BIND_PATH: &str = "solstone-core-ingest/src/stream_identity.rs";
const SEGMENT_CRATE: &str = "solstone-core-segment";

/// Fixed AC21 evidence vectors, functionally exercised in
/// solstone-core-segment (core/fixtures/stream-name-projection-vectors.json,
/// the "studio-mac" entries for source "" and "tmux"). This
/// crate does not depend on solstone-core-segment and does not re-execute
/// them; it only pins that they exist and documents where.
const FIXED_SOURCE_VECTORS: [&str; 2] = ["", "tmux"];

struct TargetIdentVisitor<'a> {
    targets: &'a [&'a str],
    hits: Vec<String>,
}

impl TargetIdentVisitor<'_> {
    fn record_identifier(&mut self, raw: &str) {
        let normalized = raw.strip_prefix("r#").unwrap_or(raw);
        if self.targets.contains(&normalized) {
            self.hits.push(normalized.to_owned());
        }
    }

    fn visit_opaque_token_stream(&mut self, tokens: proc_macro2::TokenStream) {
        for token in tokens {
            match token {
                proc_macro2::TokenTree::Ident(ident) => {
                    self.record_identifier(&ident.to_string());
                }
                proc_macro2::TokenTree::Group(group) => {
                    self.visit_opaque_token_stream(group.stream());
                }
                proc_macro2::TokenTree::Literal(_) | proc_macro2::TokenTree::Punct(_) => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for TargetIdentVisitor<'_> {
    fn visit_ident(&mut self, ident: &'ast syn::Ident) {
        self.record_identifier(&ident.to_string());
    }

    fn visit_token_stream(&mut self, tokens: &'ast proc_macro2::TokenStream) {
        self.visit_opaque_token_stream(tokens.clone());
    }
}

struct CallSiteVisitor<'a> {
    target: &'a str,
    count: usize,
}

impl<'ast> Visit<'ast> for CallSiteVisitor<'_> {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if path_ends_with_target(&node.func, self.target) {
            self.count += 1;
        }
        syn::visit::visit_expr_call(self, node);
    }
}

fn path_ends_with_target(expression: &syn::Expr, target: &str) -> bool {
    let mut expression = expression;
    loop {
        match expression {
            syn::Expr::Group(group) => expression = &group.expr,
            syn::Expr::Paren(parenthesized) => expression = &parenthesized.expr,
            syn::Expr::Path(path) => {
                let Some(segment) = path.path.segments.last() else {
                    return false;
                };
                let name = segment.ident.to_string();
                let normalized = name.strip_prefix("r#").unwrap_or(&name);
                return normalized == target;
            }
            _ => return false,
        }
    }
}

fn ident_hits_in_file(file: &syn::File, targets: &[&str]) -> Vec<String> {
    let mut visitor = TargetIdentVisitor {
        targets,
        hits: Vec::new(),
    };
    syn::visit::visit_file(&mut visitor, file);
    visitor.hits
}

fn ident_hits_in_source(source: &str, targets: &[&str]) -> Vec<String> {
    let file = syn::parse_file(source).unwrap_or_else(|error| panic!("parse source: {error}"));
    ident_hits_in_file(&file, targets)
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn collect_source_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("production source directory reads") {
        let path = entry.expect("production source entry reads").path();
        if path.is_dir() {
            collect_source_files(&path, files);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn crate_src_directories(exclude: &[&str]) -> Vec<(String, PathBuf)> {
    let crates_dir = repository_root().join("core/crates");
    let mut crates = fs::read_dir(&crates_dir)
        .expect("core/crates reads")
        .map(|entry| entry.expect("core/crates entry reads").path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    crates.sort();
    crates
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_owned();
            if exclude.iter().any(|skip| *skip == name) {
                return None;
            }
            let src = path.join("src");
            src.is_dir().then_some((name, src))
        })
        .collect()
}

fn relative_crate_path(crate_name: &str, src_root: &Path, file: &Path) -> String {
    let rest = file
        .strip_prefix(src_root)
        .expect("source file lives under crate src");
    let rest = rest
        .iter()
        .map(|component| component.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    format!("{crate_name}/src/{rest}")
}

fn relative_from(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .expect("source file lives under scan root")
        .iter()
        .map(|component| component.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn item_attributes(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(item) => &item.attrs,
        syn::Item::Enum(item) => &item.attrs,
        syn::Item::ExternCrate(item) => &item.attrs,
        syn::Item::Fn(item) => &item.attrs,
        syn::Item::ForeignMod(item) => &item.attrs,
        syn::Item::Impl(item) => &item.attrs,
        syn::Item::Macro(item) => &item.attrs,
        syn::Item::Mod(item) => &item.attrs,
        syn::Item::Static(item) => &item.attrs,
        syn::Item::Struct(item) => &item.attrs,
        syn::Item::Trait(item) => &item.attrs,
        syn::Item::TraitAlias(item) => &item.attrs,
        syn::Item::Type(item) => &item.attrs,
        syn::Item::Union(item) => &item.attrs,
        syn::Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

fn attribute_is_test(attribute: &syn::Attribute) -> bool {
    if attribute.path().is_ident("test") {
        return true;
    }
    if !attribute.path().is_ident("cfg") {
        return false;
    }
    attribute
        .parse_args::<syn::Meta>()
        .ok()
        .is_some_and(|meta| meta.path().is_ident("test"))
}

fn strip_test_items(items: Vec<syn::Item>) -> Vec<syn::Item> {
    items
        .into_iter()
        .filter_map(|mut item| {
            if item_attributes(&item).iter().any(attribute_is_test) {
                return None;
            }
            if let syn::Item::Mod(module) = &mut item
                && let Some((_, nested)) = &mut module.content
            {
                *nested = strip_test_items(std::mem::take(nested));
            }
            Some(item)
        })
        .collect()
}

fn classified_production_file(path: &Path, source: &str, targets: &[&str]) -> Option<syn::File> {
    match super::bound_read_race_closure::production_file(source) {
        Ok(file) => Some(file),
        Err(error) => {
            let parsed = syn::parse_file(source)
                .unwrap_or_else(|parse_error| panic!("parse {}: {parse_error}", path.display()));
            let mut stripped = parsed;
            stripped.items = strip_test_items(stripped.items);
            let hits = ident_hits_in_file(&stripped, targets);
            if hits.is_empty() {
                None
            } else {
                panic!(
                    "{}: production classifier failed ({error}) while target identifiers are present: {hits:?}",
                    path.display()
                );
            }
        }
    }
}

fn walk_functions(
    items: &[syn::Item],
    visit: &mut impl FnMut(String, &mut dyn FnMut(&mut CallSiteVisitor<'_>)),
) {
    for item in items {
        match item {
            syn::Item::Fn(function) => {
                let name = function.sig.ident.to_string();
                visit(name, &mut |visitor| {
                    syn::visit::visit_item_fn(visitor, function);
                });
            }
            syn::Item::Impl(implementation) => {
                for impl_item in &implementation.items {
                    if let syn::ImplItem::Fn(function) = impl_item {
                        let name = function.sig.ident.to_string();
                        visit(name, &mut |visitor| {
                            syn::visit::visit_impl_item_fn(visitor, function);
                        });
                    }
                }
            }
            syn::Item::Mod(module) => {
                if let Some((_, nested)) = &module.content {
                    walk_functions(nested, visit);
                }
            }
            _ => {}
        }
    }
}

fn call_sites_in_file(file: &syn::File, target: &str) -> (Vec<String>, usize) {
    let mut names = Vec::new();
    let mut expr_calls = 0;
    walk_functions(&file.items, &mut |name, visit_fn| {
        let mut visitor = CallSiteVisitor { target, count: 0 };
        visit_fn(&mut visitor);
        if visitor.count > 0 {
            names.push(name);
            expr_calls += visitor.count;
        }
    });
    (names, expr_calls)
}

fn scan_files(
    files: &[(String, PathBuf, String)],
    target: &str,
) -> (BTreeSet<String>, BTreeSet<(String, String)>, usize) {
    let targets = [target];
    let mut files_with_ident = BTreeSet::new();
    let mut functions = BTreeSet::new();
    let mut expr_calls = 0;
    for (relative, path, source) in files {
        let Some(file) = classified_production_file(path, source, &targets) else {
            continue;
        };
        if !ident_hits_in_file(&file, &targets).is_empty() {
            files_with_ident.insert(relative.clone());
        }
        let (names, calls) = call_sites_in_file(&file, target);
        expr_calls += calls;
        for name in names {
            functions.insert((relative.clone(), name));
        }
    }
    (files_with_ident, functions, expr_calls)
}

fn load_root_files(root: &Path) -> Vec<(String, PathBuf, String)> {
    let mut files = Vec::new();
    collect_source_files(root, &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            (relative_from(root, &path), path, source)
        })
        .collect()
}

fn load_workspace_files(exclude: &[&str]) -> Vec<(String, PathBuf, String)> {
    let mut loaded = Vec::new();
    for (crate_name, src) in crate_src_directories(exclude) {
        let mut files = Vec::new();
        collect_source_files(&src, &mut files);
        files.sort();
        for path in files {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            loaded.push((relative_crate_path(&crate_name, &src, &path), path, source));
        }
    }
    loaded
}

#[test]
fn bind_stream_scan_does_not_match_near_miss_identifiers() {
    let source = "fn probe() { bind_named_stream(j, d, s, n, c, src, h); bind_paired_stream(j, d, s, b, c, src, h); bind_ingest_stream(); }";
    assert!(ident_hits_in_source(source, &[BIND_STREAM]).is_empty());
}

#[test]
fn bind_paired_stream_callers_outside_segment_are_exactly_ingest_stream_identity() {
    let files = load_workspace_files(&[SEGMENT_CRATE]);
    let (hits, _, _) = scan_files(&files, BIND_PAIRED_STREAM);
    assert_eq!(hits, BTreeSet::from([INGEST_BIND_PATH.to_owned()]));
}

#[test]
fn bind_stream_has_no_callers_outside_segment() {
    let files = load_workspace_files(&[SEGMENT_CRATE]);
    let (hits, _, _) = scan_files(&files, BIND_STREAM);
    assert!(
        hits.is_empty(),
        "unexpected bind_stream callers outside solstone-core-segment: {hits:?}"
    );
}

#[test]
fn discovers_an_injected_bind_paired_stream_call_in_a_fixture_tree() {
    let root = tempfile::tempdir().expect("fixture root creates");
    let nested = root.path().join("device/ingest");
    fs::create_dir_all(&nested).expect("nested fixture directory creates");
    fs::write(
        nested.join("bind.rs"),
        "fn bind_device_stream() {\n    bind_paired_stream(journal, day, segment, &base, cid, source, hints);\n}\n",
    )
    .expect("nested fixture writes");

    let files = load_root_files(root.path());
    let (hits, sites, expr_calls) = scan_files(&files, BIND_PAIRED_STREAM);
    assert!(
        hits.iter().any(|path| path.ends_with("bind.rs")),
        "injected bind_paired_stream call must be discovered: {hits:?}"
    );
    assert_eq!(
        sites,
        BTreeSet::from([(
            "device/ingest/bind.rs".to_owned(),
            "bind_device_stream".to_owned()
        )])
    );
    assert_eq!(expr_calls, 1);
}

#[test]
fn pairing_bundle_pin_matches_the_shipped_bundle() {
    assert_eq!(super::pairing_contract_bundle::bundle_semver(), "1.0.0");
    assert_eq!(
        super::pairing_contract_bundle::authority_digest(),
        "34a0ca85485e7fbdeb8397fb33a1d0fb6e6d3845d85d0a7f8219dfd335affdda"
    );
}

#[test]
fn allocator_import_reference_is_pinned() {
    let ident = ALLOCATOR_IMPORT
        .rsplit_once("::")
        .map(|(_, name)| name)
        .expect("ALLOCATOR_IMPORT is a crate path");
    let files = load_workspace_files(&[SEGMENT_CRATE]);
    let (hits, _, expr_calls) = scan_files(&files, ident);
    assert_eq!(ident, BIND_PAIRED_STREAM);
    assert_eq!(hits, BTreeSet::from([INGEST_BIND_PATH.to_owned()]));
    assert!(
        expr_calls > 0,
        "ALLOCATOR_IMPORT ident {ident:?} must match production calls in {INGEST_BIND_PATH}"
    );
}

#[test]
fn fixed_source_vectors_are_the_ac21_pair() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/stream-name-projection-vectors.json"
    ))
    .expect("stream-name projection fixture parses");
    let project = fixture["project"]
        .as_array()
        .expect("fixture has a project array");
    let empty = project
        .iter()
        .find(|vector| {
            vector["label"].as_str() == Some("studio-mac")
                && vector["source"].as_str() == Some(FIXED_SOURCE_VECTORS[0])
        })
        .expect("studio-mac empty-source AC21 vector exists");
    assert_eq!(empty["expect"].as_str(), Some("studio-mac"));
    let tmux = project
        .iter()
        .find(|vector| {
            vector["label"].as_str() == Some("studio-mac")
                && vector["source"].as_str() == Some(FIXED_SOURCE_VECTORS[1])
        })
        .expect("studio-mac tmux AC21 vector exists");
    assert_eq!(tmux["expect"].as_str(), Some("studio-mac_tmux"));
}
