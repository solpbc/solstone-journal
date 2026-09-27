// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One speller for each entity and facet file name.
//!
//! `entity.json` and `observations.jsonl` belong to the entity crate's store,
//! and `facet.json` to the facets crate's store. Outside a name's owner,
//! production code does not spell it: it goes through the owner's API. Every
//! site that still does is listed below, per site, with its reason, so a
//! listed file can't gain a site and a routed site has to leave the list.
//!
//! Spelling is what this checks. A path an owner hands out, or a name built
//! from pieces (`concat!`, `with_extension`, a stem in `format!`), isn't seen.
//! Build scripts and `tests/` targets are out of scope.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// A file name, and the directory whose files may spell it.
const NAMES: &[(&str, &str)] = &[
    ("entity.json", "core/crates/solstone-core-entity/src/store/"),
    (
        "observations.jsonl",
        "core/crates/solstone-core-entity/src/store/",
    ),
    ("facet.json", "core/crates/solstone-core-facets/src/store/"),
];

/// A literal that is exactly this reads the entity folder's identity files
/// by prefix.
const PREFIX: (&str, &str) = ("entity.", "core/crates/solstone-core-entity/src/store/");

/// Durability registry ids that hand out these files' paths with no literal,
/// and the directories that may name them.
const ARTIFACTS: &[(&str, &[&str])] = &[
    (
        "ArtifactId::Entity",
        &[
            "core/crates/solstone-core-entity/src/store/",
            "core/crates/solstone-core-journal-io/src/",
        ],
    ),
    (
        "ArtifactId::FacetDeclaration",
        &[
            "core/crates/solstone-core-facets/src/store/",
            "core/crates/solstone-core-journal-io/src/",
        ],
    ),
];

