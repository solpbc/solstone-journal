// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The writer lease's operation inventory defines the producer boundary.

use quote::ToTokens;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::{self, Visit};

fn without_test(meta: &syn::Meta) -> Option<bool> {
    match meta {
        syn::Meta::Path(path) if path.is_ident("test") => Some(false),
        syn::Meta::List(list) => {
            use syn::parse::Parser;
            let nested = syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
                .parse2(list.tokens.clone())
                .ok()?;
            let values = nested.iter().map(without_test).collect::<Vec<_>>();
            if list.path.is_ident("all") {
                if values.contains(&Some(false)) {
                    Some(false)
                } else if values.iter().all(|v| *v == Some(true)) {
                    Some(true)
                } else {
                    None
                }
            } else if list.path.is_ident("any") {
                if values.contains(&Some(true)) {
                    Some(true)
                } else if values.iter().all(|v| *v == Some(false)) {
                    Some(false)
                } else {
                    None
                }
            } else if list.path.is_ident("not") && values.len() == 1 {
                values[0].map(|v| !v)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if let syn::Meta::List(list) = &attr.meta
            && list.path.is_ident("cfg")
        {
            return syn::parse2::<syn::Meta>(list.tokens.clone())
                .is_ok_and(|meta| without_test(&meta) == Some(false));
        }
        false
    })
}

struct Calls<'a> {
    operations: &'a BTreeSet<String>,
    aliases: BTreeMap<String, String>,
    function: String,
    allowed: bool,
    command_file: bool,
    violations: Vec<String>,
}

impl Calls<'_> {
    fn check(&mut self, name: &str, line: usize) {
        let original = self.aliases.get(name).map_or(name, String::as_str);
        if !self.allowed && self.operations.contains(original) {
            self.violations
                .push(format!("{}:{line}: {original}", self.function));
        }
    }
}

impl<'ast> Visit<'ast> for Calls<'_> {
    fn visit_file(&mut self, node: &'ast syn::File) {
        // Rust imports are in scope even when written after their callers.
        for item in &node.items {
            if let syn::Item::Use(import) = item {
                self.visit_item_use(import);
            }
        }
        visit::visit_file(self, node);
    }
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !test_only(&node.attrs) {
            if let Some((_, items)) = &node.content {
                for item in items {
                    if let syn::Item::Use(import) = item {
                        self.visit_item_use(import);
                    }
                }
            }
            visit::visit_item_mod(self, node);
        }
    }
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if test_only(&node.attrs) {
            return;
        }
        let before = self.function.clone();
        self.function = node.sig.ident.to_string();
        let allowed = self.allowed;
        self.allowed |= self.command_file
            && (self.function == "indexer"
                || self.function.starts_with("indexer_")
                || self.function.starts_with("run_indexer"));
        visit::visit_item_fn(self, node);
        self.function = before;
        self.allowed = allowed;
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if test_only(&node.attrs) {
            return;
        }
        let before = std::mem::replace(&mut self.function, node.sig.ident.to_string());
        visit::visit_impl_item_fn(self, node);
        self.function = before;
    }
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if !test_only(&node.attrs) {
            visit::visit_item_impl(self, node);
        }
    }
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        fn aliases(tree: &syn::UseTree, names: &mut BTreeMap<String, String>) {
            match tree {
                syn::UseTree::Path(path) => aliases(&path.tree, names),
                syn::UseTree::Rename(rename) => {
                    names.insert(rename.rename.to_string(), rename.ident.to_string());
                }
                syn::UseTree::Group(group) => {
                    for item in &group.items {
                        aliases(item, names);
                    }
                }
                _ => {}
            }
        }
        if !test_only(&node.attrs) {
            aliases(&node.tree, &mut self.aliases);
            let text = node.to_token_stream().to_string();
            if text.contains("indexer_store") && text.contains("drain") {
                self.aliases.insert("drain".into(), "drain".into());
            }
            if !self.allowed && text.contains("indexer_store") && text.contains('*') {
                self.violations
                    .push("wildcard import hides index writer bindings".into());
            }
        }
    }
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = node.func.as_ref()
            && let Some(last) = path.path.segments.last()
        {
            let name = last.ident.to_string();
            let writer_drain = path.path.segments.iter().any(|segment| {
                let part = segment.ident.to_string();
                part.contains("indexer_store")
                    || part == "todo"
                    || self
                        .aliases
                        .get(&part)
                        .is_some_and(|original| original == "todo" || original == "drain")
            });
            if name != "drain" || writer_drain {
                self.check(&name, last.ident.span().start().line);
            }
        }
        visit::visit_expr_call(self, node);
    }
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        // The queue's free function shares its name with collection iterators.
        if node.method != "drain" {
            self.check(&node.method.to_string(), node.method.span().start().line);
        }
        visit::visit_expr_method_call(self, node);
    }
}

