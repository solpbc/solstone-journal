// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The trash folder is hidden: nothing that reads or walks the journal sees it,
//! and nothing writes into it but the trash itself and a whole-journal restore.
//!
//! Two source checks over every production Rust file (test modules and test
//! files are skipped):
//!
//! 1. **Only the listed files name the trash folder or a restore's set-aside
//!    folder** (`TRASH_DIR`, `BEFORE_RESTORE_PREFIX`, or either as a literal). Code that wants what is in the trash goes through
//!    `solstone-core-trash`.
//! 2. **A walk of the journal root skips the trash.** A directory listing whose
//!    argument is the journal root itself (`journal`, `journal_root`,
//!    `journal_path`, `journal_dir`) fails unless its file is listed below as a
//!    walk that already skips it.
//!
//! What this cannot catch: a root reached under another name or through a
//! helper, restic, and non-Rust code. The journal's path resolvers refuse a
//! path whose first component is the trash, which covers every read route that
//! takes an owner- or agent-supplied path.

use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;

/// Files allowed to name the trash folder, and why.
const NAMES_THE_TRASH: &[(&str, &str)] = &[
    (
        "core/crates/solstone-core-journal/src/lib.rs",
        "defines the name",
    ),
    (
        "core/crates/solstone-core-trash/src/lib.rs",
        "the trash itself",
    ),
    (
        "core/crates/solstone-core-journal-io/src/paths.rs",
        "the journal resolver refuses it",
    ),
    (
        "core/crates/solstone-core-format/src/paths.rs",
        "the index's resolver refuses it",
    ),
    (
        "core/crates/solstone-core-backup-cli/src/lib.rs",
        "tells the owner where a stopped restore's originals are",
    ),
    (
        "core/crates/solstone-core-backup-runtime/src/restore_set_aside.rs",
        "a whole-journal restore sets files aside into it",
    ),
    (
        "core/crates/solstone-core-trash/src/previous_journal.rs",
        "what a restore set aside, as one trash item",
    ),
    (
        "core/crates/solstone-core-journal-io/src/copy_excludes.rs",
        "a previous journal and the trash's lock stay out of backups",
    ),
    (
        "core/crates/solstone-core-repository-contracts/src/contracts/journal_trash_hidden.rs",
        "this guard",
    ),
];

/// Files that list the journal root and already skip the trash, and how.
const ROOT_WALKS: &[(&str, &str)] = &[
    (
        "core/crates/solstone-core-segment/src/chronicle_migration.rs",
        "keeps only eight-digit day names",
    ),
    (
        "core/crates/solstone-core-trash/src/previous_journal.rs",
        "keeps only the names a restore sets aside into",
    ),
];

/// Calls that list a directory.
const LISTINGS: &[&str] = &[
    "read_dir",
    "list_dir_entries",
    "list_dir_entries_bounded",
    "list_directory",
    "walk_directory",
    "enumerate_windows_inventory",
];

/// What the journal root is called when it is passed as itself.
const ROOT_NAMES: &[&str] = &["journal", "journal_root", "journal_path", "journal_dir"];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn production_sources(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.join("core/crates"), root.join("tools")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if matches!(
                    name.as_str(),
                    "target" | "tests" | "benches" | "fixtures" | "node_modules"
                ) || name.starts_with('.')
                {
                    continue;
                }
                stack.push(path);
            } else if name.ends_with(".rs")
                && !name.ends_with("_tests.rs")
                && name != "tests.rs"
                && !name.starts_with("test_")
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Whether an item or expression carries `#[cfg(test)]` (or a `cfg` naming test).
/// Whether a literal names the trash, or the folder a restore sets aside into.
fn names_hidden_literal(literal: &str) -> bool {
    literal.contains(".trash") || literal.contains(".before-restore")
}

fn is_test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        (attr.path().is_ident("cfg") || attr.path().is_ident("test"))
            && (attr.path().is_ident("test")
                || attr
                    .meta
                    .require_list()
                    .is_ok_and(|list| list.tokens.to_string().contains("test")))
    })
}

#[derive(Default)]
struct Scan {
    names_trash: Vec<usize>,
    root_walks: Vec<usize>,
}

impl Scan {
    fn tokens(&mut self, tokens: proc_macro2::TokenStream) {
        for token in tokens {
            match token {
                proc_macro2::TokenTree::Ident(ident)
                    if ident == "TRASH_DIR" || ident == "BEFORE_RESTORE_PREFIX" =>
                {
                    self.names_trash.push(ident.span().start().line);
                }
                proc_macro2::TokenTree::Literal(literal)
                    if names_hidden_literal(&literal.to_string()) =>
                {
                    self.names_trash.push(literal.span().start().line);
                }
                proc_macro2::TokenTree::Group(group) => self.tokens(group.stream()),
                _ => {}
            }
        }
    }
}

