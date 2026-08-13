// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_general_category::{GeneralCategory, get_general_category};

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
    let value: serde_json::Value = serde_json::from_str(input)?;
    validate_health_text_json_shape(&value)
        .map_err(<serde_json::Error as serde::de::Error>::custom)?;
    serde_json::from_value(value)
}

fn validate_health_text_json_shape(value: &serde_json::Value) -> Result<(), String> {
    let root = object(value, "fixture")?;
    exact_keys(
        root,
        &[
            "decimal_cases",
            "port_cases",
            "provenance",
            "runtime",
            "scalar_cases",
            "schema",
            "unsafe_unicode",
            "whitespace_cases",
        ],
        "fixture",
    )?;
    let provenance = object(&root["provenance"], "provenance")?;
    exact_keys(
        provenance,
        &["capture_tool", "health_fixture", "service_source"],
        "provenance",
    )?;
    for name in ["capture_tool", "health_fixture", "service_source"] {
        let source = object(&provenance[name], name)?;
        let expected = if name == "health_fixture" {
            &["path", "sha256"][..]
        } else {
            &["git_blob", "path", "sha256"][..]
        };
        exact_keys(source, expected, name)?;
    }
    exact_keys(
        object(&root["runtime"], "runtime")?,
        &[
            "executable_sha256",
            "int_max_str_digits",
            "python",
            "unicode",
        ],
        "runtime",
    )?;
    let unsafe_unicode = object(&root["unsafe_unicode"], "unsafe_unicode")?;
    exact_keys(
        unsafe_unicode,
        &["categories", "counts", "ranges"],
        "unsafe_unicode",
    )?;
    for range in array(&unsafe_unicode["ranges"], "ranges")? {
        exact_keys(
            object(range, "range")?,
            &["end", "lower", "start", "upper"],
            "range",
        )?;
    }
    for row in array(&root["scalar_cases"], "scalar_cases")? {
        let row = object(row, "scalar row")?;
        exact_keys(row, &["id", "recipe", "result"], "scalar row")?;
        validate_recipe_shape(&row["recipe"])?;
        validate_result_shape(&row["result"], "scalar result")?;
    }
    for row in array(&root["port_cases"], "port_cases")? {
        let row = object(row, "port row")?;
        exact_keys(row, &["argv", "id", "result"], "port row")?;
        let argv = object(&row["argv"], "port argv")?;
        match argv.get("kind").and_then(serde_json::Value::as_str) {
            Some("text") => exact_keys(argv, &["kind", "values"], "text argv")?,
            Some("codepoints") => {
                exact_keys(argv, &["kind", "prefix", "values"], "codepoint argv")?
            }
            Some("surrogateescape") => exact_keys(
                argv,
                &["bytes_hex", "kind", "prefix"],
                "surrogateescape argv",
            )?,
            _ => return Err("unknown port argv kind".to_owned()),
        }
        validate_port_result_shape(&row["result"])?;
    }
    for row in array(&root["decimal_cases"], "decimal_cases")? {
        let row = array(row, "decimal row")?;
        if row.len() != 4 {
            return Err("decimal row keys differ".to_owned());
        }
        validate_result_shape(&row[2], "decimal result")?;
        validate_result_shape(&row[3], "decimal result")?;
    }
    for row in array(&root["whitespace_cases"], "whitespace_cases")? {
        let row = array(row, "whitespace row")?;
        if row.len() != 2 {
            return Err("whitespace row keys differ".to_owned());
        }
        validate_result_shape(&row[1], "whitespace result")?;
    }
    Ok(())
}

fn validate_recipe_shape(value: &serde_json::Value) -> Result<(), String> {
    let recipe = object(value, "scalar recipe")?;
    match recipe.get("kind").and_then(serde_json::Value::as_str) {
        Some("literal") => exact_keys(recipe, &["kind", "text"], "literal recipe"),
        Some("codepoints") => exact_keys(recipe, &["kind", "values"], "codepoint recipe"),
        Some("repeat") => exact_keys(
            recipe,
            &[
                "codepoint",
                "count",
                "kind",
                "leading",
                "separator",
                "sign",
                "trailing",
            ],
            "repeat recipe",
        ),
        _ => Err("unknown scalar recipe kind".to_owned()),
    }
}

fn validate_result_shape(value: &serde_json::Value, context: &str) -> Result<(), String> {
    let result = object(value, context)?;
    match result.get("kind").and_then(serde_json::Value::as_str) {
        Some("value") => exact_keys(result, &["kind", "value"], context),
        Some("ValueError") => exact_keys(result, &["kind"], context),
        _ => Err(format!("unknown {context} kind")),
    }
}

fn validate_port_result_shape(value: &serde_json::Value) -> Result<(), String> {
    let result = object(value, "port result")?;
    match result.get("kind").and_then(serde_json::Value::as_str) {
        Some("return") => exact_keys(result, &["kind", "value"], "return result"),
        Some("exit") => exact_keys(
            result,
            &["code", "kind", "stderr_codepoints"],
            "exit result",
        ),
        _ => Err("unknown port result kind".to_owned()),
    }
}

