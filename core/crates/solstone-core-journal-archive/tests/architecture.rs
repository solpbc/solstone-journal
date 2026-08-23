// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The archive capability surface is deliberately narrow: source and target
//! acquire filesystem capabilities; the remaining modules only consume them.

const SOURCES: &[(&str, &str)] = &[
    ("deny", include_str!("../src/deny.rs")),
    ("encode", include_str!("../src/encode.rs")),
    ("entry", include_str!("../src/entry.rs")),
    ("error", include_str!("../src/error.rs")),
    ("inventory", include_str!("../src/inventory.rs")),
    ("manifest", include_str!("../src/manifest.rs")),
    ("publish", include_str!("../src/publish.rs")),
    ("source", include_str!("../src/source.rs")),
    ("target", include_str!("../src/target.rs")),
    ("test_hooks", include_str!("../src/test_hooks.rs")),
    ("writer", include_str!("../src/writer.rs")),
];
const LIB: &str = include_str!("../src/lib.rs");
const MANIFEST: &str = include_str!("../Cargo.toml");

fn declared_modules() -> Vec<String> {
    LIB.lines()
        .map(str::trim)
        .filter_map(|line| {
            line.strip_prefix("mod ")
                .and_then(|rest| rest.strip_suffix(';'))
        })
        .map(str::to_owned)
        .collect()
}

#[test]
fn scan_covers_every_declared_production_module() {
    let declared = declared_modules();
    let registered: Vec<&str> = SOURCES.iter().map(|(name, _)| *name).collect();
    assert!(!declared.is_empty());
    assert_eq!(declared.len(), SOURCES.len());
    for module in declared {
        assert!(
            registered.contains(&module.as_str()),
            "unscanned module {module}"
        );
    }
}

#[test]
fn descriptor_acquisition_is_confined_to_source_and_target() {
    for (name, source) in SOURCES {
        if matches!(*name, "source" | "target" | "publish") {
            continue;
        }
        for primitive in ["openat(", "fstatat(", "O_NOFOLLOW"] {
            assert!(
                !source.contains(primitive),
                "module {name} reaches acquisition primitive {primitive}"
            );
        }
    }
}

#[test]
fn target_is_read_only_and_ambient_free() {
    let target = SOURCES
        .iter()
        .find_map(|(name, source)| (*name == "target").then_some(*source))
        .expect("target module registered");
    let production = production_source(target);
    for forbidden in [
        "std::env",
        "current_dir",
        "Command::new",
        "std::process",
        "create_dir",
        "remove_dir",
        "remove_file",
        "rename(",
        "write(",
        "AsRawFd",
        "RawFd",
    ] {
        assert!(
            !production.contains(forbidden),
            "target reaches forbidden surface {forbidden}"
        );
    }
    assert!(production.contains("open(\"/\", TARGET_DIRECTORY_FLAGS, Mode::empty())"));
    assert!(production.contains("O_NOFOLLOW"));
}

#[test]
fn publication_is_descriptor_relative_and_has_no_ambient_or_process_reach() {
    let publish = SOURCES
        .iter()
        .find_map(|(name, source)| (*name == "publish").then_some(*source))
        .expect("publish module registered");
    let production = production_source(publish);
    for required in ["openat(", "linkat(", "unlinkat(", "fsync(", "O_NOFOLLOW"] {
        assert!(
            production.contains(required),
            "publication missing {required}"
        );
    }
    assert!(
        production.contains("this isolated module is the archive crate's publication authority")
    );
    for forbidden in ["std::env", "current_dir", "Command::new", "std::process"] {
        assert!(
            !production.contains(forbidden),
            "publication reaches forbidden surface {forbidden}"
        );
    }
}