/// Every site outside an owner that spells a name, as
/// (file, enclosing item, name, count, reason). A reason is one of:
/// `derived: …` (stays: it reads or matches by path shape and needs no owner
/// semantics), `message: …` (prose naming the file), or `pending 5d-N: …`
/// (routed through an owner API by that slice).
const LISTED: &[(&str, &str, &str, usize, &str)] = &[
    (
        "core/crates/solstone-core-convey-shell/src/speakers_calendar.rs",
        "load_all_journal_entities",
        "entity.json",
        1,
        "pending 5d-2: item 3, load_all_journal_entities from the entity crate",
    ),
    (
        "core/crates/solstone-core-convey-shell/src/speakers_known.rs",
        "load_entity",
        "entity.json",
        1,
        "pending 5d-2: item 3, read_entity_identity",
    ),
    (
        "core/crates/solstone-core-facets/src/entity_doctor.rs",
        "ids_in_unresolved_folder",
        "entity.",
        1,
        "pending 5d-2: item 7, an entity-crate reader for identity files in unresolved folders",
    ),
    (
        "core/crates/solstone-core-facets/src/store/observations.rs",
        "prepare_observation_batch",
        "observations.jsonl",
        1,
        "pending 5d-2: item 7, the link owner's observations path as the parse label",
    ),
    (
        "core/crates/solstone-core-facets/src/store/observations.rs",
        "publish_observation_batch",
        "observations.jsonl",
        1,
        "pending 5d-2: item 7, a captured-snapshot parse label",
    ),
    (
        "core/crates/solstone-core-facets/src/store/reference_scan.rs",
        "count_unrecognized_files",
        "entity.json",
        1,
        "pending 5d-2: item 7, unrecognized_entity_files in the entity crate",
    ),
    (
        "core/crates/solstone-core-format/src/content/mod.rs",
        "INDEX_FAMILY_PATTERNS",
        "observations.jsonl",
        1,
        "derived: the index's content classifier globs notes files by path, below the owners",
    ),
    (
        "core/crates/solstone-core-format/src/content/mod.rs",
        "KNOWN_UNINDEXED_PATTERNS",
        "entity.json",
        1,
        "derived: the classifier records identity files as indexed by entity search",
    ),
    (
        "core/crates/solstone-core-import-sources/src/archive.rs",
        "gate_staged_facet_links",
        "entity.json",
        1,
        "pending 5d-2: item 5, LinkDirs for the staged relink",
    ),
    (
        "core/crates/solstone-core-import-sources/src/archive.rs",
        "mark_entity_json",
        "entity.json",
        1,
        "pending 5d-2: item 5, a per-family flag instead of the path",
    ),
    (
        "core/crates/solstone-core-import-sources/src/archive.rs",
        "merge_facet_relationships",
        "observations.jsonl",
        2,
        "pending 5d-2: item 5, the real observations path as the parse label",
    ),
    (
        "core/crates/solstone-core-import-sources/src/archive.rs",
        "plan_zip_archive",
        "entity.json",
        1,
        "derived: counts archive members by path shape; touches no journal file",
    ),
    (
        "core/crates/solstone-core-import-sources/src/archive.rs",
        "plan_zip_archive",
        "facet.json",
        1,
        "derived: counts archive members by path shape; touches no journal file",
    ),
    (
        "core/crates/solstone-core-import-sources/src/archive.rs",
        "stage_entities",
        "entity.json",
        2,
        "pending 5d-2: item 5, resolve_identity_destination for staged identity writes",
    ),
    (
        "core/crates/solstone-core-indexer/src/daily_evidence.rs",
        "facet_declarations",
        "facet.json",
        1,
        "derived: digest of facet declarations; the indexer sits below the facets crate",
    ),
    (
        "core/crates/solstone-core-indexer/src/edges/candidates.rs",
        "load_facet_candidates",
        "entity.json",
        1,
        "pending 5d-3: LinkDirs::scan after the entity crate drops indexer-store",
    ),
    (
        "core/crates/solstone-core-indexer/src/edges/candidates.rs",
        "load_journal_candidates",
        "entity.json",
        1,
        "pending 5d-3: read_identity_map after the entity crate drops indexer-store",
    ),
    (
        "core/crates/solstone-core-indexer/src/edges/candidates.rs",
        "load_journal_entities",
        "entity.json",
        1,
        "pending 5d-3: read_entity_identity after the entity crate drops indexer-store",
    ),
    (
        "core/crates/solstone-core-indexer/src/edges/registry.rs",
        "EDGE_SOURCE_PATTERNS",
        "observations.jsonl",
        1,
        "derived: the index's edge registry globs notes files by path",
    ),
    (
        "core/crates/solstone-core-indexer/src/edges/speaker.rs",
        "load_journal_entities",
        "entity.json",
        1,
        "pending 5d-3: read_identity_map after the entity crate drops indexer-store",
    ),
    (
        "core/crates/solstone-core-indexer/src/entity_search.rs",
        "load_identities",
        "entity.json",
        1,
        "pending 5d-3: an owner identity path for the search watermark",
    ),
    (
        "core/crates/solstone-core-indexer/src/entity_search.rs",
        "load_relationships",
        "entity.json",
        1,
        "pending 5d-3: LinkDirs::scan, keyed by the link's entity id",
    ),
    (
        "core/crates/solstone-core-indexer-store/src/classification.rs",
        "FacetDeclarationSet::from_journal",
        "facet.json",
        1,
        "derived: resolves facet directory to id below the facets crate, which depends on it",
    ),
    (
        "core/crates/solstone-core-indexer-store/src/scan.rs",
        "index_entity_search_build",
        "entity.json",
        3,
        "derived: SQL purging legacy index rows keyed by an old source path",
    ),
    (
        "core/crates/solstone-core-journal-archive/src/inventory.rs",
        "count_file",
        "entity.json",
        1,
        "derived: export inventory counts files by path shape; files are copied as bytes",
    ),
    (
        "core/crates/solstone-core-journal-archive/src/inventory.rs",
        "count_file",
        "facet.json",
        1,
        "derived: export inventory counts files by path shape; files are copied as bytes",
    ),
    (
        "core/crates/solstone-core-journal-archive/src/windows_source.rs",
        "count_file",
        "entity.json",
        1,
        "derived: export inventory counts files by path shape; files are copied as bytes",
    ),
    (
        "core/crates/solstone-core-journal-archive/src/windows_source.rs",
        "count_file",
        "facet.json",
        1,
        "derived: export inventory counts files by path shape; files are copied as bytes",
    ),
    (
        "core/crates/solstone-core-journal-cli/src/facet_names.rs",
        "scan_dot_directories",
        "facet.json",
        1,
        "pending 5d-2: item 2, observe_facet_destination",
    ),
    (
        "core/crates/solstone-core-journal-cli/src/local_ops.rs",
        "adopt_orphan_facet",
        "facet.json",
        1,
        "pending 5d-2: item 2, create_facet",
    ),
    (
        "core/crates/solstone-core-journal-cli/src/local_ops.rs",
        "copy_tree_into",
        "facet.json",
        1,
        "pending 5d-2: item 2, the dead include_declaration branch goes",
    ),
    (
        "core/crates/solstone-core-journal-cli/src/local_ops.rs",
        "facet_merge_preview_in_journal",
        "facet.json",
        1,
        "pending 5d-2: item 2, observe_facet_destination",
    ),
    (
        "core/crates/solstone-core-journal-cli/src/local_ops.rs",
        "facet_merge_preview_text",
        "facet.json",
        1,
        "message: the merge preview tells the owner which declaration is kept",
    ),
    (
        "core/crates/solstone-core-journal-cli/src/local_ops.rs",
        "merge_entity_links",
        "entity.json",
        1,
        "pending 5d-2: item 2, LinkDirs::link_rel for the report label",
    ),
    (
        "core/crates/solstone-core-journal-cli/src/local_ops.rs",
        "merge_tree_entry",
        "entity.json",
        1,
        "derived: overlay fallback for a folder that is not a link, or a link whose id cannot name a folder; the destination wins, unlike merge_link_fields",
    ),
    (
        "core/crates/solstone-core-journal-cli/src/local_ops.rs",
        "merge_tree_into",
        "facet.json",
        1,
        "derived: a facet merge keeps the destination's declaration and leaves the source's out by name",
    ),
    (
        "core/crates/solstone-core-journal-cli/src/local_ops.rs",
        "orphan_facets",
        "facet.json",
        1,
        "pending 5d-2: item 2, facet_declaration_entry",
    ),
    (
        "core/crates/solstone-core-journal-io/src/durability.rs",
        "JOURNAL_ARTIFACTS",
        "entity.json",
        1,
        "derived: the durability registry declares each artifact's class and glob, below the owners",
    ),
    (
        "core/crates/solstone-core-journal-io/src/durability.rs",
        "JOURNAL_ARTIFACTS",
        "facet.json",
        1,
        "derived: the durability registry declares each artifact's class and glob, below the owners",
    ),
    (
        "core/crates/solstone-core-records-web/src/search.rs",
        "facets",
        "facet.json",
        1,
        "pending 5d-2: item 1, list_facet_directories and observe_facet_declaration",
    ),
    (
        "core/crates/solstone-core-settings-web/src/facets.rs",
        "facet",
        "facet.json",
        1,
        "pending 5d-2: item 1, observe_facet_declaration",
    ),
    (
        "core/crates/solstone-core-speaker-resolve/src/repair_inventory.rs",
        "survey_repair_inventory",
        "ArtifactId::Entity",
        1,
        "pending 5d-2: item 4, observe_entity_identity",
    ),
    (
        "core/crates/solstone-core-speaker-resolve/src/repair_inventory.rs",
        "survey_repair_inventory",
        "entity.json",
        1,
        "pending 5d-2: item 4, observe_entity_identity",
    ),
    (
        "core/crates/solstone-core-speaker-resolve/src/repair_inventory.rs",
        "survey_repair_inventory",
        "entity.json",
        3,
        "message: repair gaps name the identity file they couldn't use",
    ),
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

/// Whether `text` holds `name` as a whole file name: neither preceded nor
/// followed by a character that could belong to a file name.
fn holds_name(text: &str, name: &str) -> bool {
    let mut from = 0;
    while let Some(found) = text[from..].find(name) {
        let start = from + found;
        let end = start + name.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        let name_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-');
        let opens = before.is_none_or(|c| !name_char(c));
        let closes = after.is_none_or(|c| !name_char(c));
        if opens && closes {
            return true;
        }
        from = start + 1;
    }
    false
}

/// The names a literal spells.
fn names_in(text: &str) -> Vec<&'static str> {
    let mut found: Vec<&'static str> = NAMES
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| holds_name(text, name))
        .collect();
    if text == PREFIX.0 {
        found.push(PREFIX.0);
    }
    found
}

