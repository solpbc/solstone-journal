// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One owner for the layout of facet link folders.
//!
//! `facets/<facet>/entities/<folder>/` holds an entity's link into a facet and
//! its notes there. Where a link lives, and how two links to one entity
//! combine, is decided in one module; every other crate goes through it. This
//! contract fails on production source outside that module that spells a path
//! into a link folder itself -- in a string, in a formatted string, or in a
//! chain of `.join(...)` calls.

use std::fs;
use std::path::{Path, PathBuf};

/// The module that owns the layout.
const OWNER: &str = "core/crates/solstone-core-entity/src/store/facet_links.rs";

/// Read-only derived readers that match link-folder paths by pattern and need
/// no link semantics, so going through the owner would add a dependency and
/// nothing else. Each names its reason.
const DERIVED_READERS: &[(&str, &str)] = &[
    (
        "core/crates/solstone-core-format/src/content/mod.rs",
        "the index's content classifier globs notes files by path",
    ),
    (
        "core/crates/solstone-core-indexer/src/edges/registry.rs",
        "the index's edge registry globs notes files by path",
    ),
];

const LINK_FILES: &[&str] = &["entity.json", "observations.jsonl"];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

/// Whether a string spells a path into a facet's link folders.
fn names_a_link_folder_path(text: &str) -> bool {
    let Some(facets) = text.find("facets/") else {
        return false;
    };
    let rest = &text[facets..];
    let Some(entities) = rest.find("/entities/") else {
        return false;
    };
    let after = &rest[entities + "/entities/".len()..];
    // A placeholder that names a folder, not a day file beside the folders.
    let placeholder_folder = after.starts_with('{')
        && after
            .find('}')
            .is_some_and(|close| matches!(after[close + 1..].chars().next(), None | Some('/')));
    placeholder_folder || LINK_FILES.iter().any(|file| after.contains(file))
}

#[derive(Default)]
struct LayoutVisitor {
    hits: Vec<String>,
}

impl LayoutVisitor {
    fn literal(&mut self, text: &str) {
        if names_a_link_folder_path(text) {
            self.hits.push(text.to_owned());
        }
    }

    fn tokens(&mut self, tokens: proc_macro2::TokenStream) {
        for token in tokens {
            match token {
                proc_macro2::TokenTree::Literal(literal) => {
                    if let Ok(syn::Lit::Str(text)) =
                        syn::parse_str::<syn::Lit>(&literal.to_string())
                    {
                        self.literal(&text.value());
                    }
                }
                proc_macro2::TokenTree::Group(group) => self.tokens(group.stream()),
                proc_macro2::TokenTree::Ident(_) | proc_macro2::TokenTree::Punct(_) => {}
            }
        }
    }
}

fn is_test_only(attributes: &[syn::Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("test")
            || (attribute.path().is_ident("cfg")
                && attribute
                    .meta
                    .require_list()
                    .is_ok_and(|list| names_test_outside_not(list.tokens.clone())))
    })
}

/// Whether a `cfg` predicate requires `test`: the `test` flag appears, and not
/// inside a `not(...)`.
fn names_test_outside_not(tokens: proc_macro2::TokenStream) -> bool {
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            // `not(...)` never requires test, and `any(...)` can hold without it.
            proc_macro2::TokenTree::Ident(ident) if ident == "not" || ident == "any" => {
                if matches!(tokens.peek(), Some(proc_macro2::TokenTree::Group(_))) {
                    tokens.next();
                }
            }
            proc_macro2::TokenTree::Ident(ident) if ident == "test" => return true,
            proc_macro2::TokenTree::Group(group) if names_test_outside_not(group.stream()) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// The literal arguments of a `.join(...)` chain, innermost first.
fn join_chain(expression: &syn::Expr, parts: &mut Vec<String>) {
    if let syn::Expr::MethodCall(call) = expression {
        join_chain(&call.receiver, parts);
        if call.method == "join"
            && let Some(syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(text),
                ..
            })) = call.args.first()
        {
            parts.push(text.value());
        }
    }
}

