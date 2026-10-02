// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Privacy-limited version and host facts for copy and support handoffs.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub const SEPARATOR: &str = " · ";
pub const COMPONENT_NAMES: [&str; 9] = [
    "journal",
    "solstone macos app",
    "ios app",
    "watch app",
    "android app",
    "windows app",
    "linux desktop app",
    "tmux app",
    "solstone extension",
];

/// The complete public about resource. Empty strings mean unobserved facts;
/// the renderer omits them. It contains no installation or device identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct About {
    pub protocol_version: u32,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub build: Option<String>,
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub about: String,
}

#[must_use]
pub fn normalize_arch(raw: &str) -> &str {
    match raw {
        "aarch64" | "ARM64" | "arm64-v8a" | "arm64" => "arm64",
        "amd64" | "x64" | "AMD64" | "x86_64" => "x86_64",
        other => other,
    }
}

#[must_use]
pub fn render_line(
    name: &str,
    version: &str,
    build: Option<&str>,
    os: &str,
    os_version: &str,
    arch: &str,
) -> String {
    let mut line = format!("{name} {}", version.trim_start_matches('v'));
    if let Some(build) = build.filter(|value| !value.is_empty()) {
        line.push_str(&format!(" ({build})"));
    }
    if !os.is_empty() {
        line.push_str(SEPARATOR);
        line.push_str(os);
        if !os_version.is_empty() {
            line.push(' ');
            line.push_str(os_version);
        }
    }
    if !arch.is_empty() {
        line.push_str(SEPARATOR);
        line.push_str(normalize_arch(arch));
    }
    line
}

impl About {
    #[must_use]
    pub fn from_facts(
        version: &str,
        build: Option<String>,
        os: String,
        os_version: String,
        arch: String,
    ) -> Self {
        let about = render_line(
            "journal",
            version,
            build.as_deref(),
            &os,
            &os_version,
            &arch,
        );
        Self {
            protocol_version: 1,
            version: version.trim_start_matches('v').into(),
            build,
            os,
            os_version,
            arch: normalize_arch(&arch).into(),
            about,
        }
    }
}

/// Read only public facts about this installed runtime and its native machine.
#[must_use]
pub fn host_about(version: &str) -> About {
    #[cfg(target_os = "linux")]
    {
        let platform = std::fs::read_to_string("/etc/os-release")
            .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"));
        let uname = nix::sys::utsname::uname().ok();
        let (os, os_version) = match platform {
            Ok(text) => parse_os_release(&text),
            Err(error) => {
                log::warn!("about: os-release unavailable: {error}");
                (
                    "linux".into(),
                    uname
                        .as_ref()
                        .map(|value| value.release().to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
            }
        };
        let arch = uname
            .as_ref()
            .map(|value| value.machine().to_string_lossy().into_owned())
            .unwrap_or_default();
        About::from_facts(version, None, os, os_version, arch)
    }
    #[cfg(target_os = "macos")]
    {
        macos::host_about(version)
    }
    #[cfg(windows)]
    {
        windows::host_about(version)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        About::from_facts(
            version,
            None,
            std::env::consts::OS.into(),
            String::new(),
            std::env::consts::ARCH.into(),
        )
    }
}

#[must_use]
pub fn parse_os_release(text: &str) -> (String, String) {
    let value = |key: &str| {
        text.lines().find_map(|line| {
            let (found, value) = line.split_once('=')?;
            if found != key {
                return None;
            }
            let value = value.trim().trim_matches(['"', '\'']);
            (!value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)))
            .then(|| value.to_owned())
        })
    };
    (
        value("ID").unwrap_or_else(|| "linux".into()),
        value("VERSION_ID").unwrap_or_default(),
    )
}

#[must_use]
pub fn windows_version(build: u32) -> String {
    format!("{} {build}", if build >= 22000 { "11" } else { "10" })
}