fn owner_of(name: &str) -> Vec<&'static str> {
    if let Some((_, owner)) = NAMES.iter().find(|(candidate, _)| *candidate == name) {
        return vec![owner];
    }
    if name == PREFIX.0 {
        return vec![PREFIX.1];
    }
    ARTIFACTS
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, owners)| owners.to_vec())
        .unwrap_or_default()
}

/// Whether a `cfg` predicate requires `test`: `test` appears, and not inside
/// `not(...)` or `any(...)`.
fn requires_test(tokens: proc_macro2::TokenStream) -> bool {
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            proc_macro2::TokenTree::Ident(ident) if ident == "not" || ident == "any" => {
                if matches!(tokens.peek(), Some(proc_macro2::TokenTree::Group(_))) {
                    tokens.next();
                }
            }
            proc_macro2::TokenTree::Ident(ident) if ident == "test" => return true,
            proc_macro2::TokenTree::Group(group) if requires_test(group.stream()) => return true,
            _ => {}
        }
    }
    false
}

fn test_only(attributes: &[syn::Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("test")
            || (attribute.path().is_ident("cfg")
                && attribute
                    .meta
                    .require_list()
                    .is_ok_and(|list| requires_test(list.tokens.clone())))
    })
}

fn path_attribute(attributes: &[syn::Attribute]) -> Option<String> {
    attributes.iter().find_map(|attribute| {
        if !attribute.path().is_ident("path") {
            return None;
        }
        match &attribute.meta {
            syn::Meta::NameValue(syn::MetaNameValue {
                value:
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(text),
                        ..
                    }),
                ..
            }) => Some(text.value()),
            _ => None,
        }
    })
}

/// One site that spells a name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Site {
    item: String,
    name: &'static str,
}

/// Walks one production file: records sites, and the child modules it
/// declares out of line.
struct FileVisitor {
    /// The directory `mod x;` resolves in, for the current inline nesting.
    module_dir: PathBuf,
    /// The directory `#[path]` resolves in, for the current inline nesting.
    path_dir: PathBuf,
    item: Vec<String>,
    sites: Vec<Site>,
    /// Out-of-line child modules, each with whether it resolves its own
    /// `mod x;` beside itself (a `mod.rs`, or a file loaded by `#[path]`).
    children: Vec<Result<(PathBuf, bool), String>>,
}

