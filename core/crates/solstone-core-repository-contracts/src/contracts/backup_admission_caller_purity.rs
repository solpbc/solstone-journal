// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct ForbiddenIdentVisitor {
    hits: Vec<String>,
}

impl ForbiddenIdentVisitor {
    fn record_forbidden_identifier(&mut self, raw: &str) {
        let normalized = raw.strip_prefix("r#").unwrap_or(raw);
        if normalized == "run_backup" || normalized == "record_backup_error" {
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

fn forbidden_hits_in_source(source: &str) -> Vec<String> {
    let file =
        syn::parse_file(source).unwrap_or_else(|error| panic!("parse fixture source: {error}"));
    let mut visitor = ForbiddenIdentVisitor::default();
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

fn load_production_root(root: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    if !root.is_dir() {
        return Err(format!(
            "production root does not exist or is not a directory: {}",
            root.display()
        ));
    }
    let mut files = Vec::new();
    collect_source_files(root, &mut files);
    if files.is_empty() {
        return Err(format!(
            "production root contains no .rs files: {}",
            root.display()
        ));
    }
    files.sort();
    Ok(files
        .into_iter()
        .map(|path| {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            (path, source)
        })
        .collect())
}

fn load_all_roots(roots: &[PathBuf]) -> Result<Vec<(PathBuf, String)>, String> {
    let mut sources = Vec::new();
    for root in roots {
        sources.extend(load_production_root(root)?);
    }
    Ok(sources)
}

#[test]
fn detects_bare_forbidden_identifier() {
    assert!(!forbidden_hits_in_source("fn probe() { run_backup(journal, services); }").is_empty());
}

#[test]
fn detects_module_qualified_forbidden_identifier() {
    assert!(
        !forbidden_hits_in_source("fn probe() { engine::run_backup(journal, services); }")
            .is_empty()
    );
}

#[test]
fn detects_fully_qualified_forbidden_identifier() {
    assert!(
        !forbidden_hits_in_source(
            "fn probe() { solstone_core_backup_runtime::run_backup(journal, services); }"
        )
        .is_empty()
    );
}

#[test]
fn detects_import_aliased_forbidden_identifier() {
    assert!(
        !forbidden_hits_in_source(
            "use solstone_core_backup_runtime::run_backup as legacy;\nfn probe() { legacy(journal, services); }"
        )
        .is_empty()
    );
}

#[test]
fn detects_let_bound_qualified_forbidden_identifier() {
    assert!(
        !forbidden_hits_in_source(
            "fn probe() { let legacy = solstone_core_backup_runtime::run_backup; legacy(journal, services); }"
        )
        .is_empty()
    );
}

#[test]
fn detects_parenthesized_import_alias_forbidden_identifier() {
    assert!(
        !forbidden_hits_in_source(
            "use solstone_core_backup_runtime::run_backup as legacy;\nfn probe() { (legacy)(journal, services); }"
        )
        .is_empty()
    );
}

#[test]
fn detects_raw_direct_run_backup_identifier() {
    assert_eq!(
        forbidden_hits_in_source("fn probe() { r#run_backup(journal, services); }"),
        ["run_backup"]
    );
}

#[test]
fn detects_raw_direct_record_backup_error_identifier() {
    assert_eq!(
        forbidden_hits_in_source("fn probe() { r#record_backup_error(journal, clock, reason); }"),
        ["record_backup_error"]
    );
}

#[test]
fn detects_raw_qualified_run_backup_identifier() {
    assert_eq!(
        forbidden_hits_in_source(
            "fn probe() { solstone_core_backup_runtime::r#run_backup(journal, services); }"
        ),
        ["run_backup"]
    );
}

#[test]
fn detects_raw_qualified_record_backup_error_identifier() {
    assert_eq!(
        forbidden_hits_in_source(
            "fn probe() { solstone_core_backup_runtime::r#record_backup_error(journal, clock, reason); }"
        ),
        ["record_backup_error"]
    );
}

#[test]
fn detects_forbidden_identifier_in_unexpanded_macro_rules_body() {
    assert_eq!(
        forbidden_hits_in_source("macro_rules! probe { () => { run_backup }; }"),
        ["run_backup"]
    );
}

#[test]
fn detects_forbidden_identifier_in_macro_argument_token_stream() {
    assert_eq!(
        forbidden_hits_in_source("fn probe() { some_macro!(record_backup_error); }"),
        ["record_backup_error"]
    );
}

#[test]
fn detects_raw_forbidden_identifier_in_nested_macro_group() {
    assert_eq!(
        forbidden_hits_in_source("fn probe() { some_macro!({ nested!(r#run_backup) }); }"),
        ["run_backup"]
    );
}

#[test]
fn detects_forbidden_identifier_in_attribute_meta_tokens() {
    assert_eq!(
        forbidden_hits_in_source("#[probe(run_backup)]\nfn probe() {}"),
        ["run_backup"]
    );
}

#[test]
fn detects_raw_forbidden_identifier_in_nested_attribute_meta_tokens() {
    assert_eq!(
        forbidden_hits_in_source("#[probe(nested(r#record_backup_error))]\nfn probe() {}"),
        ["record_backup_error"]
    );
}

#[test]
fn ignores_unrelated_macro_token_stream() {
    assert!(
        forbidden_hits_in_source("fn probe() { some_macro!({ unrelated_identifier }); }")
            .is_empty()
    );
}

#[test]
fn ignores_unrelated_attribute_meta_tokens() {
    assert!(
        forbidden_hits_in_source("#[probe(nested(unrelated_identifier))]\nfn probe() {}")
            .is_empty()
    );
}

#[test]
fn ignores_unrelated_and_near_miss_identifiers() {
    let source = "fn probe() {\n    resolve_tools(&capability, runner, downloader, dirs);\n    capability.execute(&services);\n    backup_run_result(result);\n}";
    assert!(forbidden_hits_in_source(source).is_empty());
}

#[test]
fn discovers_forbidden_identifier_in_nested_source_file() {
    let root = tempfile::tempdir().expect("fixture root creates");
    let nested = root.path().join("sub/dir");
    fs::create_dir_all(&nested).expect("nested fixture directory creates");
    fs::write(
        nested.join("nested.rs"),
        "fn probe() { solstone_core_backup_runtime::run_backup(journal, services); }",
    )
    .expect("nested fixture writes");

    let files = load_production_root(root.path()).expect("nested source root loads");
    let hits: Vec<_> = files
        .iter()
        .flat_map(|(_, source)| forbidden_hits_in_source(source))
        .collect();
    assert_eq!(hits, ["run_backup"]);
}

#[test]
fn discovers_forbidden_identifier_in_nested_tests_rs_source_file() {
    let root = tempfile::tempdir().expect("fixture root creates");
    let nested = root.path().join("sub/dir");
    fs::create_dir_all(&nested).expect("nested fixture directory creates");
    fs::write(
        nested.join("tests.rs"),
        "fn probe() { solstone_core_backup_runtime::run_backup(journal, services); }",
    )
    .expect("tests fixture writes");
    fs::write(nested.join("harmless.rs"), "fn harmless() {}").expect("harmless fixture writes");

    let files = load_production_root(root.path()).expect("nested source root loads");
    let hits: Vec<_> = files
        .iter()
        .flat_map(|(_, source)| forbidden_hits_in_source(source))
        .collect();
    assert_eq!(hits, ["run_backup"]);
}

#[test]
fn rejects_empty_production_root() {
    let root = tempfile::tempdir().expect("empty fixture root creates");
    assert!(load_production_root(root.path()).is_err());
}

#[test]
fn rejects_missing_production_root() {
    let parent = tempfile::tempdir().expect("fixture parent creates");
    let missing = parent.path().join("missing");
    assert!(load_production_root(&missing).is_err());
}

#[test]
fn mixed_roots_fail_for_the_empty_root() {
    let parent = tempfile::tempdir().expect("fixture parent creates");
    let populated = parent.path().join("populated");
    fs::create_dir_all(&populated).expect("populated root creates");
    fs::write(populated.join("lib.rs"), "fn harmless() {}").expect("harmless fixture writes");
    let empty = parent.path().join("empty");
    fs::create_dir_all(&empty).expect("empty root creates");

    let error = load_all_roots(&[populated, empty.clone()]).expect_err("empty root rejects");
    assert!(error.contains(&empty.display().to_string()));
}

#[test]
fn backup_entry_crates_contain_no_legacy_backup_runtime_identifiers() {
    let root = repository_root();
    let sources = load_all_roots(&[
        root.join("core/crates/solstone-core-backup-cli/src"),
        root.join("core/crates/solstone-core-maintenance/src"),
    ])
    .unwrap_or_else(|error| panic!("load production backup entry sources: {error}"));
    let hits: Vec<_> = sources
        .iter()
        .flat_map(|(_, source)| forbidden_hits_in_source(source))
        .collect();
    assert!(
        hits.is_empty(),
        "backup entry production sources contain forbidden legacy identifiers: {hits:?}"
    );
}