/// Apple Silicon remains the native machine when the executable runs under Rosetta.
#[must_use]
pub fn native_macos_arch(translated: Option<bool>, machine: Option<&str>) -> &str {
    match translated {
        Some(true) => "arm64",
        Some(false) => machine.map(normalize_arch).unwrap_or_default(),
        None => "",
    }
}

/// IMAGE_FILE_MACHINE values describe the host, independently of the process slice.
#[must_use]
pub fn native_windows_arch(_process_machine: u16, native_machine: u16) -> &'static str {
    match native_machine {
        0xaa64 => "arm64",
        0x8664 => "x86_64",
        0x014c => "x86",
        0x01c4 => "arm",
        _ => "",
    }
}

/// Code authority for the committed language-neutral contract and fixtures.
/// Regenerate with the about-contract example; consumers vendor these bytes.
#[must_use]
pub fn contract() -> Value {
    json!({
        "generated_from": "solstone-core-about::contract",
        "regenerate": "cargo run --locked --manifest-path core/Cargo.toml -p solstone-core-about --example about-contract",
        "no_retyping": true,
        "protocol_version": 1,
        "endpoint": "/api/system/about",
        "fragment_key": "about",
        "separator": SEPARATOR,
        "names": COMPONENT_NAMES,
        "arch_aliases": { "arm64": ["arm64", "aarch64", "ARM64", "arm64-v8a"], "x86_64": ["x86_64", "amd64", "x64", "AMD64"] },
        "fields": ["protocol_version", "version", "build?", "os", "os_version", "arch", "about"],
        "unknown_facts": "empty string, omitted with separator in rendered text; build absent",
        "bundle_version": "1.0.0",
        "fixtures": [
            {"version":"1.2.3","build":"42","os":"macos","os_version":"15.6","arch":"aarch64","about":"journal 1.2.3 (42) · macos 15.6 · arm64"},
            {"version":"1.2.3","os":"macos","os_version":"15.6","arch":"arm64","about":"journal 1.2.3 · macos 15.6 · arm64"},
            {"version":"1.2.3","os":"ubuntu","os_version":"24.04","arch":"amd64","about":"journal 1.2.3 · ubuntu 24.04 · x86_64"},
            {"version":"1.2.3","os":"linux","os_version":"6.11.0","arch":"riscv64","about":"journal 1.2.3 · linux 6.11.0 · riscv64"},
            {"version":"1.2.3","os":"windows","os_version":"10 19045","arch":"AMD64","about":"journal 1.2.3 · windows 10 19045 · x86_64"},
            {"version":"1.2.3","os":"windows","os_version":"11 26100","arch":"ARM64","about":"journal 1.2.3 · windows 11 26100 · arm64"},
            {"version":"1.2.3","os":"linux","os_version":"","arch":"","about":"journal 1.2.3 · linux"},
            {"version":"1.2.3","os":"","os_version":"","arch":"","about":"journal 1.2.3"},
        ],
    })
}

#[must_use]
pub fn api_schema() -> Value {
    json!({
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "$id":"https://solstone.app/schemas/about/v1.json",
        "type":"object", "additionalProperties":true,
        "required":["protocol_version","version","os","os_version","arch","about"],
        "properties": {
            "protocol_version":{"const":1}, "version":{"type":"string","minLength":1},
            "build":{"type":"string","minLength":1}, "os":{"type":"string"},
            "os_version":{"type":"string"}, "arch":{"type":"string"}, "about":{"type":"string","minLength":1}
        }
    })
}

#[must_use]
pub fn resource_fixtures() -> Value {
    let valid = About::from_facts(
        "1.2.3",
        None,
        "ubuntu".into(),
        "24.04".into(),
        "x86_64".into(),
    );
    let mut wrong_version = serde_json::to_value(&valid).expect("about serializes");
    wrong_version["protocol_version"] = json!(2);
    let mut wrong_type = serde_json::to_value(&valid).expect("about serializes");
    wrong_type["arch"] = json!(64);
    json!({"valid":[valid], "invalid":[{}, wrong_version, wrong_type],
        "behavior":{
            "missing_endpoint":"retain current version-only line; 404 does not change metadata freshness",
            "failed_host_read":"retain current version-only line; host failure does not change metadata freshness",
            "disconnected":"retain identity-bound last-known facts and append last seen relative time",
            "unknown_version":"journal unknown",
            "unknown_arch":"extensible, retain verbatim",
            "names":"closed; only the declared component names",
            "native_arch":"native machine; omit on failed observation, never process slice"
        }, "consumer_families":["macos","ios","watch","android","windows","linux","tmux","extension","journal-ui","support"]})
}