impl FileVisitor {
    fn record(&mut self, name: &'static str) {
        let item = if self.item.is_empty() {
            "<module>".to_owned()
        } else {
            self.item.join("::")
        };
        self.sites.push(Site { item, name });
    }

    fn literal(&mut self, text: &str) {
        for name in names_in(text) {
            self.record(name);
        }
    }

    fn tokens(&mut self, tokens: proc_macro2::TokenStream) {
        let tokens: Vec<proc_macro2::TokenTree> = tokens.into_iter().collect();
        for window in tokens.windows(4) {
            if let [
                proc_macro2::TokenTree::Ident(owner),
                proc_macro2::TokenTree::Punct(first),
                proc_macro2::TokenTree::Punct(second),
                proc_macro2::TokenTree::Ident(variant),
            ] = window
                && first.as_char() == ':'
                && second.as_char() == ':'
            {
                self.artifact(&owner.to_string(), &variant.to_string());
            }
        }
        for token in tokens {
            match token {
                proc_macro2::TokenTree::Literal(literal) => {
                    match syn::parse_str::<syn::Lit>(&literal.to_string()) {
                        Ok(syn::Lit::Str(text)) => self.literal(&text.value()),
                        Ok(syn::Lit::ByteStr(bytes)) => {
                            self.literal(&String::from_utf8_lossy(&bytes.value()));
                        }
                        _ => {}
                    }
                }
                proc_macro2::TokenTree::Group(group) => self.tokens(group.stream()),
                proc_macro2::TokenTree::Ident(_) | proc_macro2::TokenTree::Punct(_) => {}
            }
        }
    }

    fn artifact(&mut self, owner: &str, variant: &str) {
        let spelled = format!("{owner}::{variant}");
        if let Some((name, _)) = ARTIFACTS.iter().find(|(name, _)| *name == spelled) {
            self.record(name);
        }
    }

    fn within<T>(&mut self, name: String, run: impl FnOnce(&mut Self) -> T) -> T {
        self.item.push(name);
        let result = run(self);
        self.item.pop();
        result
    }
}

