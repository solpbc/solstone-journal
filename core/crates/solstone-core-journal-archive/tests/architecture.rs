// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The archive source is deliberately narrow: its source text is the cheapest
//! durable guard against a later convenience API widening its authority.

const SOURCES: &[(&str, &str)] = &[
    ("entry", include_str!("../src/entry.rs")),
    ("error", include_str!("../src/error.rs")),
    ("inventory", include_str!("../src/inventory.rs")),
    ("source", include_str!("../src/source.rs")),
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
fn descriptor_acquisition_is_confined_to_source() {
    for (name, source) in SOURCES {
        if *name == "source" {
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
fn filesystem_root_reopens_are_literal_and_argument_free() {
    let source = SOURCES
        .iter()
        .find_map(|(name, source)| (*name == "source").then_some(*source))
        .expect("source module registered");

    assert!(source.contains("fn open_absolute_filesystem_root() -> Result<OwnedFd, ArchiveError>"));
    assert!(source.contains("open(\"/\", DIRECTORY_FLAGS, Mode::empty())"));
    assert_eq!(
        source.matches("open_absolute_filesystem_root()?").count(),
        3,
        "two root-self reopens plus the non-root traversal open"
    );
    assert!(!source.contains("OsString::from(\".\")"));
    assert!(!source.contains("openat(&authoritative"));
    assert!(!source.contains("openat(&first"));
}

#[test]
fn no_module_reaches_deferred_or_forbidden_surfaces() {
    for (name, source) in SOURCES.iter().chain(std::iter::once(&("lib", LIB))) {
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
                !source.contains(forbidden),
                "module {name} reaches forbidden surface {forbidden}"
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
        if signature.contains("pub fn open_file") || signature.contains("pub fn revalidate") {
            assert!(signature.contains("&InventoryEntry"), "{signature}");
            assert!(
                !signature.contains("&Path") && !signature.contains("&str"),
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
fn manifest_declares_only_nix_as_a_dependency() {
    assert_eq!(
        dependency_lines(MANIFEST),
        vec!["nix = { workspace = true, features = [\"dir\", \"user\"] }"],
    );
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

fn dependency_lines(manifest: &str) -> Vec<&str> {
    let mut in_dependencies = false;
    let mut dependencies = Vec::new();
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed == "[dependencies]" {
            in_dependencies = true;
            continue;
        }
        if in_dependencies && trimmed.starts_with('[') {
            break;
        }
        if in_dependencies && !trimmed.is_empty() && !trimmed.starts_with('#') {
            dependencies.push(trimmed);
        }
    }
    dependencies
}
