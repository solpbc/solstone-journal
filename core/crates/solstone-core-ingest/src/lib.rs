// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Segment-arrival routes for linked devices.
//!
//! D1: requests use one JSON multipart `envelope` field.  File bytes stay in
//! repeated `files` parts, while envelope metadata and per-file extension keys
//! are retained in the accepted event.  This avoids ambiguous scattered form
//! fields and preserves forward-compatible descriptors.
//!
//! D2: only a linked-device `AccessBasis` admits these routes.  Localhost has
//! no device identity and is refused rather than being implicitly attributed.
//!
//! D3: this crate serves four published device-ingest operations:
//! `ingestUpload`, `ingestSegments`, `ingestManifest`, and
//! `ingestManifestDay`. `register` and bearer-credential issuance are removed
//! by the hard cut; `ingestEvent` and `callosumStream` await a Rust Callosum
//! client; `health` has no settled semantics. `deleteSource` is served by
//! `solstone-core-clients-web` as a whole-segment location erase through
//! retention's door. The remaining deferred operations are an intentional
//! strand delta, not missing routes.
//!
//! Segment bytes and sidecars are written only through `solstone-core-segment`.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod listing;
mod model;
mod read_routes;
mod router;
mod stream_identity;
mod validation;

pub use router::api_router;

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod architecture_tests {
    // Unlike solstone-core-segment's older scanner, `public_signatures` also
    // recognizes `pub async fn`; handlers must not open a raw byte-write door.
    const SOURCES: &[(&str, &str)] = &[
        ("lib.rs", include_str!("lib.rs")),
        ("model.rs", include_str!("model.rs")),
        ("listing.rs", include_str!("listing.rs")),
        ("read_routes.rs", include_str!("read_routes.rs")),
        ("router.rs", include_str!("router.rs")),
        ("stream_identity.rs", include_str!("stream_identity.rs")),
        ("validation.rs", include_str!("validation.rs")),
    ];

    #[test]
    fn crate_has_no_direct_journal_write_primitive() {
        for (_, source) in SOURCES {
            let source = source.split("#[cfg(test)]").next().unwrap_or(source);
            for primitive in [
                "std::fs::write",
                "File::create",
                "OpenOptions::new",
                ".write_all(",
                "write_bytes_exclusive",
                "hold_lock",
                "write_json",
                "append_jsonl",
            ] {
                assert!(
                    !source.contains(primitive),
                    "forbidden primitive {primitive}"
                );
            }
        }
    }

    #[test]
    fn public_byte_signatures_do_not_expose_raw_write_paths() {
        for (_, source) in SOURCES {
            let source = source.split("#[cfg(test)]").next().unwrap_or(source);
            for signature in public_signatures(source) {
                assert!(
                    !signature.contains("&[u8]"),
                    "public raw byte surface: {signature}"
                );
            }
        }
    }

    #[test]
    fn every_production_source_is_architecture_scanned() {
        let scanned = SOURCES
            .iter()
            .map(|(name, _)| *name)
            .collect::<std::collections::BTreeSet<_>>();
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for entry in std::fs::read_dir(directory).expect("read source directory") {
            let entry = entry.expect("read source entry");
            let name = entry.file_name();
            let name = name.to_str().expect("utf8 source name");
            if name.ends_with(".rs") {
                assert!(scanned.contains(name), "{name} omitted from SOURCES");
            }
        }
    }

    fn public_signatures(source: &str) -> impl Iterator<Item = &str> {
        source.lines().filter(|line| {
            let line = line.trim_start();
            line.starts_with("pub fn ") || line.starts_with("pub async fn ")
        })
    }

    #[test]
    fn public_function_bodies_do_not_install_a_fallback() {
        for (_, source) in SOURCES {
            let source = source.split("#[cfg(test)]").next().unwrap_or(source);
            for body in public_function_bodies(source) {
                assert!(
                    !body.contains(".fallback("),
                    "public function installs a fallback:\n{body}"
                );
            }
        }
    }

    fn public_function_bodies(source: &str) -> Vec<&str> {
        let mut bodies = Vec::new();
        let lines: Vec<&str> = source.lines().collect();
        let mut index = 0;
        while index < lines.len() {
            let trimmed = lines[index].trim_start();
            if trimmed.starts_with("pub fn ") || trimmed.starts_with("pub async fn ") {
                let mut cursor = index;
                let mut open_at = None;
                while cursor < lines.len() {
                    if let Some(offset) = lines[cursor].find('{') {
                        open_at = Some((cursor, offset));
                        break;
                    }
                    cursor += 1;
                }
                let Some((start_line, start_col)) = open_at else {
                    index += 1;
                    continue;
                };
                let start = line_offset(source, start_line) + start_col;
                let mut depth = 0;
                let bytes = source.as_bytes();
                let mut end = start;
                for (offset, byte) in bytes.iter().enumerate().skip(start) {
                    match byte {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = offset + 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                bodies.push(&source[start..end]);
                index = start_line + 1;
                continue;
            }
            index += 1;
        }
        bodies
    }

    fn line_offset(source: &str, line_index: usize) -> usize {
        if line_index == 0 {
            return 0;
        }
        source
            .match_indices('\n')
            .nth(line_index - 1)
            .map(|(offset, _)| offset + 1)
            .unwrap_or(source.len())
    }
}