impl<'ast> syn::visit::Visit<'ast> for FileVisitor {
    // Docs, `cfg` and `#[path]` aren't code; every other attribute is read,
    // as error messages and field renames can spell a name.
    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        let path = attribute.path();
        if path.is_ident("cfg_attr") {
            let tokens = quote::ToTokens::to_token_stream(&attribute.meta).to_string();
            if tokens.contains("path") {
                self.children.push(Err(format!(
                    "a cfg_attr path in {} isn't followed; name the module's file plainly",
                    self.module_dir.display()
                )));
            }
            return;
        }
        if path.is_ident("doc") || path.is_ident("cfg") || path.is_ident("path") {
            return;
        }
        self.tokens(quote::ToTokens::to_token_stream(&attribute.meta));
    }

    fn visit_use_path(&mut self, path: &'ast syn::UsePath) {
        if path.ident == "ArtifactId" {
            let mut stack = vec![&*path.tree];
            while let Some(tree) = stack.pop() {
                match tree {
                    syn::UseTree::Name(name) => {
                        self.artifact("ArtifactId", &name.ident.to_string())
                    }
                    syn::UseTree::Rename(rename) => {
                        self.artifact("ArtifactId", &rename.ident.to_string());
                    }
                    syn::UseTree::Glob(_) => {
                        for (name, _) in ARTIFACTS {
                            self.record(name);
                        }
                    }
                    syn::UseTree::Group(group) => stack.extend(group.items.iter()),
                    syn::UseTree::Path(_) => {}
                }
            }
        }
        syn::visit::visit_use_path(self, path);
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if test_only(&module.attrs) {
            return;
        }
        let name = module.ident.to_string();
        let name = name.strip_prefix("r#").unwrap_or(&name).to_owned();
        match &module.content {
            None => {
                let child = match path_attribute(&module.attrs) {
                    Some(path) => Ok((self.path_dir.join(path), true)),
                    None => {
                        let flat = self.module_dir.join(format!("{name}.rs"));
                        let nested = self.module_dir.join(&name).join("mod.rs");
                        if flat.is_file() {
                            Ok((flat, false))
                        } else if nested.is_file() {
                            Ok((nested, true))
                        } else {
                            Err(format!(
                                "mod {name} in {} resolves to no file",
                                self.module_dir.display()
                            ))
                        }
                    }
                };
                self.children.push(child);
            }
            Some(_) => {
                // Inside an inline module, both `mod x;` and `#[path]` resolve
                // under the module's directory: the file's own directory (or,
                // for a non-mod-rs file, its stem) plus the inline names, or
                // the inline module's own `#[path]`.
                let module_dir = std::mem::take(&mut self.module_dir);
                let path_dir = std::mem::take(&mut self.path_dir);
                let inner = match path_attribute(&module.attrs) {
                    Some(path) => module_dir.join(path),
                    None => module_dir.join(&name),
                };
                self.module_dir = inner.clone();
                self.path_dir = inner;
                self.within(name, |visitor| syn::visit::visit_item_mod(visitor, module));
                self.module_dir = module_dir;
                self.path_dir = path_dir;
            }
        }
    }

    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        if !test_only(&function.attrs) {
            self.within(function.sig.ident.to_string(), |visitor| {
                syn::visit::visit_item_fn(visitor, function);
            });
        }
    }

    fn visit_item_impl(&mut self, block: &'ast syn::ItemImpl) {
        if test_only(&block.attrs) {
            return;
        }
        let last = |path: &syn::Path| {
            path.segments
                .last()
                .map_or_else(|| "impl".to_owned(), |segment| segment.ident.to_string())
        };
        let mut name = match &*block.self_ty {
            syn::Type::Path(path) => last(&path.path),
            _ => "impl".to_owned(),
        };
        if let Some((_, trait_path, _)) = &block.trait_ {
            name = format!("{name} as {}", last(trait_path));
        }
        self.within(name, |visitor| syn::visit::visit_item_impl(visitor, block));
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        if !test_only(&function.attrs) {
            self.within(function.sig.ident.to_string(), |visitor| {
                syn::visit::visit_impl_item_fn(visitor, function);
            });
        }
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        if !test_only(&item.attrs) {
            self.within(item.ident.to_string(), |visitor| {
                syn::visit::visit_item_const(visitor, item);
            });
        }
    }

    fn visit_item_static(&mut self, item: &'ast syn::ItemStatic) {
        if !test_only(&item.attrs) {
            self.within(item.ident.to_string(), |visitor| {
                syn::visit::visit_item_static(visitor, item);
            });
        }
    }

    fn visit_item(&mut self, item: &'ast syn::Item) {
        let attributes = match item {
            syn::Item::Use(item) => &item.attrs,
            syn::Item::Struct(item) => &item.attrs,
            syn::Item::Enum(item) => &item.attrs,
            syn::Item::Trait(item) => &item.attrs,
            syn::Item::Macro(item) => &item.attrs,
            _ => {
                syn::visit::visit_item(self, item);
                return;
            }
        };
        if !test_only(attributes) {
            syn::visit::visit_item(self, item);
        }
    }

    fn visit_stmt(&mut self, statement: &'ast syn::Stmt) {
        let attributes: &[syn::Attribute] = match statement {
            syn::Stmt::Local(local) => &local.attrs,
            syn::Stmt::Macro(mac) => &mac.attrs,
            syn::Stmt::Expr(expression, _) => expression_attributes(expression),
            syn::Stmt::Item(_) => &[],
        };
        if !test_only(attributes) {
            syn::visit::visit_stmt(self, statement);
        }
    }

    fn visit_lit_str(&mut self, text: &'ast syn::LitStr) {
        self.literal(&text.value());
    }

    fn visit_lit_byte_str(&mut self, bytes: &'ast syn::LitByteStr) {
        self.literal(&String::from_utf8_lossy(&bytes.value()));
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.tokens(mac.tokens.clone());
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        if let [.., owner, variant] = segments.as_slice() {
            let spelled = format!("{owner}::{variant}");
            if let Some((name, _)) = ARTIFACTS.iter().find(|(name, _)| *name == spelled) {
                self.record(name);
            }
        }
        syn::visit::visit_path(self, path);
    }
}

fn expression_attributes(expression: &syn::Expr) -> &[syn::Attribute] {
    match expression {
        syn::Expr::Call(call) => &call.attrs,
        syn::Expr::MethodCall(call) => &call.attrs,
        syn::Expr::Macro(mac) => &mac.attrs,
        syn::Expr::Block(block) => &block.attrs,
        syn::Expr::If(expression) => &expression.attrs,
        syn::Expr::Assign(expression) => &expression.attrs,
        _ => &[],
    }
}

