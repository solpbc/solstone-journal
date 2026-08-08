// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::manifest_known_key::starts_with_body_prefix;
use crate::{BodyObject, BodyValue, ManifestKnownKey, ManifestScanError};

const MAX_MANIFEST_BYTES: usize = 1_048_576;

/// A parsed body manifest with facts derived from its top-level key occurrences.
#[derive(Clone, Debug, PartialEq)]
pub struct ScannedBodyManifest {
    object: BodyObject,
    duplicated_known_keys: Vec<ManifestKnownKey>,
    has_body_prefixed_key: bool,
    has_unknown_body_prefixed_key: bool,
}

impl ScannedBodyManifest {
    /// Returns the parsed top-level manifest object.
    pub fn object(&self) -> &BodyObject {
        &self.object
    }

    /// Returns known keys that occurred at least twice in canonical order.
    pub fn duplicated_known_keys(&self) -> &[ManifestKnownKey] {
        &self.duplicated_known_keys
    }

    /// Returns whether any top-level key starts with the ASCII `body_` prefix.
    pub fn has_body_prefixed_key(&self) -> bool {
        self.has_body_prefixed_key
    }

    /// Returns whether an unknown top-level key starts with the ASCII `body_` prefix.
    pub fn has_unknown_body_prefixed_key(&self) -> bool {
        self.has_unknown_body_prefixed_key
    }
}

/// Parses a bounded body manifest and records facts from its raw top-level keys.
pub fn scan_body_manifest(input: &[u8]) -> Result<ScannedBodyManifest, ManifestScanError> {
    if input.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestScanError::InputTooLarge);
    }

    let (value, top_level_keys) = crate::parser::parse_with_top_level_keys(input)
        .map_err(|_| ManifestScanError::MalformedManifest)?;
    let BodyValue::Object(object) = value else {
        return Err(ManifestScanError::MalformedManifest);
    };

    let mut duplicated_known_keys = Vec::new();
    for known in ManifestKnownKey::ALL {
        let count = top_level_keys
            .iter()
            .filter(|key| ManifestKnownKey::from_body_string(key) == Some(known))
            .count();
        if count >= 2 {
            duplicated_known_keys.push(known);
        }
    }

    let mut has_body_prefixed_key = false;
    let mut has_unknown_body_prefixed_key = false;
    for key in &top_level_keys {
        if starts_with_body_prefix(key) {
            has_body_prefixed_key = true;
            if ManifestKnownKey::from_body_string(key).is_none() {
                has_unknown_body_prefixed_key = true;
            }
        }
    }

    Ok(ScannedBodyManifest {
        object,
        duplicated_known_keys,
        has_body_prefixed_key,
        has_unknown_body_prefixed_key,
    })
}

#[cfg(test)]
mod tests {
    use crate::parser::test_corpus::{
        alternating_containers, assert_body_value_bitwise_eq, assert_entry_points_agree,
        fixture_inputs, generated_corpus, repeated_objects,
    };
    use crate::{BodyValue, ManifestScanError, parse};

    use super::scan_body_manifest;

    #[test]
    fn selected_parser_corpus_refines_to_manifest_scanning() {
        let fixture_names = [
            "native manifest: apple_retain_complete_one_row",
            "native manifest: oura_retain_parsed_one_row",
            "native manifest: apple_discard_zero_rows",
            "native manifest: oura_discard_zero_rows",
        ];
        let generated_names = [
            "insignificant whitespace",
            "member order",
            "escaped duplicate",
            "mixed duplicates",
            "nested arrays and objects",
            "NaN",
            "Infinity",
            "negative Infinity",
            "fixed lower exponent boundary",
            "scientific lower exponent boundary",
            "fixed upper exponent boundary",
            "scientific upper exponent boundary",
            "maximum finite",
            "4300-digit integer",
            "invalid UTF-8",
            "missing object delimiter",
            "trailing object delimiter",
        ];
        let selected = fixture_inputs()
            .into_iter()
            .filter(|(name, _)| fixture_names.contains(&name.as_str()))
            .chain(
                generated_corpus()
                    .into_iter()
                    .filter(|(name, _)| generated_names.contains(&name.as_str())),
            )
            .collect::<Vec<_>>();

        assert_eq!(selected.len(), fixture_names.len() + generated_names.len());
        for (name, input) in selected {
            if matches!(
                name.as_str(),
                "missing object delimiter" | "trailing object delimiter"
            ) {
                assert!(parse(&input).is_err(), "{name} must be grammar-malformed");
                assert_eq!(
                    scan_body_manifest(&input),
                    Err(ManifestScanError::MalformedManifest),
                    "{name} must be rejected as malformed"
                );
            }
            assert_scan_refines_parse(&input, &name);
            for length in 1..input.len() {
                assert_scan_refines_parse(&input[..length], &format!("{name} prefix {length}"));
            }
        }
    }

    #[test]
    fn depth_and_nonobject_cases_refine_to_manifest_scanning() {
        for build in [
            repeated_objects as fn(usize) -> (Vec<u8>, usize),
            alternating_containers,
        ] {
            for depth in [128, 129] {
                let (input, _) = build(depth);
                assert_scan_refines_parse(&input, &format!("depth {depth}"));
            }
        }
        for input in [
            b"[1,2]".as_slice(),
            b"\"text\"",
            b"123",
            b"true",
            b"false",
            b"null",
            br#"{"plain":true}"#,
        ] {
            assert_scan_refines_parse(input, "inline value");
        }
    }

    #[test]
    fn parser_entry_points_agree_for_scanner_representatives() {
        for input in [
            br#"{"body_source_schema":"solstone.body.bundle.v1"}"#.as_slice(),
            br#"{"import_id":1,"import_id":2}"#,
            b"{",
        ] {
            assert_entry_points_agree(input);
        }
    }

    fn assert_scan_refines_parse(input: &[u8], name: &str) {
        let parsed = parse(input);
        let scanned = scan_body_manifest(input);
        match parsed {
            Ok(BodyValue::Object(expected)) => {
                let scanned = scanned.unwrap_or_else(|error| {
                    panic!("scanner should accept object {name}: {error:?}")
                });
                assert_body_value_bitwise_eq(
                    &BodyValue::Object(scanned.object().clone()),
                    &BodyValue::Object(expected),
                );
            }
            Ok(_) | Err(_) => {
                assert_eq!(scanned, Err(ManifestScanError::MalformedManifest), "{name}")
            }
        }
    }
}
