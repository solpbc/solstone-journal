// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;

const TARGETS: &[&str] = &[
    "delete_stream_record",
    "read_stream_record",
    "repair_stream_tail_from_markers",
    "set_stream_tail_unconditionally",
    "rebuild_stream_state",
    "backfill_stream_records",
    "advance_unbound_stream",
    "inspect_stream",
    "rebuild_streams",
    "relocate_segment",
    "is_tail",
    "list_stream_records_tolerant",
];

struct TargetIdentVisitor<'a> {
    targets: &'a [&'a str],
    hits: Vec<String>,
}

impl TargetIdentVisitor<'_> {
    fn record_identifier(&mut self, raw: &str) {
        let normalized = raw.strip_prefix("r#").unwrap_or(raw);
        if self.targets.iter().any(|target| *target == normalized) {
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

fn ident_hits_in_file(file: &syn::File, targets: &[&str]) -> Vec<String> {
    let mut visitor = TargetIdentVisitor {
        targets,
        hits: Vec::new(),
    };
    syn::visit::visit_file(&mut visitor, file);
    visitor.hits
}

fn function_mentions_targets_item_fn(function: &syn::ItemFn, targets: &[&str]) -> bool {
    let mut visitor = TargetIdentVisitor {
        targets,
        hits: Vec::new(),
    };
    syn::visit::visit_item_fn(&mut visitor, function);
    !visitor.hits.is_empty()
}

fn function_mentions_targets_impl_fn(function: &syn::ImplItemFn, targets: &[&str]) -> bool {
    let mut visitor = TargetIdentVisitor {
        targets,
        hits: Vec::new(),
    };
    syn::visit::visit_impl_item_fn(&mut visitor, function);
    !visitor.hits.is_empty()
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

fn crate_src_directories() -> Vec<(String, PathBuf)> {
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

fn collect_mentioning_functions(items: &[syn::Item], targets: &[&str], names: &mut Vec<String>) {
    for item in items {
        match item {
            syn::Item::Fn(function) => {
                if function_mentions_targets_item_fn(function, targets) {
                    names.push(function.sig.ident.to_string());
                }
            }
            syn::Item::Impl(implementation) => {
                for impl_item in &implementation.items {
                    if let syn::ImplItem::Fn(function) = impl_item
                        && function_mentions_targets_impl_fn(function, targets)
                    {
                        names.push(function.sig.ident.to_string());
                    }
                }
            }
            syn::Item::Mod(module) => {
                if let Some((_, nested)) = &module.content {
                    collect_mentioning_functions(nested, targets, names);
                }
            }
            _ => {}
        }
    }
}

fn discover_from_files(files: &[(String, PathBuf, String)]) -> BTreeSet<(String, String)> {
    let mut discovered = BTreeSet::new();
    for (relative, path, source) in files {
        let Some(file) = classified_production_file(path, source, TARGETS) else {
            continue;
        };
        let mut names = Vec::new();
        collect_mentioning_functions(&file.items, TARGETS, &mut names);
        for name in names {
            discovered.insert((relative.clone(), name));
        }
    }
    discovered
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

fn load_workspace_files() -> Vec<(String, PathBuf, String)> {
    let mut loaded = Vec::new();
    for (crate_name, src) in crate_src_directories() {
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

fn expected_consumers() -> BTreeSet<(String, String)> {
    [
        // location source delete; filters source == "location" then deletes by name
        (
            "solstone-core-clients-web/src/delete.rs",
            "unlink_location_stream",
        ),
        // import publication; unbound stream names are import-created
        ("solstone-core-import/src/publish.rs", "advance_stream"),
        // operator CLI move seam; name comes from the relocation request
        ("solstone-core-segment-cli/src/move.rs", "relocate"),
        // operator CLI helper; name is the marker/argv stream token
        ("solstone-core-segment-cli/src/read.rs", "checks"),
        // operator CLI helper; name is the marker/argv stream token
        ("solstone-core-segment-cli/src/read.rs", "describe_next"),
        // operator CLI helper; name-keyed tail check
        ("solstone-core-segment-cli/src/read.rs", "is_tail"),
        // relocates a segment and repairs the named stream tail
        ("solstone-core-segment/src/relocate.rs", "relocate_segment"),
        // unbound by design; no cid/source check
        (
            "solstone-core-segment/src/stream_record.rs",
            "advance_unbound_stream",
        ),
        // name-keyed unlink of streams/<name>.json
        (
            "solstone-core-segment/src/stream_record.rs",
            "delete_stream_record",
        ),
        // rebuilds registry keyed by inferred stream name
        (
            "solstone-core-segment/src/stream_repair.rs",
            "backfill_stream_records",
        ),
        // inventory listing by filename stem
        (
            "solstone-core-segment/src/stream_repair.rs",
            "list_stream_records_tolerant",
        ),
        // name-keyed read of streams/<name>.json
        (
            "solstone-core-segment/src/stream_repair.rs",
            "read_stream_record",
        ),
        // writes streams/<name>.json from backfill
        (
            "solstone-core-segment/src/stream_repair.rs",
            "rebuild_stream_state",
        ),
        // name-keyed tail repair
        (
            "solstone-core-segment/src/stream_repair.rs",
            "repair_stream_tail_from_markers",
        ),
        // name-keyed tail write; no production caller today
        (
            "solstone-core-segment/src/stream_repair.rs",
            "set_stream_tail_unconditionally",
        ),
        // settings storage listing; surfaces a record's name field
        ("solstone-core-settings-web/src/storage.rs", "streams"),
        // operator CLI tool; name is the explicit argv, no binding-identity risk
        ("solstone-core-streams-cli/src/lib.rs", "inspect_stream"),
        // operator CLI listing; name is inventory, not binding identity
        ("solstone-core-streams-cli/src/lib.rs", "list_streams"),
        // operator CLI rebuild; reads a marker's stream field then name-keyed repair
        ("solstone-core-streams-cli/src/lib.rs", "rebuild_streams"),
        // operator CLI dispatcher; name is the explicit argv
        (
            "solstone-core-streams-cli/src/lib.rs",
            "run_cli_with_lock_options",
        ),
        // support-drafts writes to a fixed unbound stream name
        (
            "solstone-core-support-drafts/src/lib.rs",
            "append_validated_draft_event_at_local_time",
        ),
    ]
    .into_iter()
    .map(|(path, name)| (path.to_owned(), name.to_owned()))
    .collect()
}

#[test]
fn stream_name_identity_consumers_match_the_reviewed_set() {
    let discovered = discover_from_files(&load_workspace_files());
    assert_eq!(discovered, expected_consumers());
}

#[test]
fn unreviewed_delete_stream_record_caller_is_flagged() {
    let root = tempfile::tempdir().expect("fixture root creates");
    let nested = root.path().join("device/ingest");
    fs::create_dir_all(&nested).expect("nested fixture directory creates");
    fs::write(
        nested.join("cleanup.rs"),
        "fn rogue_cleanup(journal: &Path, name: &str) {\n    delete_stream_record(journal, name);\n}\n",
    )
    .expect("injected caller writes");

    let reviewed = BTreeSet::from([(
        "reviewed/src/lib.rs".to_owned(),
        "reviewed_delete".to_owned(),
    )]);
    let discovered = discover_from_files(&load_root_files(root.path()));
    let unexpected: BTreeSet<_> = discovered.difference(&reviewed).cloned().collect();
    assert_eq!(
        unexpected,
        BTreeSet::from([(
            "device/ingest/cleanup.rs".to_owned(),
            "rogue_cleanup".to_owned()
        )]),
        "scanner must flag an unreviewed delete_stream_record caller: {discovered:?}"
    );
}
