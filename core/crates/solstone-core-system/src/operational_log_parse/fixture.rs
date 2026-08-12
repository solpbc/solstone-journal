// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;

use serde::Deserialize;
use sha2::{Digest, Sha256};

const FIXTURE_JSON: &str = include_str!("../../../../fixtures/health_logs_reference.json");
pub(crate) const FIXTURE_SHA256: &str =
    "e7282efe72618ad6ff375fdd4065a7e60e3151a9b210f6c7a00377880a596a4b";

static FIXTURE: OnceLock<HealthLogsFixture> = OnceLock::new();

pub(crate) fn fixture() -> &'static HealthLogsFixture {
    FIXTURE.get_or_init(|| {
        assert_eq!(raw_sha256(), FIXTURE_SHA256, "health logs fixture digest");
        parse_json(FIXTURE_JSON).expect("core/fixtures/health_logs_reference.json must be valid")
    })
}

pub(crate) fn parse_json(input: &str) -> serde_json::Result<HealthLogsFixture> {
    serde_json::from_str(input)
}

pub(crate) fn raw_sha256() -> String {
    format!("{:x}", Sha256::digest(FIXTURE_JSON.as_bytes()))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct HealthLogsFixture {
    pub schema: u32,
    pub source: SourceFixture,
    pub runtime: RuntimeFixture,
    pub rows: Vec<RowCase>,
    pub since: Vec<SinceCase>,
    pub regex: Vec<RegexCase>,
    pub unicode_contract: UnicodeContract,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct SourceFixture {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct RuntimeFixture {
    pub executable_sha256: String,
    pub fixed_now: String,
    pub python: String,
    pub unicode: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct RowCase {
    pub input: String,
    pub outcome: Option<RowOutcome>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct RowOutcome {
    pub timestamp: String,
    pub service: String,
    pub stream: String,
    pub message: String,
    pub raw: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
#[allow(dead_code)]
pub(crate) enum SinceCase {
    Outcome(SinceOutcomeCase),
    Error(SinceErrorCase),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct SinceOutcomeCase {
    pub input: String,
    pub outcome: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct SinceErrorCase {
    pub input: String,
    pub error: String,
    pub error_type: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
#[allow(dead_code)]
pub(crate) enum RegexCase {
    Outcome(RegexOutcomeCase),
    Error(RegexErrorCase),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct RegexOutcomeCase {
    pub pattern: String,
    pub haystacks: Vec<String>,
    pub matches: Vec<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct RegexErrorCase {
    pub pattern: String,
    pub haystacks: Vec<String>,
    pub error: String,
    pub error_type: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct UnicodeContract {
    pub whitespace_codepoints: Vec<u32>,
    pub decimal_codepoints: Vec<(u32, u8)>,
    pub decimal_zero_codepoints: Vec<u32>,
}