#[test]
fn source_no_longer_reacquires_a_journal_root() {
    let source = SOURCES
        .iter()
        .find_map(|(name, source)| (*name == "source").then_some(*source))
        .expect("source module registered");

    for forbidden in [
        "open_absolute_filesystem_root",
        "open(\"/\"",
        "REQUESTED_ROOT_FLAGS",
        "canonicalize",
    ] {
        assert!(
            !source.contains(forbidden),
            "source still reacquires a journal root via {forbidden}"
        );
    }
}

#[test]
fn regular_file_opens_are_nonblocking_and_nofollow() {
    let source = SOURCES
        .iter()
        .find_map(|(name, source)| (*name == "source").then_some(*source))
        .expect("source module registered");

    assert!(source.contains(
        "const FILE_FLAGS: OFlag = OFlag::O_RDONLY\n    .union(OFlag::O_CLOEXEC)\n    .union(OFlag::O_NOFOLLOW)\n    .union(OFlag::O_NONBLOCK);"
    ));
    assert_eq!(
        source.matches("FILE_FLAGS").count(),
        2,
        "one declaration and one shared regular-leaf open path"
    );
}

#[test]
fn no_module_reaches_deferred_or_forbidden_surfaces() {
    for (name, source) in SOURCES
        .iter()
        .filter(|(name, _)| !matches!(*name, "encode" | "manifest" | "writer" | "publish"))
        .chain(std::iter::once(&("lib", LIB)))
    {
        let production = production_source(source);
        for forbidden in [
            "ZipArchive",
            "ZipWriter",
            "zip::",
            "Command::new",
            "std::process",
            "reqwest",
            "ureq",
            "pyo3",
            "Python",
        ] {
            assert!(
                !production.contains(forbidden),
                "module {name} reaches forbidden surface {forbidden}"
            );
        }
    }
}

#[test]
fn format_modules_reach_no_non_zip_forbidden_surfaces() {
    for (name, source) in SOURCES
        .iter()
        .filter(|(name, _)| matches!(*name, "encode" | "manifest" | "writer"))
    {
        let production = production_source(source);
        for forbidden in [
            "Command::new",
            "std::process",
            "std::env",
            "SystemTime::now",
            "Instant::now",
            "current_dir",
            "reqwest",
            "ureq",
            "pyo3",
            "Python",
        ] {
            assert!(
                !production.contains(forbidden),
                "format module {name} reaches forbidden surface {forbidden}"
            );
        }
    }
}

#[test]
fn public_descendant_operations_accept_only_inventory_handles() {
    let source = SOURCES
        .iter()
        .find_map(|(name, source)| (*name == "source").then_some(*source))
        .expect("source module registered");
    let signatures = public_signatures(source);
    let public_names: Vec<&str> = signatures
        .iter()
        .map(|signature| {
            signature
                .strip_prefix("pub fn ")
                .expect("public function prefix")
                .split_once('(')
                .expect("public function arguments")
                .0
        })
        .collect();
    assert_eq!(
        public_names,
        [
            "open",
            "inventory",
            "canonical_source",
            "open_file",
            "revalidate"
        ]
    );
    let canonical = signatures
        .iter()
        .find(|signature| signature.contains("pub fn canonical_source"))
        .expect("canonical source accessor");
    assert!(canonical.contains("&self") && canonical.contains("-> &Path"));

    for signature in signatures {
        assert!(
            !signature.contains("fault") && !signature.contains("inject"),
            "public test-control authority: {signature}"
        );
        if signature.contains("pub fn open_file") {
            assert!(signature.contains("&InventoryEntry"), "{signature}");
            assert!(
                !signature.contains("&Path") && !signature.contains("&str"),
                "{signature}"
            );
        }
        if signature.contains("pub fn revalidate") {
            assert!(signature.contains("&self"), "{signature}");
            assert!(
                !signature.contains("&InventoryEntry")
                    && !signature.contains("&Path")
                    && !signature.contains("&str"),
                "{signature}"
            );
        }
        if signature.contains("&Path") {
            assert!(
                signature.contains("pub fn open(root: &Path)")
                    || signature.contains("pub fn canonical_source(&self) -> &Path"),
                "unexpected public path authority: {signature}"
            );
        }
        for forbidden in ["OwnedFd", "RawFd", "AsFd"] {
            assert!(
                !signature.contains(forbidden),
                "public descriptor authority {forbidden}: {signature}"
            );
        }
    }
    assert!(source.contains("pub fn open(root: &Path)"));
}