fn is_root(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Reference(reference) => is_root(&reference.expr),
        syn::Expr::Paren(paren) => is_root(&paren.expr),
        syn::Expr::Path(path) => path
            .path
            .get_ident()
            .is_some_and(|ident| ROOT_NAMES.iter().any(|name| ident == name)),
        syn::Expr::Field(field) => matches!(&field.member, syn::Member::Named(name)
            if ROOT_NAMES.iter().any(|root| name == root)),
        syn::Expr::MethodCall(call) => {
            matches!(
                call.method.to_string().as_str(),
                "as_path" | "as_ref" | "canonical_path"
            ) && is_root(&call.receiver)
        }
        _ => false,
    }
}

fn names_a_listing(func: &syn::Expr) -> bool {
    let syn::Expr::Path(path) = func else {
        return false;
    };
    let segments: Vec<String> = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect();
    let last = segments.last().map(String::as_str).unwrap_or("");
    LISTINGS.contains(&last) || segments.ends_with(&["WalkDir".to_owned(), "new".to_owned()])
}

impl<'ast> Visit<'ast> for Scan {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        let attrs = match item {
            syn::Item::Mod(item) => &item.attrs,
            syn::Item::Fn(item) => &item.attrs,
            syn::Item::Impl(item) => &item.attrs,
            syn::Item::Use(item) => &item.attrs,
            syn::Item::Const(item) => &item.attrs,
            syn::Item::Static(item) => &item.attrs,
            _ => return syn::visit::visit_item(self, item),
        };
        if !is_test_only(attrs) {
            syn::visit::visit_item(self, item);
        }
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if names_a_listing(&call.func) && call.args.first().is_some_and(is_root) {
            self.root_walks
                .push(call.paren_token.span.open().start().line);
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_ident(&mut self, ident: &'ast proc_macro2::Ident) {
        if ident == "TRASH_DIR" || ident == "BEFORE_RESTORE_PREFIX" {
            self.names_trash.push(ident.span().start().line);
        }
    }

    fn visit_lit_str(&mut self, literal: &'ast syn::LitStr) {
        if names_hidden_literal(&literal.value()) {
            self.names_trash.push(literal.span().start().line);
        }
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.tokens(mac.tokens.clone());
    }

    // Doc comments and other attributes are prose about the code, not code.
    fn visit_attribute(&mut self, _: &'ast syn::Attribute) {}
}

fn scan(source: &str) -> Scan {
    let file = syn::parse_file(source).unwrap_or_else(|error| panic!("parse source: {error}"));
    let mut scan = Scan::default();
    scan.visit_file(&file);
    scan
}

#[test]
fn nothing_outside_the_trash_names_or_walks_into_it() {
    let root = repository_root();
    let mut names = Vec::new();
    let mut walks = Vec::new();
    let mut scanned = 0_usize;
    for path in production_sources(&root) {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        if !source.contains("TRASH_DIR")
            && !source.contains(".trash")
            && !source.contains("BEFORE_RESTORE_PREFIX")
            && !source.contains(".before-restore")
            && !LISTINGS.iter().any(|name| source.contains(name))
            && !source.contains("WalkDir")
        {
            continue;
        }
        let Ok(file) = syn::parse_file(&source) else {
            continue;
        };
        scanned += 1;
        let mut found = Scan::default();
        found.visit_file(&file);
        if !NAMES_THE_TRASH.iter().any(|(allowed, _)| *allowed == rel) {
            names.extend(found.names_trash.iter().map(|line| format!("{rel}:{line}")));
        }
        if !ROOT_WALKS.iter().any(|(allowed, _)| *allowed == rel) {
            walks.extend(found.root_walks.iter().map(|line| format!("{rel}:{line}")));
        }
    }
    assert!(scanned > 50, "the scan covered only {scanned} files");
    assert!(
        names.is_empty(),
        "these name the journal's trash folder outside the trash: {names:#?}\n\
         What is in the trash is reached only through solstone-core-trash \
         (detach, attach, erase, list). A walk that must cross the journal root \
         skips it with solstone_core_journal::is_trash_root_name."
    );
    assert!(
        walks.is_empty(),
        "these list the journal root: {walks:#?}\n\
         Nothing that walks the journal may see the trash folder. Walk a known \
         subdirectory (chronicle/, entities/, ...), or skip names for which \
         solstone_core_journal::is_trash_root_name is true and add the file to \
         ROOT_WALKS in this contract saying how it skips the trash."
    );
}

#[test]
fn the_scan_finds_what_it_forbids() {
    let found = scan(
        r#"
        fn walk(journal: &std::path::Path) {
            let _ = std::fs::read_dir(journal);
            let _ = std::fs::read_dir(&journal_root);
            let _ = list_dir_entries(self.journal.as_path());
            let _ = walkdir::WalkDir::new(journal_path);
            let _ = std::fs::read_dir(journal.join("chronicle"));
            let _ = journal.join(TRASH_DIR);
            let _ = journal.join(".trash/x");
            let _ = format!("{}/.trash", 1);
        }
        #[cfg(test)]
        mod tests {
            fn ignored(journal: &std::path::Path) { let _ = std::fs::read_dir(journal); let _ = TRASH_DIR; }
        }
        "#,
    );
    assert_eq!(found.root_walks.len(), 4, "{:?}", found.root_walks);
    assert_eq!(found.names_trash.len(), 3, "{:?}", found.names_trash);
}