/// The production files of one crate, walked from its roots, and every
/// site in them. A `mod` that resolves to no file is an error.
fn walk_crate(
    krate: &Path,
    seen: &mut BTreeSet<PathBuf>,
    sites: &mut BTreeMap<PathBuf, Vec<Site>>,
    errors: &mut Vec<String>,
) {
    let roots = crate_roots(krate);
    if roots.is_empty() {
        errors.push(format!(
            "{} has no library or binary to walk",
            krate.display()
        ));
    }
    // A crate root, a `mod.rs` and a file loaded by `#[path]` resolve their
    // `mod x;` beside themselves; any other file under its own stem.
    let mut pending: Vec<(PathBuf, bool)> = roots.into_iter().map(|root| (root, true)).collect();
    while let Some((file, owns_directory)) = pending.pop() {
        let Ok(canonical) = fs::canonicalize(&file) else {
            errors.push(format!("{} can't be read", file.display()));
            continue;
        };
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let source = match fs::read_to_string(&canonical) {
            Ok(source) => source,
            Err(error) => {
                errors.push(format!("{}: {error}", canonical.display()));
                continue;
            }
        };
        let parsed = match syn::parse_file(&source) {
            Ok(parsed) => parsed,
            Err(error) => {
                errors.push(format!(
                    "{}: can't be read as Rust, so it can't be checked: {error}",
                    canonical.display()
                ));
                continue;
            }
        };
        let directory = canonical
            .parent()
            .expect("file has a directory")
            .to_path_buf();
        let module_dir = if owns_directory {
            directory.clone()
        } else {
            directory.join(canonical.file_stem().expect("file has a stem"))
        };
        let mut visitor = FileVisitor {
            module_dir,
            path_dir: directory,
            item: Vec::new(),
            sites: Vec::new(),
            children: Vec::new(),
        };
        syn::visit::visit_file(&mut visitor, &parsed);
        if !visitor.sites.is_empty() {
            sites.entry(canonical).or_default().extend(visitor.sites);
        }
        for child in visitor.children {
            match child {
                Ok(child) => pending.push(child),
                Err(error) => errors.push(error),
            }
        }
    }
}