fn object<'a>(
    value: &'a serde_json::Value,
    context: &str,
) -> Result<&'a serde_json::Map<String, serde_json::Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} is not an object"))
}

fn array<'a>(
    value: &'a serde_json::Value,
    context: &str,
) -> Result<&'a [serde_json::Value], String> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{context} is not an array"))
}

fn exact_keys(
    value: &serde_json::Map<String, serde_json::Value>,
    expected: &[&str],
    context: &str,
) -> Result<(), String> {
    let actual = value.keys().map(String::as_str).collect::<Vec<_>>();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    if actual != expected {
        return Err(format!("{context} keys differ"));
    }
    Ok(())
}
pub fn parse_health_logs_fixture(input: &str) -> serde_json::Result<HealthLogsFixture> {
    serde_json::from_str(input)
}

#[derive(Debug, Deserialize, Serialize)]
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
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ResultCase {
    #[serde(rename = "value")]
    Value {
        value: String,
    },
    ValueError,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeCase {
    pub id: String,
    pub recipe: ScalarRecipe,
    pub result: ResultCase,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ScalarRecipe {
    #[serde(rename = "literal")]
    Literal { text: String },
    #[serde(rename = "codepoints")]
    Codepoints { values: Vec<u32> },
    #[serde(rename = "repeat")]
    Repeat {
        codepoint: u32,
        count: u32,
        leading: Vec<u32>,
        separator: String,
        sign: String,
        trailing: Vec<u32>,
    },
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortCase {
    pub id: String,
    pub argv: PortArgv,
    pub result: PortResult,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum PortArgv {
    #[serde(rename = "text")]
    Text { values: Vec<String> },
    #[serde(rename = "codepoints")]
    Codepoints {
        prefix: Vec<String>,
        values: Vec<u32>,
    },
    #[serde(rename = "surrogateescape")]
    Surrogateescape {
        bytes_hex: String,
        prefix: Vec<String>,
    },
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum PortResult {
    #[serde(rename = "return")]
    Return { value: i64 },
    #[serde(rename = "exit")]
    Exit {
        code: i32,
        stderr_codepoints: Vec<u32>,
    },
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub capture_tool: SourceIdentity,
    pub health_fixture: SourceIdentity,
    pub service_source: SourceIdentity,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    pub path: String,
    pub sha256: String,
    pub git_blob: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextRuntime {
    pub executable_sha256: String,
    pub int_max_str_digits: u32,
    pub python: String,
    pub unicode: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnsafeUnicode {
    pub categories: BTreeMap<String, Vec<u32>>,
    pub counts: BTreeMap<String, usize>,
    pub ranges: Vec<UnsafeRange>,
}
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
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
    validate_health_text_fixture(text).expect("health text fixture semantic contract");
    let logs = health_logs_fixture();
    assert_eq!(logs.schema, 1);
    assert_eq!(logs.regex.len(), 36);
    assert_eq!(logs.unicode_contract.decimal_codepoints.len(), 760);
}

pub fn validate_health_text_fixture(fixture: &HealthTextFixture) -> Result<(), &'static str> {
    if fixture.schema != 2
        || fixture.decimal_cases.len() != 760
        || fixture.port_cases.len() != 13
        || fixture.scalar_cases.len() != 49
        || fixture.whitespace_cases.len() != 29
    {
        return Err("denominator");
    }
    validate_provenance(fixture)?;
    validate_recipes(fixture)?;
    validate_unicode(fixture)?;
    Ok(())
}

fn validate_provenance(fixture: &HealthTextFixture) -> Result<(), &'static str> {
    let provenance = &fixture.provenance;
    if provenance.capture_tool.path != "scripts/capture_health_text_reference.py"
        || provenance.capture_tool.sha256
            != "4f072c1e2f9a9c4d55dd2d42f031694340debfdd77d1402d7984e3942e74ecf8"
        || provenance.capture_tool.git_blob.as_deref()
            != Some("dc5415cdb99af22f3900b48c21eac1383f5bb4a5")
        || provenance.health_fixture.path != "core/fixtures/health_logs_reference.json"
        || provenance.health_fixture.sha256 != HEALTH_LOGS_SHA256
        || provenance.health_fixture.git_blob.is_some()
        || provenance.service_source.path != "solstone/think/service.py"
        || provenance.service_source.sha256
            != "62c31b78f97a2c147bf5873c1d732a61949c98ad54388038f313cfe23dfa8ae2"
        || provenance.service_source.git_blob.as_deref()
            != Some("baa4f68d18830e92aa6ae215ffbf86cc8e14513f")
    {
        return Err("provenance");
    }
    let runtime = &fixture.runtime;
    if runtime.executable_sha256
        != "255e900f44ce87c630e83b637a79435f9ae7778dd72f6e2a2f18a486e501d016"
        || runtime.int_max_str_digits != 4300
        || runtime.python != "3.14.6 (main, Jun 23 2026, 15:18:23) [Clang 22.1.3 ]"
        || runtime.unicode != "16.0.0"
    {
        return Err("runtime");
    }
    Ok(())
}

fn validate_recipes(fixture: &HealthTextFixture) -> Result<(), &'static str> {
    let mut scalar_ids = std::collections::BTreeSet::new();
    for case in &fixture.scalar_cases {
        if !scalar_ids.insert(&case.id) {
            return Err("scalar-id");
        }
        match &case.recipe {
            ScalarRecipe::Literal { .. } => {}
            ScalarRecipe::Codepoints { values } => {
                if values.is_empty() || values.iter().any(|value| *value > 0x10ffff) {
                    return Err("scalar-codepoints");
                }
            }
            ScalarRecipe::Repeat {
                codepoint,
                leading,
                separator: _,
                sign,
                trailing,
                ..
            } => {
                if !matches!(sign.as_str(), "" | "+" | "-")
                    || [*codepoint]
                        .into_iter()
                        .chain(leading.iter().copied())
                        .chain(trailing.iter().copied())
                        .any(|value| char::from_u32(value).is_none())
                {
                    return Err("scalar-repeat");
                }
            }
        }
        if let ResultCase::Value { value } = &case.result
            && !canonical_integer(value)
        {
            return Err("scalar-result");
        }
    }
    let mut port_ids = std::collections::BTreeSet::new();
    for case in &fixture.port_cases {
        if !port_ids.insert(&case.id) {
            return Err("port-id");
        }
        match &case.argv {
            PortArgv::Text { .. } => {}
            PortArgv::Codepoints { values, .. } => {
                if values.is_empty() || values.iter().any(|value| *value > 0x10ffff) {
                    return Err("port-codepoints");
                }
            }
            PortArgv::Surrogateescape { bytes_hex, .. } => {
                if bytes_hex.is_empty()
                    || !bytes_hex.len().is_multiple_of(2)
                    || !bytes_hex.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err("port-surrogateescape");
                }
            }
        }
        if let PortResult::Exit {
            stderr_codepoints, ..
        } = &case.result
            && stderr_codepoints.iter().any(|value| *value > 0x10ffff)
        {
            return Err("port-result");
        }
    }
    for (codepoint, _, single, mixed) in &fixture.decimal_cases {
        let scalar = char::from_u32(*codepoint).ok_or("decimal-scalar")?;
        if get_general_category(scalar) != GeneralCategory::DecimalNumber {
            return Err("decimal-category");
        }
        for result in [single, mixed] {
            if let ResultCase::Value { value } = result
                && !canonical_integer(value)
            {
                return Err("decimal-result");
            }
        }
    }
    for (codepoint, result) in &fixture.whitespace_cases {
        char::from_u32(*codepoint).ok_or("whitespace-scalar")?;
        if let ResultCase::Value { value } = result
            && !canonical_integer(value)
        {
            return Err("whitespace-result");
        }
    }
    Ok(())
}

fn canonical_integer(value: &str) -> bool {
    if value == "0" {
        return true;
    }
    let digits = value.strip_prefix('-').unwrap_or(value);
    !digits.is_empty()
        && !digits.starts_with('0')
        && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_unicode(fixture: &HealthTextFixture) -> Result<(), &'static str> {
    let expected_categories = BTreeMap::from([
        ("Cc", (65, GeneralCategory::Control)),
        ("Cf", (170, GeneralCategory::Format)),
        ("Zl", (1, GeneralCategory::LineSeparator)),
        ("Zp", (1, GeneralCategory::ParagraphSeparator)),
    ]);
    if fixture.unsafe_unicode.categories.len() != expected_categories.len() {
        return Err("unicode-categories");
    }
    let mut union = Vec::new();
    for (name, (count, category)) in expected_categories {
        let values = fixture
            .unsafe_unicode
            .categories
            .get(name)
            .ok_or("unicode-category")?;
        if values.len() != count
            || values.windows(2).any(|pair| pair[0] >= pair[1])
            || values.iter().any(|value| {
                char::from_u32(*value).is_none_or(|scalar| get_general_category(scalar) != category)
            })
        {
            return Err("unicode-category");
        }
        union.extend(values);
    }
    union.sort_unstable();
    if union.len() != 237 || union.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("unicode-union");
    }
    let expected_counts = BTreeMap::from([
        ("Cc".to_owned(), 65),
        ("Cf".to_owned(), 170),
        ("Zl".to_owned(), 1),
        ("Zp".to_owned(), 1),
        ("ranges".to_owned(), 23),
        ("union".to_owned(), 237),
    ]);
    if fixture.unsafe_unicode.counts != expected_counts {
        return Err("unicode-count");
    }
    let mut derived_ranges = Vec::new();
    let mut start = union[0];
    let mut end = start;
    for value in union.iter().copied().skip(1) {
        if value == end + 1 {
            end = value;
        } else {
            derived_ranges.push(range(start, end));
            start = value;
            end = value;
        }
    }
    derived_ranges.push(range(start, end));
    if fixture.unsafe_unicode.ranges != derived_ranges {
        return Err("unicode-range");
    }
    Ok(())
}

fn range(start: u32, end: u32) -> UnsafeRange {
    UnsafeRange {
        start,
        end,
        lower: start.checked_sub(1),
        upper: (end < 0x10ffff).then_some(end + 1),
    }
}
