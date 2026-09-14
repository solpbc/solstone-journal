// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct ForbiddenIdentVisitor {
    forbidden: Vec<&'static str>,
    hits: Vec<String>,
}

impl ForbiddenIdentVisitor {
    fn new(forbidden: Vec<&'static str>) -> Self {
        Self {
            forbidden,
            hits: Vec::new(),
        }
    }

    fn record_forbidden_identifier(&mut self, raw: &str) {
        let normalized = raw.strip_prefix("r#").unwrap_or(raw);
        if self.forbidden.iter().any(|&f| f == normalized) {
            self.hits.push(normalized.to_owned());
        }
    }

    fn visit_opaque_token_stream(&mut self, tokens: proc_macro2::TokenStream) {
        for token in tokens {
            match token {
                proc_macro2::TokenTree::Ident(ident) => {
                    self.record_forbidden_identifier(&ident.to_string());
                }
                proc_macro2::TokenTree::Group(group) => {
                    self.visit_opaque_token_stream(group.stream());
                }
                proc_macro2::TokenTree::Literal(_) | proc_macro2::TokenTree::Punct(_) => {}
            }
        }
    }
}

impl<'ast> syn::visit::Visit<'ast> for ForbiddenIdentVisitor {
    fn visit_ident(&mut self, ident: &'ast syn::Ident) {
        self.record_forbidden_identifier(&ident.to_string());
    }

    fn visit_token_stream(&mut self, tokens: &'ast proc_macro2::TokenStream) {
        self.visit_opaque_token_stream(tokens.clone());
    }
}

fn forbidden_hits_in_source(source: &str, forbidden: &[&'static str]) -> Vec<String> {
    let file =
        syn::parse_file(source).unwrap_or_else(|error| panic!("parse fixture source: {error}"));
    let mut visitor = ForbiddenIdentVisitor::new(forbidden.to_vec());
    syn::visit::visit_file(&mut visitor, &file);
    visitor.hits
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn collect_rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    if !directory.exists() {
        return;
    }
    for entry in fs::read_dir(directory).expect("source directory reads") {
        let path = entry.expect("source entry reads").path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

#[test]
fn facet_read_modules_never_mention_writers_or_allocators() {
    let root = repository_root();
    let read_modules = [
        root.join("core/crates/solstone-core-facets/src/store/declaration.rs"),
        root.join("core/crates/solstone-core-facets/src/store/map.rs"),
    ];

    let forbidden = [
        "save_facet_declaration",
        "allocate_facet_id",
        "allocate_facet_id_locked",
        "assign_new_facet_id",
        "assign_new_facet_id_locked",
        "backfill_facet_ids",
        "create_facet",
        "update_facet",
        "delete_facet",
    ];

    for module_path in &read_modules {
        assert!(
            module_path.exists(),
            "facet read module does not exist: {}",
            module_path.display()
        );
        let source = fs::read_to_string(module_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", module_path.display()));
        let hits = forbidden_hits_in_source(&source, &forbidden);
        assert!(
            hits.is_empty(),
            "facet read module {} mentioned forbidden writers/allocators: {:?}",
            module_path.display(),
            hits
        );
    }
}

#[test]
fn save_facet_declaration_only_in_write_and_facet_id() {
    let root = repository_root();
    let mut rust_files = Vec::new();
    collect_rust_files(&root.join("core/crates"), &mut rust_files);

    let allowed_files = [
        root.join("core/crates/solstone-core-facets/src/store/write.rs"),
        root.join("core/crates/solstone-core-facets/src/store/facet_id.rs"),
    ];

    for file_path in rust_files {
        // Skip this contract file itself
        if file_path.ends_with("facet_read_purity.rs") {
            continue;
        }
        let source = fs::read_to_string(&file_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", file_path.display()));
        let hits = forbidden_hits_in_source(&source, &["save_facet_declaration"]);
        if allowed_files.contains(&file_path) {
            assert!(
                !hits.is_empty(),
                "expected save_facet_declaration in allowed file {}",
                file_path.display()
            );
        } else {
            assert!(
                hits.is_empty(),
                "unauthorized call to save_facet_declaration in {}: {:?}",
                file_path.display(),
                hits
            );
        }
    }
}
