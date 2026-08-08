// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_body_source::{BodyDigest, BodyMonth, BundleId, EnvelopeShard};

mod support;

use support::{envelope_multimonth_fixture, native_bundle_fixture};

fn month_from_fixture_path(path: &str) -> BodyMonth {
    let month = path
        .strip_prefix("normalized/")
        .and_then(|value| value.strip_suffix(".jsonl"))
        .expect("fixture shard path has normalized month form");
    BodyMonth::from_bytes(month.as_bytes()).expect("fixture shard month is valid")
}

fn assert_shard_matches_fixture(shard: &EnvelopeShard, expected: &Value) {
    assert_eq!(shard.path(), expected["path"].as_str().expect("shard path"));
    assert_eq!(
        shard.month().as_str(),
        month_from_fixture_path(shard.path()).as_str()
    );
    assert_eq!(
        shard.bytes(),
        expected["bytes"].as_u64().expect("shard bytes")
    );
    assert_eq!(shard.rows(), expected["rows"].as_u64().expect("shard rows"));
    assert_eq!(
        shard.sha256().as_str(),
        expected["sha256"].as_str().expect("shard digest")
    );
}

#[test]
fn envelope_shard_fixture_matches_native_bundle_one_row_descriptors() {
    let fixture = native_bundle_fixture();
    let mut descriptors = 0;

    for case in fixture["cases"].as_array().expect("fixture cases") {
        let name = case["name"].as_str().expect("case name");
        let envelope: Value = serde_json::from_str(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("expected envelope JSONL"),
        )
        .expect("expected envelope JSONL parses");
        let shards = envelope["shards"].as_array().expect("envelope shards");
        let bundle = BundleId::from_bytes(
            case["directory"]
                .as_str()
                .expect("case directory")
                .as_bytes(),
        )
        .expect("fixture directory is a valid bundle ID");

        match name {
            "apple_retain_complete_one_row" | "oura_retain_parsed_one_row" => {
                assert_eq!(shards.len(), 1, "{name} has one shard");
            }
            "apple_discard_zero_rows" | "oura_discard_zero_rows" => {
                assert!(shards.is_empty(), "{name} has no shards");
            }
            _ => panic!("unexpected fixture case {name}"),
        }

        for (index, expected) in shards.iter().enumerate() {
            let shard = EnvelopeShard::new(
                &bundle,
                index as u64,
                month_from_fixture_path(expected["path"].as_str().expect("shard path")),
                expected["bytes"].as_u64().expect("shard bytes"),
                expected["rows"].as_u64().expect("shard rows"),
                BodyDigest::from_bytes(
                    expected["sha256"]
                        .as_str()
                        .expect("shard digest")
                        .as_bytes(),
                )
                .expect("fixture shard digest is valid"),
            )
            .unwrap_or_else(|error| panic!("{name} shard {index} should bind: {error}"));
            assert_shard_matches_fixture(&shard, expected);
            descriptors += 1;
        }
    }

    assert_eq!(descriptors, 2);
}

#[test]
fn envelope_shard_fixture_matches_multimonth_descriptors() {
    let fixture = envelope_multimonth_fixture();
    let case = &fixture["cases"][0];
    let bundle = BundleId::from_bytes(
        case["directory"]
            .as_str()
            .expect("case directory")
            .as_bytes(),
    )
    .expect("fixture directory is a valid bundle ID");
    let shards = case["expected_envelope"]["shards"]
        .as_array()
        .expect("expected envelope shards");
    let expected = [
        (
            "normalized/2026-01.jsonl",
            "2026-01",
            62,
            2,
            "sha256:1d9964c8896214915223fc1c7730e41a4e759ad9f84271c6df8cf449c6a72ccf",
        ),
        (
            "normalized/2026-02.jsonl",
            "2026-02",
            31,
            1,
            "sha256:6eda3b0e6121bd3952de3871bb0ffd7ad6e4da93f02cf1bb1490118d7f37b655",
        ),
    ];
    assert_eq!(shards.len(), expected.len());

    for (index, (fixture_shard, expected)) in shards.iter().zip(expected).enumerate() {
        let (path, month, bytes, rows, sha256) = expected;
        assert_eq!(fixture_shard["path"], path);
        assert_eq!(fixture_shard["bytes"], bytes);
        assert_eq!(fixture_shard["rows"], rows);
        assert_eq!(fixture_shard["sha256"], sha256);

        let shard = EnvelopeShard::new(
            &bundle,
            index as u64,
            BodyMonth::from_bytes(month.as_bytes()).expect("expected month is valid"),
            bytes,
            rows,
            BodyDigest::from_bytes(sha256.as_bytes()).expect("expected digest is valid"),
        )
        .expect("fixture shard should bind");
        assert_eq!(shard.path(), path);
        assert_eq!(shard.month().as_str(), month);
        assert_eq!(shard.bytes(), bytes);
        assert_eq!(shard.rows(), rows);
        assert_eq!(shard.sha256().as_str(), sha256);
    }
}
