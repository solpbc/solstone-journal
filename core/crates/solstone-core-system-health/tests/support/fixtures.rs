// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;
use sha2::{Digest, Sha256};

const HEALTH_TEXT_JSON: &str = include_str!("../../../../fixtures/health_text_reference.json");
const HEALTH_LOGS_JSON: &str = include_str!("../../../../fixtures/health_logs_reference.json");
pub const HEALTH_TEXT_SHA256: &str =
    "b0c3ac7312aea7e017c5807c2f531b7463b8a416f78ca3a1d7c63cd6536f664d";
pub const HEALTH_LOGS_SHA256: &str =
    "e7282efe72618ad6ff375fdd4065a7e60e3151a9b210f6c7a00377880a596a4b";

static HEALTH_TEXT: OnceLock<HealthTextFixture> = OnceLock::new();
static HEALTH_LOGS: OnceLock<HealthLogsFixture> = OnceLock::new();

pub fn health_text_fixture() -> &'static HealthTextFixture {
    HEALTH_TEXT.get_or_init(|| {
        assert_eq!(
            health_text_raw_sha256(),
            HEALTH_TEXT_SHA256,
            "health text fixture digest"
        );
        parse_health_text_fixture(HEALTH_TEXT_JSON).expect("health text fixture must be valid")
    })
}
pub fn health_logs_fixture() -> &'static HealthLogsFixture {
    HEALTH_LOGS.get_or_init(|| {
        assert_eq!(
            health_logs_raw_sha256(),
            HEALTH_LOGS_SHA256,
            "health logs fixture digest"
        );
        parse_health_logs_fixture(HEALTH_LOGS_JSON).expect("health logs fixture must be valid")
    })
}
pub fn health_text_raw_sha256() -> String {
    format!("{:x}", Sha256::digest(HEALTH_TEXT_JSON.as_bytes()))
}
pub fn health_logs_raw_sha256() -> String {
    format!("{:x}", Sha256::digest(HEALTH_LOGS_JSON.as_bytes()))
}
pub fn parse_health_text_fixture(input: &str) -> serde_json::Result<HealthTextFixture> {
    serde_json::from_str(input)
}
pub fn parse_health_logs_fixture(input: &str) -> serde_json::Result<HealthLogsFixture> {
    serde_json::from_str(input)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthTextFixture {
    pub decimal_cases: Vec<(u32, u8, ResultCase, ResultCase)>,
    pub port_cases: Vec<PortCase>,
    pub provenance: Provenance,
    pub runtime: TextRuntime,
    pub scalar_cases: Vec<RecipeCase>,
    pub schema: u32,
    pub unsafe_unicode: UnsafeUnicode,
    pub whitespace_cases: Vec<(u32, ResultCase)>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultCase {
    pub kind: String,
    pub value: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeCase {
    pub id: String,
    pub recipe: serde_json::Value,
    pub result: serde_json::Value,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortCase {
    pub id: String,
    pub argv: serde_json::Value,
    pub result: serde_json::Value,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub capture_tool: SourceIdentity,
    pub health_fixture: SourceIdentity,
    pub service_source: SourceIdentity,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    pub path: String,
    pub sha256: String,
    pub git_blob: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextRuntime {
    pub executable_sha256: String,
    pub int_max_str_digits: u32,
    pub python: String,
    pub unicode: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnsafeUnicode {
    pub categories: BTreeMap<String, Vec<u32>>,
    pub counts: BTreeMap<String, usize>,
    pub ranges: Vec<UnsafeRange>,
}
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UnsafeRange {
    pub start: u32,
    pub end: u32,
    pub lower: Option<u32>,
    pub upper: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthLogsFixture {
    pub schema: u32,
    pub source: LogSource,
    pub runtime: LogRuntime,
    pub rows: Vec<serde_json::Value>,
    pub since: Vec<serde_json::Value>,
    pub regex: Vec<RegexCase>,
    pub unicode_contract: UnicodeContract,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogSource {
    pub path: String,
    pub sha256: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogRuntime {
    pub executable_sha256: String,
    pub fixed_now: String,
    pub python: String,
    pub unicode: String,
}
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum RegexCase {
    Outcome(RegexOutcome),
    Error(RegexError),
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegexOutcome {
    pub pattern: String,
    pub haystacks: Vec<String>,
    pub matches: Vec<bool>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegexError {
    pub pattern: String,
    pub haystacks: Vec<String>,
    pub error: String,
    pub error_type: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnicodeContract {
    pub whitespace_codepoints: Vec<u32>,
    pub decimal_codepoints: Vec<(u32, u8)>,
    pub decimal_zero_codepoints: Vec<u32>,
}

pub fn assert_fixture_shapes() {
    let text = health_text_fixture();
    assert_eq!(text.schema, 2);
    assert_eq!(text.decimal_cases.len(), 760);
    assert_eq!(text.port_cases.len(), 13);
    assert_eq!(text.scalar_cases.len(), 49);
    assert_eq!(text.whitespace_cases.len(), 29);
    assert_eq!(text.runtime.unicode, "16.0.0");
    assert_eq!(text.unsafe_unicode.counts.get("Cc"), Some(&65));
    assert_eq!(text.unsafe_unicode.counts.get("Cf"), Some(&170));
    assert_eq!(text.unsafe_unicode.counts.get("Zl"), Some(&1));
    assert_eq!(text.unsafe_unicode.counts.get("Zp"), Some(&1));
    assert_eq!(text.unsafe_unicode.counts.get("union"), Some(&237));
    assert_eq!(text.unsafe_unicode.ranges.len(), 23);
    let logs = health_logs_fixture();
    assert_eq!(logs.schema, 1);
    assert_eq!(logs.regex.len(), 36);
    assert_eq!(logs.unicode_contract.decimal_codepoints.len(), 760);
}