/// A crate's production roots: its library and binaries, from `Cargo.toml`
/// and the default layout, except binaries kept under `tests/`.
fn crate_roots(krate: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let manifest = fs::read_to_string(krate.join("Cargo.toml"))
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok());
    let declared_path = |table: &toml_edit::Item| {
        table
            .get("path")
            .and_then(toml_edit::Item::as_str)
            .map(str::to_owned)
    };
    let mut lib = krate.join("src/lib.rs");
    if let Some(path) = manifest
        .as_ref()
        .and_then(|manifest| manifest.get("lib"))
        .and_then(declared_path)
    {
        lib = krate.join(path);
    }
    if lib.is_file() {
        roots.push(lib);
    }
    let main = krate.join("src/main.rs");
    if main.is_file() {
        roots.push(main);
    }
    if let Some(bins) = manifest
        .as_ref()
        .and_then(|manifest| manifest.get("bin"))
        .and_then(toml_edit::Item::as_array_of_tables)
    {
        for bin in bins {
            if let Some(path) = bin.get("path").and_then(toml_edit::Item::as_str)
                && !path.starts_with("tests/")
            {
                roots.push(krate.join(path));
            }
        }
    }
    if let Ok(entries) = fs::read_dir(krate.join("src/bin")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "rs") {
                roots.push(path);
            } else if path.join("main.rs").is_file() {
                roots.push(path.join("main.rs"));
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Site counts keyed by (file, enclosing item, name).
type Counted = BTreeMap<(String, String, &'static str), usize>;

/// Every production site outside a name's owner, keyed by file relative to
/// the repository.
fn outside_sites(root: &Path, crates: &Path) -> (Counted, Vec<String>) {
    let mut sites = BTreeMap::new();
    let mut errors = Vec::new();
    // A file two crates reach (a shared binary) is counted once.
    let mut seen = BTreeSet::new();
    let mut entries: Vec<PathBuf> = fs::read_dir(crates)
        .expect("read crates")
        .map(|entry| entry.expect("crate entry").path())
        .filter(|path| path.join("Cargo.toml").is_file())
        .collect();
    entries.sort();
    for krate in entries {
        walk_crate(&krate, &mut seen, &mut sites, &mut errors);
    }
    let root = fs::canonicalize(root).expect("canonical root");
    let mut counted = BTreeMap::new();
    for (file, found) in sites {
        let relative = file
            .strip_prefix(&root)
            .unwrap_or(&file)
            .to_string_lossy()
            .into_owned();
        for site in found {
            if owner_of(site.name)
                .iter()
                .any(|owner| relative.starts_with(owner))
            {
                continue;
            }
            *counted
                .entry((relative.clone(), site.item, site.name))
                .or_insert(0) += 1;
        }
    }
    (counted, errors)
}

/// Differences between what the tree spells and what is listed.
fn compare(found: &Counted, listed: &[(&str, &str, &str, usize, &str)]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut listed_counts: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    for (file, item, name, count, reason) in listed {
        assert!(
            reason.starts_with("derived: ")
                || reason.starts_with("message: ")
                || reason.starts_with("pending 5d-"),
            "a listed site needs a reason of a known kind: {file} {item} {name}"
        );
        *listed_counts
            .entry(((*file).to_owned(), (*item).to_owned(), (*name).to_owned()))
            .or_insert(0) += count;
    }
    for ((file, item, name), count) in found {
        let key = (file.clone(), item.clone(), (*name).to_owned());
        match listed_counts.get(&key) {
            None => problems.push(format!(
                "{file} ({item}) spells {name} {count}x and isn't listed"
            )),
            Some(listed) if count > listed => problems.push(format!(
                "{file} ({item}) spells {name} {count}x; {listed} listed"
            )),
            _ => {}
        }
    }
    for ((file, item, name), listed) in &listed_counts {
        let count = found
            .get(&(file.clone(), item.clone(), leak(name)))
            .copied()
            .unwrap_or(0);
        if count < *listed {
            problems.push(format!(
                "stale: {file} ({item}) is listed for {name} {listed}x but spells it {count}x"
            ));
        }
    }
    problems
}

fn leak(name: &str) -> &'static str {
    NAMES
        .iter()
        .map(|(candidate, _)| *candidate)
        .chain([PREFIX.0])
        .chain(ARTIFACTS.iter().map(|(candidate, _)| *candidate))
        .find(|candidate| *candidate == name)
        .unwrap_or("")
}

#[test]
fn only_owners_spell_entity_and_facet_file_names() {
    let root = repository_root();
    let (found, errors) = outside_sites(&root, &root.join("core/crates"));
    assert!(
        errors.is_empty(),
        "the module tree can't be walked:\n{}",
        errors.join("\n")
    );
    for (file, ..) in LISTED {
        assert!(root.join(file).is_file(), "listed file is gone: {file}");
    }
    let problems = compare(&found, LISTED);
    assert!(
        problems.is_empty(),
        "go through the owner's API (the entity or facets store), or list the site with its reason:\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_real_split_variable_sites_are_seen_and_test_modules_are_not() {
    let root = repository_root();
    let (found, _) = outside_sites(&root, &root.join("core/crates"));
    let count = |file: &str| {
        found
            .iter()
            .filter(|((path, ..), _)| path == file)
            .map(|(_, count)| count)
            .sum::<usize>()
    };
    // Bound through a `for` pattern, or passed in as a parameter: invisible
    // to a path-shape check.
    assert_eq!(
        count("core/crates/solstone-core-indexer/src/edges/candidates.rs"),
        3
    );
    assert_eq!(
        count("core/crates/solstone-core-indexer/src/entity_search.rs"),
        2
    );
    assert_eq!(
        count("core/crates/solstone-core-convey-shell/src/speakers_known.rs"),
        1
    );
    for file in [
        "core/crates/solstone-core-indexer/src/edges/candidates.rs",
        "core/crates/solstone-core-indexer/src/entity_search.rs",
        "core/crates/solstone-core-convey-shell/src/speakers_known.rs",
    ] {
        let source = fs::read_to_string(root.join(file)).expect("read source");
        assert_eq!(
            crate::facet_link_layout::path_shape_hits(&source),
            0,
            "{file} is visible to the path-shape check too; this contract adds nothing there"
        );
    }
    for test_only in [
        "core/crates/solstone-core-records-web/src/corpus.rs",
        "core/crates/solstone-core-records-web/src/search_page_oracle.rs",
        "core/crates/solstone-core-transcripts-web/src/corpus.rs",
        "core/crates/solstone-core-settings-web/src/mutations.rs",
        "core/crates/solstone-core-repository-contracts/src/contracts/facet_link_layout.rs",
        "core/crates/solstone-core-repository-contracts/src/contracts/entity_facet_file_names.rs",
    ] {
        assert_eq!(
            count(test_only),
            0,
            "{test_only} is test-only through its parent"
        );
    }
}

#[test]
fn the_matcher_sees_names_and_only_names() {
    for hit in [
        "entity.json",
        "facets/{facet}/facet.json",
        "entities/*/observations.jsonl",
        "DELETE FROM chunks WHERE path LIKE 'entities/%/entity.json'",
        "malformed entity.json: bad",
        "can't read `entity.json`",
        "(facet.json)",
        "**/{entity.json,facet.json}",
        "entity.",
    ] {
        assert!(!names_in(hit).is_empty(), "{hit}");
    }
    for miss in [
        "spawn-identity.json",
        "facets.json",
        "facet.routing_pending",
        "entity.json.pipe",
        "entity.wedged-1.json",
    ] {
        assert!(names_in(miss).is_empty(), "{miss}");
    }
    assert_eq!(names_in("**/{entity.json,facet.json}").len(), 2);
}

#[test]
fn the_visitor_sees_every_production_spelling_and_skips_tests_and_docs() {
    let source = r##"
        /// reads entity.json
        fn a(dir: &Path) {
            let _ = dir.join("entity.json");
            let _ = format!("{dir}/facet.json");
            let _ = matches!(name, b"facet.json");
            let _ = name.starts_with("entity.");
            let _ = ArtifactId::FacetDeclaration;
        }
        impl Reader {
            fn read(&self) { let _ = "observations.jsonl"; }
            #[cfg(test)]
            fn fixture(&self) { let _ = "entity.json"; }
        }
        #[cfg(any(test, feature = "test-hooks"))]
        fn hooks() { let _ = "entity.json"; }
        #[cfg(all(test, feature = "full-tests"))]
        fn full() { let _ = "entity.json"; }
        fn statements() {
            #[cfg(test)]
            let _ = "facet.json";
            #[cfg(not(test))]
            let _ = "facet.json";
        }
        #[cfg(test)]
        mod tests { fn b() { let _ = "entity.json"; } }
        #[derive(Error)]
        enum Failure {
            #[error("malformed facet.json at {0}")]
            Malformed(String),
        }
        use journal_io::ArtifactId::Entity as Identity;
        fn artifact(id: ArtifactId) -> bool { matches!(id, ArtifactId::Entity) }
    "##;
    let file = syn::parse_file(source).expect("parse fixture");
    let mut visitor = FileVisitor {
        module_dir: PathBuf::new(),
        path_dir: PathBuf::new(),
        item: Vec::new(),
        sites: Vec::new(),
        children: Vec::new(),
    };
    syn::visit::visit_file(&mut visitor, &file);
    let mut seen: Vec<(String, &str)> = visitor
        .sites
        .iter()
        .map(|site| (site.item.clone(), site.name))
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("<module>".to_owned(), "ArtifactId::Entity"),
            ("<module>".to_owned(), "facet.json"),
            ("Reader::read".to_owned(), "observations.jsonl"),
            ("a".to_owned(), "ArtifactId::FacetDeclaration"),
            ("a".to_owned(), "entity."),
            ("a".to_owned(), "entity.json"),
            ("a".to_owned(), "facet.json"),
            ("a".to_owned(), "facet.json"),
            ("artifact".to_owned(), "ArtifactId::Entity"),
            ("hooks".to_owned(), "entity.json"),
            ("statements".to_owned(), "facet.json"),
        ]
    );
}

