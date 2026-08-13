// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;
use sha2::{Digest, Sha256};

const FIXTURE_JSON: &str = include_str!("../../../fixtures/health_log_io_reference.json");
pub(crate) const FIXTURE_SHA256: &str =
    "eb201053feed6cbef343ae737ae905075b7b234d1cfb35fe2684b2131bc27001";

static FIXTURE: OnceLock<HealthLogIoFixture> = OnceLock::new();

pub(crate) fn fixture() -> &'static HealthLogIoFixture {
    FIXTURE.get_or_init(|| {
        assert_eq!(
            raw_sha256(),
            FIXTURE_SHA256,
            "health log I/O fixture digest"
        );
        let fixture = parse_json(FIXTURE_JSON)
            .expect("core/fixtures/health_log_io_reference.json must be valid");
        assert_eq!(fixture.schema, "solstone.health-log-io-reference.v1");
        assert_eq!(fixture.cases.len(), 57);
        assert_eq!(fixture.metadata.case_count, 57);
        let actual_family_counts =
            fixture
                .cases
                .iter()
                .fold(BTreeMap::new(), |mut counts, case| {
                    *counts.entry(case.family.as_str()).or_insert(0_usize) += 1;
                    counts
                });
        assert_eq!(actual_family_counts.get("today_health_directory"), Some(&7));
        assert_eq!(actual_family_counts.get("ordinary_tail"), Some(&14));
        assert_eq!(actual_family_counts.get("reverse_tail"), Some(&31));
        assert_eq!(actual_family_counts.get("day_log_enumeration"), Some(&5));
        assert_eq!(
            fixture.metadata.family_counts.get("today_health_directory"),
            Some(&7)
        );
        assert_eq!(
            fixture.metadata.family_counts.get("ordinary_tail"),
            Some(&14)
        );
        assert_eq!(
            fixture.metadata.family_counts.get("reverse_tail"),
            Some(&31)
        );
        assert_eq!(
            fixture.metadata.family_counts.get("day_log_enumeration"),
            Some(&5)
        );
        fixture
    })
}

pub(crate) fn parse_json(input: &str) -> serde_json::Result<HealthLogIoFixture> {
    serde_json::from_str(input)
}

pub(crate) fn raw_sha256() -> String {
    format!("{:x}", Sha256::digest(FIXTURE_JSON.as_bytes()))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HealthLogIoFixture {
    pub schema: String,
    pub metadata: Metadata,
    pub cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct Metadata {
    pub capture_script_sha256: String,
    pub case_count: usize,
    pub chunk_size: usize,
    pub family_counts: BTreeMap<String, usize>,
    pub platform: String,
    pub python_executable_sha256: String,
    pub python_version: String,
    pub reference_commit: String,
    pub reference_git_blob_oid: String,
    pub reference_path: String,
    pub reference_sha256: String,
    pub reference_utils_git_blob_oid: String,
    pub reference_utils_path: String,
    pub reference_utils_sha256: String,
    pub unicode_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct Case {
    pub family: String,
    pub id: String,
    #[serde(default)]
    pub input: Option<serde_json::Value>,
    #[serde(default)]
    pub outcome: Option<serde_json::Value>,
    #[serde(default)]
    pub calls: Option<serde_json::Value>,
    #[serde(default)]
    pub forward: Option<serde_json::Value>,
    #[serde(default)]
    pub reverse: Option<serde_json::Value>,
    #[serde(default)]
    pub non_utf8_setup: Option<serde_json::Value>,
    #[serde(default)]
    pub injected_boundary: Option<String>,
}