/// Optional native-browser host snapshot; the enclosing native envelope remains extensible.
#[must_use]
pub fn native_snapshot_schema() -> Value {
    json!({
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "$id":"https://solstone.app/schemas/native-about/v1.json",
        "type":"object", "additionalProperties":false,
        "required":["protocol_version","os","os_version","arch","journal_line","journal_current","journal_seen_at_epoch_secs"],
        "properties": {
            "protocol_version":{"const":1},
            "os":{"type":"string","pattern":"^[^\\r\\n]*$"},
            "os_version":{"type":"string","pattern":"^[^\\r\\n]*$"},
            "arch":{"type":"string","pattern":"^[^\\r\\n]*$"},
            "journal_line":{"type":"string","pattern":"^journal [^\\r\\n]+$","minLength":9,"maxLength":8192,
                "not":{"pattern":" · last seen "}},
            "journal_current":{"type":"boolean"},
            "journal_seen_at_epoch_secs":{"type":["integer","null"],"minimum":0}
        },
        "if":{"properties":{"journal_line":{"const":"journal unknown"}}},
        "then":{"properties":{"journal_current":{"const":false},"journal_seen_at_epoch_secs":{"type":"null"}}}
    })
}

#[must_use]
pub fn native_snapshot_fixtures() -> Value {
    let current = json!({"protocol_version":1,"os":"windows","os_version":"11 26100","arch":"arm64",
        "journal_line":"journal 2.0.29 · ubuntu 24.04 · x86_64","journal_current":true,"journal_seen_at_epoch_secs":1700000000});
    let unknown = json!({"protocol_version":1,"os":"ubuntu","os_version":"24.04","arch":"x86_64",
        "journal_line":"journal unknown","journal_current":false,"journal_seen_at_epoch_secs":null});
    let mut private = current.clone();
    private["hostname"] = json!("PRIVATE HOST");
    let mut wrong_type = current.clone();
    wrong_type["journal_current"] = json!("true");
    let mut wrong_major = current.clone();
    wrong_major["protocol_version"] = json!(2);
    let mut invalid_unknown = unknown.clone();
    invalid_unknown["journal_current"] = json!(true);
    let mut extra_line = current.clone();
    extra_line["journal_line"] = json!("journal 2.0.29\nPRIVATE HOST");
    let mut already_stale = current.clone();
    already_stale["journal_line"] = json!("journal 2.0.29 · last seen 2 days ago");
    json!({
        "valid":[current, unknown],"invalid":[{}, private, wrong_type, wrong_major, invalid_unknown, extra_line, already_stale],
        "envelopes": [
            {"type":"state","capture":"permitted","delivery":"idle","freshness_ms":15000,"destination_generation":"fixture-a","period_id":"fixture-period","about":current},
            {"type":"hello_ack","capture":"not_paired","delivery":"unknown","freshness_ms":0,"destination_generation":null,"period_id":null,"about":unknown},
            {"type":"state","capture":"permitted","delivery":"idle","freshness_ms":15000,"destination_generation":"fixture-a","period_id":"fixture-period"},
            {"type":"state","capture":"permitted","delivery":"idle","freshness_ms":15000,"destination_generation":"fixture-a","period_id":"fixture-period","future_root":true,"about":current},
            {"type":"state","capture":"permitted","delivery":"idle","freshness_ms":15000,"destination_generation":"fixture-a","period_id":"fixture-period","about":private}
        ],
        "invalid_core_envelopes": [
            {"type":"state","delivery":"idle","freshness_ms":15000,"destination_generation":"fixture-a","period_id":"fixture-period","about":current},
            {"type":"state","capture":"permitted","delivery":"idle","destination_generation":"fixture-a","period_id":"fixture-period","about":current}
        ],
        "behavior": [
            {"case":"cold or new native port without valid snapshot","journal_line":"journal unknown","retain":false},
            {"case":"same port AND destination, absent or invalid state","retain":true,"journal_current":false,"renew_observation":false},
            {"case":"same port, different destination, absent or invalid state","journal_line":"journal unknown","retain":false},
            {"case":"explicit valid unknown reset","journal_line":"journal unknown","retain":false,"retain_incoming_native_facts":true},
            {"case":"native port disconnect OR existing state freshness deadline expires","retain":true,"journal_current":false,"renew_observation":false},
            {"case":"identical valid retransmission","renew_observation":false},
            {"case":"null observation timestamp","invent_age":false},
            {"case":"future envelope root key","core_accept":true,"snapshot_accept":true},
            {"case":"extra nested snapshot key","core_accept":true,"snapshot_accept":false}
            ,{"case":"invalid core envelope, valid snapshot","core_accept":false,"snapshot_accept":false}
            ,{"case":"zero freshness OR completion after receipt deadline","journal_current":false,"renew_observation":false}
            ,{"case":"older completion after newer accepted state","accept":false,"renew_observation":false}
            ,{"case":"destination transition pending at first status publication","exclude_prior_journal":true}
        ]
    })
}