#[test]
fn manifest_declares_only_runtime_dependencies() {
    assert_eq!(
        section_lines(MANIFEST, "[dependencies]"),
        vec!["zip = { version = \"=2.4.2\", default-features = false, features = [\"deflate\"] }",],
    );
    assert_eq!(
        section_lines(MANIFEST, "[target.'cfg(unix)'.dependencies]"),
        vec![
            "nix = { workspace = true, features = [\"dir\", \"user\"] }",
            "solstone-core-journal-io = { workspace = true }",
        ],
    );
}

#[test]
fn no_dead_code_allowance_remains_in_the_crate() {
    let allowance = "#[allow(dead_code)]";
    let total = SOURCES
        .iter()
        .map(|(_, source)| source.matches(allowance).count())
        .sum::<usize>()
        + LIB.matches(allowance).count();
    assert_eq!(total, 0);
}

#[test]
fn write_archive_has_exactly_one_production_caller() {
    let occurrences = SOURCES
        .iter()
        .map(|(_, source)| production_source(source).matches("write_archive").count())
        .sum::<usize>()
        + production_source(LIB).matches("write_archive").count();
    assert_eq!(
        occurrences, 2,
        "definition and controlled call must be the only occurrences"
    );
    let writer = SOURCES
        .iter()
        .find_map(|(name, source)| (*name == "writer").then_some(*source))
        .expect("writer module registered");
    assert!(production_source(writer).contains("pub(crate) fn write_archive"));

    let encode = SOURCES
        .iter()
        .find_map(|(name, source)| (*name == "encode").then_some(*source))
        .expect("encode module registered");
    assert_eq!(
        production_source(encode).matches("write_archive(").count(),
        1
    );
}

#[test]
fn every_frozen_member_is_length_checked_before_zip_construction() {
    let encode = SOURCES
        .iter()
        .find_map(|(name, source)| (*name == "encode").then_some(*source))
        .expect("encode module registered");
    let production = production_source(encode);
    let iteration = production
        .find("for entry in request.source.inventory().entries()")
        .expect("frozen inventory validation loop");
    let validation = production[iteration..]
        .find("validate_member_name_length(entry.member_name())")
        .map(|offset| iteration + offset)
        .expect("member-length validation call");
    let construction = production
        .find("ZipWriter::new(sink)")
        .expect("ZIP construction");

    assert!(iteration < validation && validation < construction);
    assert_eq!(
        production.matches("validate_member_name_length(").count(),
        2
    );
}

fn production_source(source: &str) -> &str {
    [
        "\n#[cfg(test)]\nmod tests",
        "\n#[cfg(test)]\n#[allow(clippy::disallowed_methods, clippy::disallowed_types)]\nmod tests",
    ]
    .into_iter()
    .filter_map(|boundary| source.find(boundary))
    .min()
    .map_or(source, |boundary| &source[..boundary])
}

fn public_signatures(source: &str) -> Vec<&str> {
    source
        .match_indices("pub fn")
        .map(|(start, _)| {
            let remainder = &source[start..];
            let end = remainder.find('{').expect("public function has a body");
            &remainder[..end]
        })
        .collect()
}

fn section_lines<'a>(manifest: &'a str, header: &str) -> Vec<&'a str> {
    let mut in_section = false;
    let mut lines = Vec::new();
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed == header {
            in_section = true;
            continue;
        }
        if in_section && trimmed.starts_with('[') {
            break;
        }
        if in_section && !trimmed.is_empty() && !trimmed.starts_with('#') {
            lines.push(trimmed);
        }
    }
    lines
}
