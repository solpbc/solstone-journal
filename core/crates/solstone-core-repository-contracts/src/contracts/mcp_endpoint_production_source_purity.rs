// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Keep owner bootstrap bound to the established journal and link-identity readers.

use std::fs;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::visit::Visit;
use syn::{Expr, Item, Pat, Stmt};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn read_repo_file(relative: &str) -> String {
    let path = repository_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn count_occurrences(source: &str, needle: &str) -> usize {
    source.matches(needle).count()
}

fn collect_source_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("endpoint source directory reads") {
        let path = entry.expect("endpoint source entry reads").path();
        if path.is_dir() {
            collect_source_files(&path, files);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs")
            && path.file_name().and_then(|name| name.to_str()) != Some("tests.rs")
        {
            files.push(path);
        }
    }
}

fn production_sources() -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    collect_source_files(
        &repository_root().join("core/crates/solstone-core-mcp-endpoint/src"),
        &mut files,
    );
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            (path, source)
        })
        .collect()
}

#[test]
fn unix_bootstrap_uses_the_single_descriptor_bound_identity_pipeline() {
    let source = read_repo_file("core/crates/solstone-core-mcp-endpoint/src/unix.rs");

    assert!(
        source.contains(
            "use solstone_core_journal_config::{\n    McpEndpointCapability, mcp_endpoint_capability, read_journal_config_bound,\n};"
        ),
        "Unix bootstrap must import the established descriptor-bound config pipeline"
    );
    assert!(
        source.contains("use solstone_core_journal_io::journal_root::JournalRoot;"),
        "Unix bootstrap must import JournalRoot from journal-io"
    );
    assert!(
        source.contains("use solstone_core_sol_link::committed::load_committed_identity_bound;"),
        "Unix bootstrap must import the established descriptor-bound committed loader"
    );
    assert_eq!(
        count_occurrences(&source, "JournalRoot::open("),
        1,
        "Unix bootstrap must acquire exactly one JournalRoot"
    );
    assert_eq!(
        count_occurrences(&source, "read_journal_config_bound("),
        1,
        "Unix bootstrap must use the descriptor-bound config reader once"
    );
    assert_eq!(
        count_occurrences(&source, "mcp_endpoint_capability("),
        1,
        "Unix bootstrap must use the established endpoint capability gate once"
    );
    assert_eq!(
        count_occurrences(&source, "load_committed_identity_bound("),
        1,
        "Unix bootstrap must use the descriptor-bound committed-identity loader once"
    );

    let local_capability_function = ["fn mcp_", "endpoint_capability("].concat();
    for forbidden in [
        "read_journal_config(",
        "load_committed_identity(",
        "fn open(",
        "MCP_ENDPOINT_LOOPBACK_PORT",
        "struct JournalConfigRead",
        "enum McpEndpointCapability",
        "state.json",
        "link/ca",
        "read_state(",
        "parse_state(",
        "fn parse_config",
        "serde_json::",
    ] {
        assert!(
            !source.contains(forbidden),
            "Unix bootstrap must not add a duplicate path/config/identity implementation: {forbidden}"
        );
    }
    assert!(
        !source.contains(&local_capability_function),
        "Unix bootstrap must not define a duplicate endpoint capability gate"
    );
}

#[test]
fn endpoint_sources_do_not_expand_identity_provisioning_or_link_writes() {
    let sources = production_sources();

    for forbidden in [
        "create_or_repair",
        "provision_service_identity",
        "create_service_identity",
        "repair_service_identity",
    ] {
        assert!(
            sources
                .iter()
                .all(|(_, source)| !source.contains(forbidden)),
            "endpoint production sources must not grow service-identity provisioning helper {forbidden}"
        );
    }

    let unix = sources
        .iter()
        .find_map(|(relative, source)| {
            (relative.file_name().and_then(|name| name.to_str()) == Some("unix.rs"))
                .then_some(source)
        })
        .expect("Unix endpoint production source is listed");
    let implementation = unix
        .split_once("pub(super) fn bootstrap")
        .map(|(_, implementation)| implementation)
        .expect("Unix source contains bootstrap implementation");
    let lines = implementation.lines().collect::<Vec<_>>();
    let write_primitives = [
        "mkdirat(",
        "openat(",
        "OFlag::O_CREAT",
        "fs::write(",
        "File::create(",
        "write_bytes_exclusive_bound(",
    ];
    for (index, line) in lines.iter().enumerate() {
        if !write_primitives
            .iter()
            .any(|primitive| line.contains(primitive))
        {
            continue;
        }
        let start = index.saturating_sub(3);
        let end = (index + 4).min(lines.len());
        let context = lines[start..end].join("\n");
        assert!(
            !context.contains("\"link\"")
                && !context.contains("\"link/")
                && !context.contains("\"link\\\\")
                && !context.contains("link.join(")
                && !context.contains("join(\"link\")"),
            "endpoint write primitive must never target link state: {line}"
        );
    }
}