fn scan_file(
    path: &Path,
    operations: &BTreeSet<String>,
    seen: &mut BTreeSet<PathBuf>,
    violations: &mut Vec<String>,
) {
    let path = path
        .canonicalize()
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    if !seen.insert(path.clone()) {
        return;
    }
    let source = fs::read_to_string(&path).unwrap();
    let parsed =
        syn::parse_file(&source).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let store = path
        .components()
        .any(|part| part.as_os_str() == "solstone-core-indexer-store");
    let mobile = path
        .components()
        .any(|part| part.as_os_str() == "mobile-serving");
    let command_file = path.ends_with("solstone-core-journal-cli/src/local_ops.rs")
        || path.ends_with("solstone-core/src/main.rs");
    let mut visitor = Calls {
        operations,
        aliases: BTreeMap::new(),
        function: String::new(),
        allowed: store || mobile,
        command_file,
        violations: Vec::new(),
    };
    visitor.visit_file(&parsed);
    violations.extend(
        visitor
            .violations
            .into_iter()
            .map(|finding| format!("{}:{finding}", path.display())),
    );
    let directory = match path.file_stem().and_then(|stem| stem.to_str()) {
        Some("lib" | "main" | "mod") => path.parent().unwrap().to_owned(),
        Some(stem) => path.parent().unwrap().join(stem),
        _ => unreachable!(),
    };
    fn modules(
        items: &[syn::Item],
        directory: &Path,
        operations: &BTreeSet<String>,
        seen: &mut BTreeSet<PathBuf>,
        violations: &mut Vec<String>,
    ) {
        for item in items {
            let syn::Item::Mod(module) = item else {
                continue;
            };
            if test_only(&module.attrs) {
                continue;
            }
            if let Some((_, nested)) = &module.content {
                modules(
                    nested,
                    &directory.join(module.ident.to_string()),
                    operations,
                    seen,
                    violations,
                );
                continue;
            }
            let explicit = module.attrs.iter().find_map(|attr| {
                if let syn::Meta::NameValue(value) = &attr.meta
                    && value.path.is_ident("path")
                    && let syn::Expr::Lit(expr) = &value.value
                    && let syn::Lit::Str(path) = &expr.lit
                {
                    return Some(directory.join(path.value()));
                }
                None
            });
            let leaf = directory.join(format!("{}.rs", module.ident));
            let child = explicit.unwrap_or_else(|| {
                if leaf.is_file() {
                    leaf
                } else {
                    directory.join(module.ident.to_string()).join("mod.rs")
                }
            });
            // Generated build outputs have no checked-in source to inspect.
            if child.is_file() {
                scan_file(&child, operations, seen, violations);
            }
        }
    }
    modules(&parsed.items, &directory, operations, seen, violations);
}