fn artifact_bytes(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("contract serializes");
    bytes.push(b'\n');
    bytes
}

#[must_use]
pub fn bundle_artifacts() -> Vec<(&'static str, Vec<u8>)> {
    let artifacts = vec![
        ("contract.json", artifact_bytes(&contract())),
        ("about.schema.json", artifact_bytes(&api_schema())),
        ("resources.json", artifact_bytes(&resource_fixtures())),
        (
            "native-about.schema.json",
            artifact_bytes(&native_snapshot_schema()),
        ),
        (
            "native-about.json",
            artifact_bytes(&native_snapshot_fixtures()),
        ),
    ];
    let digest = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let inputs = [
        ("src/lib.rs", include_bytes!("lib.rs").as_slice()),
        ("src/macos.rs", include_bytes!("macos.rs").as_slice()),
        ("src/windows.rs", include_bytes!("windows.rs").as_slice()),
        ("Cargo.toml", include_bytes!("../Cargo.toml").as_slice()),
        (
            "examples/contract.rs",
            include_bytes!("../examples/contract.rs").as_slice(),
        ),
    ];
    let manifest = json!({"bundle_version":"1.0.0", "generator":"solstone-core-about::bundle_artifacts", "schema_dialect":"https://json-schema.org/draft/2020-12/schema",
        "regenerate":contract()["regenerate"],
        "inputs":inputs.iter().map(|(path, bytes)| (path.to_string(), Value::String(digest(bytes)))).collect::<serde_json::Map<_,_>>(),
        "artifacts":artifacts.iter().map(|(path, bytes)| (path.to_string(), Value::String(digest(bytes)))).collect::<serde_json::Map<_,_>>() });
    let mut result = artifacts;
    result.push(("manifest.json", artifact_bytes(&manifest)));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distro_uses_marketing_facts_and_not_descriptive_identity() {
        assert_eq!(
            parse_os_release("NAME=\"Owner's private distro\"\nID=ubuntu\nVERSION_ID=\"24.04\"\n"),
            ("ubuntu".into(), "24.04".into())
        );
        assert_eq!(
            parse_os_release("ID=fedora\nVERSION_ID=43\n"),
            ("fedora".into(), "43".into())
        );
        assert_eq!(
            parse_os_release("NAME=Linux\n"),
            ("linux".into(), String::new())
        );
    }

    #[test]
    fn aliases_and_windows_marketing_version_are_normalized() {
        for raw in ["aarch64", "ARM64", "arm64-v8a"] {
            assert_eq!(normalize_arch(raw), "arm64");
        }
        for raw in ["amd64", "x64", "AMD64"] {
            assert_eq!(normalize_arch(raw), "x86_64");
        }
        assert_eq!(normalize_arch("riscv64"), "riscv64");
        assert_eq!(windows_version(19045), "10 19045");
        assert_eq!(windows_version(22000), "11 22000");
    }

    #[test]
    fn machine_architecture_does_not_follow_the_emulated_process() {
        assert_eq!(native_windows_arch(0x8664, 0xaa64), "arm64");
        assert_eq!(native_windows_arch(0, 0x8664), "x86_64");
        assert_eq!(native_windows_arch(0, 0), "");
        assert_eq!(native_macos_arch(Some(true), Some("x86_64")), "arm64");
        assert_eq!(native_macos_arch(Some(false), Some("x86_64")), "x86_64");
        assert_eq!(native_macos_arch(Some(false), Some("arm64")), "arm64");
        assert_eq!(native_macos_arch(Some(false), Some("riscv64")), "riscv64");
        assert_eq!(native_macos_arch(Some(false), None), "");
        assert_eq!(native_macos_arch(None, Some("x86_64")), "");
    }

    #[test]
    fn missing_fields_do_not_create_empty_separators() {
        assert_eq!(
            render_line("journal", "1.2.3", None, "", "", ""),
            "journal 1.2.3"
        );
        assert_eq!(
            render_line("journal", "1.2.3", None, "macos", "15.6", "aarch64"),
            "journal 1.2.3 · macos 15.6 · arm64"
        );
    }

    #[test]
    fn independent_fixture_lines_match_the_locked_grammar() {
        for fixture in contract()["fixtures"].as_array().unwrap() {
            let string = |key: &str| fixture[key].as_str().unwrap_or_default();
            assert_eq!(
                render_line(
                    "journal",
                    string("version"),
                    fixture["build"].as_str(),
                    string("os"),
                    string("os_version"),
                    string("arch")
                ),
                string("about")
            );
        }
    }

    #[test]
    fn generated_bundle_matches_committed_artifacts_and_rejects_invalid_resources() {
        let committed = [
            (
                "contract.json",
                include_bytes!("../bundle/contract.json").as_slice(),
            ),
            (
                "about.schema.json",
                include_bytes!("../bundle/about.schema.json").as_slice(),
            ),
            (
                "resources.json",
                include_bytes!("../bundle/resources.json").as_slice(),
            ),
            (
                "native-about.schema.json",
                include_bytes!("../bundle/native-about.schema.json").as_slice(),
            ),
            (
                "native-about.json",
                include_bytes!("../bundle/native-about.json").as_slice(),
            ),
            (
                "manifest.json",
                include_bytes!("../bundle/manifest.json").as_slice(),
            ),
        ];
        for ((name, actual), (expected_name, expected)) in
            bundle_artifacts().into_iter().zip(committed)
        {
            assert_eq!(name, expected_name);
            assert_eq!(actual, expected, "regenerate the about bundle");
        }
        let schema = jsonschema::validator_for(&api_schema()).unwrap();
        let fixtures = resource_fixtures();
        for valid in fixtures["valid"].as_array().unwrap() {
            assert!(schema.is_valid(valid));
        }
        for invalid in fixtures["invalid"].as_array().unwrap() {
            assert!(!schema.is_valid(invalid));
        }
    }

    #[test]
    fn native_snapshot_schema_keeps_private_fields_out_and_requires_real_freshness() {
        let schema = jsonschema::validator_for(&native_snapshot_schema()).unwrap();
        let fixtures = native_snapshot_fixtures();
        for valid in fixtures["valid"].as_array().unwrap() {
            assert!(schema.is_valid(valid));
        }
        for invalid in fixtures["invalid"].as_array().unwrap() {
            assert!(!schema.is_valid(invalid));
        }
    }

    #[test]
    fn committed_contract_is_the_generated_code_contract() {
        let committed: Value =
            serde_json::from_str(include_str!("../bundle/contract.json")).unwrap();
        assert_eq!(committed, contract());
    }
}