fn integer_constant(file: &syn::File, name: &str) -> u64 {
    file.items
        .iter()
        .find_map(|item| match item {
            Item::Const(item) if item.ident == name => match item.expr.as_ref() {
                Expr::Lit(literal) => match &literal.lit {
                    syn::Lit::Int(value) => Some(
                        value
                            .base10_parse::<u64>()
                            .unwrap_or_else(|error| panic!("parse {name}: {error}")),
                    ),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .unwrap_or_else(|| panic!("integer constant {name} exists"))
}

#[derive(Default)]
struct AcquiredLockUse {
    path_uses: usize,
    shared_references: usize,
    mutable_references: usize,
    bindings: usize,
}

impl<'ast> Visit<'ast> for AcquiredLockUse {
    fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
        if path.qself.is_none() && path.path.is_ident("lock") {
            self.path_uses += 1;
        }
        syn::visit::visit_expr_path(self, path);
    }

    fn visit_expr_reference(&mut self, reference: &'ast syn::ExprReference) {
        if matches!(reference.expr.as_ref(), Expr::Path(path) if path.qself.is_none() && path.path.is_ident("lock"))
        {
            if reference.mutability.is_some() {
                self.mutable_references += 1;
            } else {
                self.shared_references += 1;
            }
        }
        syn::visit::visit_expr_reference(self, reference);
    }

    fn visit_pat_ident(&mut self, pattern: &'ast syn::PatIdent) {
        if pattern.ident == "lock" {
            self.bindings += 1;
        }
        syn::visit::visit_pat_ident(self, pattern);
    }
}

#[derive(Default)]
struct BoundedReadTake {
    exact: usize,
    other: usize,
}

impl<'ast> Visit<'ast> for BoundedReadTake {
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if call.method == "take" {
            let exact_receiver = matches!(call.receiver.as_ref(), Expr::Path(path) if path.qself.is_none() && path.path.is_ident("file"));
            let exact_limit = call.args.len() == 1
                && matches!(call.args.first(), Some(Expr::Path(path)) if path.qself.is_none() && path.path.is_ident("POP_PKCS8_READ_LIMIT"));
            if exact_receiver && exact_limit {
                self.exact += 1;
            } else {
                self.other += 1;
            }
        }
        syn::visit::visit_expr_method_call(self, call);
    }
}

#[test]
fn pop_key_finalization_keeps_exact_bounds_lock_lifetime_and_fail_closed_publish() {
    let source = read_repo_file("core/crates/solstone-core-mcp-endpoint/src/unix.rs");
    let file = syn::parse_file(&source).expect("Unix endpoint source parses");

    assert_eq!(
        integer_constant(&file, "MAX_POP_PKCS8_DER_BYTES"),
        512,
        "PoP PKCS#8 DER maximum remains 512 bytes"
    );
    let read_limit = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Const(item) if item.ident == "POP_PKCS8_READ_LIMIT" => {
                Some(item.expr.to_token_stream().to_string())
            }
            _ => None,
        })
        .expect("PoP PKCS#8 read-limit constant exists");
    assert_eq!(
        read_limit, "MAX_POP_PKCS8_DER_BYTES + 1",
        "bounded read probes exactly one byte beyond the accepted maximum"
    );

    let bootstrap = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(item) if item.sig.ident == "bootstrap" => Some(item),
            _ => None,
        })
        .expect("Unix bootstrap function exists");
    let acquired_lock_bindings = bootstrap
        .block
        .stmts
        .iter()
        .enumerate()
        .filter_map(|(index, statement)| match statement {
            Stmt::Local(local)
                if matches!(&local.pat, Pat::Ident(pattern) if pattern.ident == "lock") =>
            {
                Some((index, local))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        acquired_lock_bindings.len(),
        2,
        "bootstrap has one lock-file binding followed by one acquired-guard binding"
    );
    let (acquired_index, acquired_local) = acquired_lock_bindings[1];
    let acquired_initializer = acquired_local
        .init
        .as_ref()
        .expect("acquired lock binding has an initializer")
        .expr
        .to_token_stream()
        .to_string();
    assert!(
        acquired_initializer.starts_with("Flock :: lock (")
            && acquired_initializer.contains("lock , FlockArg :: LockExclusive"),
        "second lock binding is the exclusive acquired guard"
    );
    let mut acquired_uses = AcquiredLockUse::default();
    for statement in &bootstrap.block.stmts[acquired_index + 1..] {
        acquired_uses.visit_stmt(statement);
    }
    assert_eq!(
        acquired_uses.bindings, 0,
        "the acquired lock guard must not be shadowed before return"
    );
    assert_eq!(
        acquired_uses.mutable_references, 0,
        "the acquired lock guard must not be mutably borrowed before return"
    );
    assert!(
        acquired_uses.shared_references >= 2,
        "binding validation and finalization both borrow the acquired lock guard"
    );
    assert_eq!(
        acquired_uses.path_uses, acquired_uses.shared_references,
        "every acquired lock use through context return must be a shared borrow; moves, explicit unlocks, drops, and replacements are forbidden"
    );

    let bounded_reader = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(item) if item.sig.ident == "read_pkcs8_bounded" => Some(item),
            _ => None,
        })
        .expect("bounded PoP reader exists");
    let mut bounded_take = BoundedReadTake::default();
    bounded_take.visit_block(&bounded_reader.block);
    assert_eq!(
        bounded_take.exact, 1,
        "bounded reader takes exactly POP_PKCS8_READ_LIMIT bytes from the supplied file"
    );
    assert_eq!(
        bounded_take.other, 0,
        "bounded reader must not widen or redirect its take limit"
    );

    let publisher = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(item) if item.sig.ident == "generate_and_publish_key" => Some(item),
            _ => None,
        })
        .expect("PoP key publisher exists")
        .to_token_stream()
        .to_string();
    let eexist = publisher
        .find("EEXIST")
        .expect("publisher classifies an EEXIST result");
    assert!(
        !publisher[eexist..].contains("load_existing_key"),
        "a publication EEXIST must fail this invocation instead of trusting the plant"
    );
}