impl<'ast> syn::visit::Visit<'ast> for LayoutVisitor {
    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if !is_test_only(&module.attrs) {
            syn::visit::visit_item_mod(self, module);
        }
    }

    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        if !is_test_only(&function.attrs) {
            syn::visit::visit_item_fn(self, function);
        }
    }

    fn visit_lit_str(&mut self, text: &'ast syn::LitStr) {
        self.literal(&text.value());
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.tokens(mac.tokens.clone());
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if call.method == "join" {
            let mut parts = Vec::new();
            join_chain(&syn::Expr::MethodCall(call.clone()), &mut parts);
            let joined = parts.join("/");
            if joined.contains("facets")
                && joined.contains("entities")
                && LINK_FILES
                    .iter()
                    .any(|file| parts.iter().any(|part| part == file))
            {
                self.hits.push(format!(".join chain: {joined}"));
            }
        }
        syn::visit::visit_expr_method_call(self, call);
    }
}

/// How many link-folder paths the path-shape check finds in a source file.
pub(crate) fn path_shape_hits(source: &str) -> usize {
    let file = syn::parse_file(source).expect("parse source");
    let mut visitor = LayoutVisitor::default();
    syn::visit::visit_file(&mut visitor, &file);
    visitor.hits.len()
}

fn production_sources(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let crates = root.join("core/crates");
    for krate in fs::read_dir(&crates).expect("read crates") {
        let source = krate.expect("crate entry").path().join("src");
        if source.is_dir() {
            collect_rust(&source, &mut files);
        }
    }
    files.sort();
    files
}

fn collect_rust(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("read source directory") {
        let path = entry.expect("source entry").path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if path.is_dir() {
            if name != "tests" && name != "fixtures" {
                collect_rust(&path, files);
            }
        } else if name.ends_with(".rs")
            && !name.ends_with("_tests.rs")
            && name != "tests.rs"
            && name != "test_support.rs"
        {
            files.push(path);
        }
    }
}

#[test]
fn only_the_link_folder_owner_spells_a_path_into_a_link_folder() {
    let root = repository_root();
    let allowed: Vec<PathBuf> = std::iter::once(OWNER)
        .chain(DERIVED_READERS.iter().map(|(path, _)| *path))
        .map(|path| root.join(path))
        .collect();
    for path in &allowed {
        assert!(
            path.exists(),
            "allowlisted file is gone: {}",
            path.display()
        );
    }
    let mut violations = Vec::new();
    for path in production_sources(&root) {
        if allowed.contains(&path) {
            continue;
        }
        let source = fs::read_to_string(&path).expect("read source");
        let file = match syn::parse_file(&source) {
            Ok(file) => file,
            Err(error) => {
                violations.push(format!(
                    "{}: can't be read as Rust, so it can't be checked: {error}",
                    path.strip_prefix(&root).unwrap_or(&path).display()
                ));
                continue;
            }
        };
        let mut visitor = LayoutVisitor::default();
        syn::visit::visit_file(&mut visitor, &file);
        for hit in visitor.hits {
            violations.push(format!(
                "{}: {hit}",
                path.strip_prefix(&root).unwrap_or(&path).display()
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "these build facet link-folder paths outside {OWNER}; use its `LinkDirs` instead:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_contract_sees_every_way_a_path_is_spelled() {
    let source = r#"
        fn a(journal: &Path, facet: &str, dir: &str) {
            let _ = format!("facets/{facet}/entities/{dir}/entity.json");
            let _ = "facets/work/entities/ada/observations.jsonl";
            let _ = journal.join("facets").join(facet).join("entities").join(dir).join("entity.json");
        }
        #[cfg(test)]
        mod tests {
            fn b() { let _ = "facets/work/entities/ada/entity.json"; }
        }
        #[cfg(all(test, not(target_os = "ios")))]
        mod platform_tests {
            fn e() { let _ = "facets/work/entities/ada/entity.json"; }
        }
        #[cfg(not(test))]
        mod shipped {
            fn d() { let _ = "facets/work/entities/ada/entity.json"; }
        }
        fn c() {
            let _ = "facets/work/entities";
            let _ = "entities/ada/entity.json";
            let _ = format!("facets/{facet}/entities/{day}.jsonl");
        }
    "#;
    let file = syn::parse_file(source).expect("parse fixture");
    let mut visitor = LayoutVisitor::default();
    syn::visit::visit_file(&mut visitor, &file);
    assert_eq!(visitor.hits.len(), 4, "{:?}", visitor.hits);
}