fn operation_inventory(root: &Path) -> BTreeSet<String> {
    let source = fs::read_to_string(
        root.join("core/crates/solstone-core-indexer-store/src/writer_admission.rs"),
    )
    .unwrap();
    let section = source
        .split("//! Admit (acquires index writer lease):")
        .nth(1)
        .expect("lease operation inventory")
        .split("//! Inside those callers")
        .next()
        .unwrap();
    let mut operations = section
        .lines()
        .filter_map(|line| {
            line.strip_prefix("//! - `")
                .and_then(|line| line.split('`').next())
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>();
    assert!(
        operations.contains("rescan_file") && operations.contains("prune_by_paths"),
        "the instrument must see the existing lease owners"
    );
    // Public checked wrappers are writers too; callers cannot bypass the gate
    // by choosing the cancellable spelling of a listed operation.
    for source in ["scan.rs", "todo.rs"] {
        let source = fs::read_to_string(
            root.join("core/crates/solstone-core-indexer-store/src")
                .join(source),
        )
        .unwrap();
        let parsed = syn::parse_file(&source).unwrap();
        for item in parsed.items {
            if let syn::Item::Fn(function) = item
                && matches!(function.vis, syn::Visibility::Public(_))
            {
                let body = function.block.to_token_stream().to_string();
                if body.contains("IndexAdmission :: acquire") {
                    operations.insert(function.sig.ident.to_string());
                }
            }
        }
    }
    operations
}

#[test]
fn producers_only_enqueue_search_work() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap();
    let operations = operation_inventory(root);
    let mut seen = BTreeSet::new();
    let mut violations = Vec::new();
    for package in fs::read_dir(root.join("core/crates")).unwrap() {
        let src = package.unwrap().path().join("src");
        for entry in [src.join("lib.rs"), src.join("main.rs")] {
            if entry.is_file() {
                scan_file(&entry, &operations, &mut seen, &mut violations);
            }
        }
        if let Ok(binaries) = fs::read_dir(src.join("bin")) {
            for binary in binaries {
                let path = binary.unwrap().path();
                if path.extension().is_some_and(|ext| ext == "rs") {
                    scan_file(&path, &operations, &mut seen, &mut violations);
                }
            }
        }
    }
    scan_file(
        &root.join("tools/mobile-serving/src/lib.rs"),
        &operations,
        &mut seen,
        &mut violations,
    );
    assert!(
        seen.len() > 100,
        "production module discovery was empty or incomplete"
    );
    assert!(
        violations.is_empty(),
        "only indexer commands and the mobile index host write SQLite; producers use solstone_core_indexer_store::todo::enqueue_path(journal, path) or enqueue_token instead:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_writer_detector_sees_aliases_and_exempts_only_test_code() {
    let operations = BTreeSet::from(["rescan_file".to_owned()]);
    let source = syn::parse_file(
        r#"
        use index_store::rescan_file as hidden_writer;
        fn save() { hidden_writer(); }
        #[cfg(test)] mod fixtures { fn seed() { rescan_file(); } }
        #[cfg(not(test))] fn production() { rescan_file(); }
    "#,
    )
    .unwrap();
    let mut visitor = Calls {
        operations: &operations,
        aliases: BTreeMap::new(),
        function: String::new(),
        allowed: false,
        command_file: false,
        violations: Vec::new(),
    };
    visitor.visit_file(&source);
    assert_eq!(visitor.violations.len(), 2);
    assert!(visitor.violations.iter().any(|v| v.starts_with("save:")));
    assert!(
        visitor
            .violations
            .iter()
            .any(|v| v.starts_with("production:"))
    );
}

#[test]
fn every_discovered_family_has_a_producer_or_is_explicitly_scan_only() {
    use solstone_core_format::content::{PatternRoot, patterns_for_root};
    use solstone_core_indexer_todo::producers::{CATCH_ALL_ONLY, PRODUCERS};
    let discovered = [PatternRoot::Structural, PatternRoot::DayRooted]
        .into_iter()
        .flat_map(patterns_for_root)
        .map(|pattern| format!("{:?}", pattern.family))
        .collect::<BTreeSet<_>>();
    assert!(
        discovered.len() > 10,
        "family discovery must see the existing content registry"
    );
    let covered = PRODUCERS
        .iter()
        .chain(CATCH_ALL_ONLY)
        .map(|(family, seam)| {
            assert!(
                !seam.is_empty(),
                "name the enqueue seam or why the app never writes this family"
            );
            format!("{family:?}")
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        discovered, covered,
        "add a producer that enqueues this family; scan-only is for material the app never writes"
    );
}