#[test]
fn the_walk_follows_the_module_tree_and_leaves_out_test_modules() {
    let temporary = tempfile::tempdir().expect("temporary crate");
    let krate = temporary.path().join("fixture-crate");
    let write = |relative: &str, text: &str| {
        let path = krate.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    let spell = "fn f() { let _ = \"entity.json\"; }\n";
    write("Cargo.toml", "[package]\nname = \"fixture-crate\"\n");
    write(
        "src/lib.rs",
        "#[cfg(test)] mod a;\n#[cfg(test)] #[path = \"sub/b.rs\"] mod b;\nmod r#loop;\nmod c;\n#[path = \"sub/pathed.rs\"] mod pathed;\nmod plain;\nfn root() { let _ = \"entity.json\"; }\n",
    );
    write("src/a.rs", &format!("mod inner;\n{spell}"));
    write("src/a/inner.rs", spell);
    write("src/sub/b.rs", spell);
    write("src/loop.rs", spell);
    write("src/c/mod.rs", &format!("mod d;\n{spell}"));
    write("src/c/d.rs", spell);
    // A file loaded by `#[path]` resolves its children beside itself.
    write("src/sub/pathed.rs", &format!("mod child;\n{spell}"));
    write("src/sub/child.rs", spell);
    // A non-mod-rs file: an inline module's children resolve under its stem.
    write(
        "src/plain.rs",
        "mod inner { mod deep; #[path = \"other.rs\"] mod other; }\n",
    );
    write("src/plain/inner/deep.rs", spell);
    write("src/plain/inner/other.rs", spell);

    let walk = || {
        let mut sites = BTreeMap::new();
        let mut errors = Vec::new();
        walk_crate(&krate, &mut BTreeSet::new(), &mut sites, &mut errors);
        let canonical = fs::canonicalize(&krate).unwrap();
        let mut files: Vec<String> = sites
            .keys()
            .map(|path| {
                path.strip_prefix(&canonical)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        files.sort();
        (files, errors)
    };
    let (files, errors) = walk();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        files,
        [
            "src/c/d.rs",
            "src/c/mod.rs",
            "src/lib.rs",
            "src/loop.rs",
            "src/plain/inner/deep.rs",
            "src/plain/inner/other.rs",
            "src/sub/child.rs",
            "src/sub/pathed.rs",
        ]
    );

    write("src/c/mod.rs", "mod d;\nmod missing;\n");
    let (_, errors) = walk();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("mod missing"), "{errors:?}");
}

#[test]
fn a_listed_file_cant_gain_a_site_and_a_routed_site_leaves_the_list() {
    let key = |item: &str| {
        (
            "core/crates/x/src/lib.rs".to_owned(),
            item.to_owned(),
            "entity.json",
        )
    };
    let listed: &[(&str, &str, &str, usize, &str)] = &[(
        "core/crates/x/src/lib.rs",
        "read",
        "entity.json",
        2,
        "pending 5d-2: x",
    )];

    let same = BTreeMap::from([(key("read"), 2)]);
    assert!(compare(&same, listed).is_empty());

    let gained = BTreeMap::from([(key("read"), 2), (key("write"), 1)]);
    assert_eq!(compare(&gained, listed).len(), 1);

    let more = BTreeMap::from([(key("read"), 3)]);
    assert_eq!(compare(&more, listed).len(), 1);

    let routed = BTreeMap::from([(key("read"), 1)]);
    let problems = compare(&routed, listed);
    assert!(
        problems.len() == 1 && problems[0].starts_with("stale:"),
        "{problems:?}"
    );
}
