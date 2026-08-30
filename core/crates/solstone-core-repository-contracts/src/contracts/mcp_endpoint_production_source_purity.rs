// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Keep owner bootstrap bound to the established journal and link-identity readers.

use std::fs;
use std::path::{Path, PathBuf};

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
