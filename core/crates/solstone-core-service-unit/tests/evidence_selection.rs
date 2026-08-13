// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use base64::Engine;
use serde_json::Value;
use solstone_core_service_legacy_evidence::{embedded, manifest_bytes, sha256_hex};
use solstone_core_service_unit::render_systemd_unit;

mod support;

const BLOB: &str = "baa4f68d18830e92aa6ae215ffbf86cc8e14513f";
const MANIFEST_SHA256: &str = "e12e916623c79f88956f130aecb1ab28de277c304c146a1c40fb06ab60dc110a";
const PROFILES: &[&str] = &["alt_port_path", "default", "spaces_nonascii"];

fn fixture(path: &str) -> Value {
    serde_json::from_slice(embedded(path).expect("selected fixture embeds")).expect("fixture JSON")
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().expect("string fixture field")
}

#[test]
fn selected_evidence_cohort_is_exact_and_raw_captures_are_intact() {
    assert_eq!(sha256_hex(manifest_bytes()), MANIFEST_SHA256);
    let expected_profiles: BTreeSet<_> = PROFILES.iter().copied().collect();

    for platform in ["linux", "macos"] {
        let normalized_path =
            format!("core/fixtures/service_legacy_evidence/normalized/{BLOB}/{platform}.json");
        let normalized = fixture(&normalized_path);
        assert_eq!(text(&normalized, "blob"), BLOB);
        assert_eq!(text(&normalized, "platform"), platform);
        assert_eq!(
            text(&normalized, "schema"),
            "service-legacy-normalized-evidence"
        );
        let variants = normalized["variants"].as_object().expect("variants object");
        assert_eq!(variants.len(), 1);
        let canonical = variants.get("canonical").expect("canonical variant");
        let profiles = canonical["profiles"].as_array().expect("profiles array");
        let actual_profiles: BTreeSet<_> = profiles
            .iter()
            .map(|profile| profile.as_str().expect("profile string"))
            .collect();
        assert_eq!(profiles.len(), actual_profiles.len(), "profiles are unique");
        assert_eq!(actual_profiles, expected_profiles);

        for profile in PROFILES {
            let raw_path = format!(
                "core/fixtures/service_legacy_evidence/raw/{BLOB}/{platform}/{profile}.json"
            );
            let raw = fixture(&raw_path);
            assert_eq!(text(&raw, "blob"), BLOB);
            assert_eq!(text(&raw, "platform"), platform);
            assert_eq!(text(&raw, "profile"), *profile);
            assert_eq!(text(&raw, "schema"), "service-legacy-raw-evidence");
            let inputs = raw["inputs"].as_object().expect("inputs object");
            assert_eq!(
                inputs.keys().map(String::as_str).collect::<BTreeSet<_>>(),
                BTreeSet::from(["env", "journal_path", "port"])
            );
            assert!(raw["inputs"]["env"].is_object());
            assert!(raw["inputs"]["journal_path"].is_string());
            assert!(raw["inputs"]["port"].is_number());
            let plist = base64::engine::general_purpose::STANDARD
                .decode(text(&raw["raw"], "plist_base64"))
                .expect("plist base64 decodes");
            let mut captured = plist;
            captured.push(0);
            captured.extend_from_slice(text(&raw["raw"], "systemd_unit").as_bytes());
            assert_eq!(sha256_hex(&captured), text(&raw["raw"], "sha256"));
        }
    }
}

#[test]
fn default_linux_systemd_rendering_remains_byte_identical_to_history() {
    let path = format!("core/fixtures/service_legacy_evidence/raw/{BLOB}/linux/default.json");
    let raw = fixture(&path);
    let inputs = &raw["inputs"];
    let env = inputs["env"].as_object().expect("environment object");
    let expected = env
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().expect("env value").to_owned()))
        .collect();
    let launcher = support::parse_plist(
        &base64::engine::general_purpose::STANDARD
            .decode(text(&raw["raw"], "plist_base64"))
            .expect("plist base64 decodes"),
    )
    .as_dictionary()
    .expect("plist dictionary")["ProgramArguments"]
        .as_array()
        .expect("arguments array")[0]
        .as_string()
        .expect("launcher string")
        .to_owned();
    let rendered = render_systemd_unit(
        &expected,
        &launcher,
        inputs["port"]
            .as_i64()
            .expect("integer port")
            .to_string()
            .as_str(),
        text(inputs, "journal_path"),
    )
    .expect("fixture journal path is valid");
    assert_eq!(rendered, text(&raw["raw"], "systemd_unit"));
}
