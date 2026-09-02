// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use hmac::{Hmac, Mac};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::fixture::local_contract;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintError(pub String);

impl fmt::Display for FingerprintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for FingerprintError {}

/// The configured active provider and model, with an optional resolved lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneResolution {
    pub lane: Option<String>,
    pub provider: String,
    pub model: Option<String>,
}

/// Inputs that Python normalizes before canonical JSON encoding.
#[derive(Debug, Clone)]
pub enum CanonicalInput {
    Json(Value),
    Tuple(Vec<CanonicalInput>),
    Path(PathBuf),
    Object(Vec<(String, CanonicalInput)>),
}

#[derive(Debug, Clone)]
enum CanonicalValue {
    Null,
    Bool(bool),
    Integer(String),
    Float(f64),
    String(String),
    Array(Vec<CanonicalValue>),
    Object(BTreeMap<String, CanonicalValue>),
}

impl CanonicalInput {
    fn normalize(&self) -> Result<CanonicalValue, FingerprintError> {
        self.normalize_with_array_order(false)
    }

    fn normalize_preserving_array_order(&self) -> Result<CanonicalValue, FingerprintError> {
        self.normalize_with_array_order(true)
    }

    fn normalize_with_array_order(
        &self,
        preserve_array_order: bool,
    ) -> Result<CanonicalValue, FingerprintError> {
        match self {
            Self::Json(value) => normalize_json(value, preserve_array_order),
            Self::Tuple(values) => normalize_sequence(values, preserve_array_order),
            Self::Path(path) => Ok(CanonicalValue::String(path.display().to_string())),
            Self::Object(values) => {
                let mut normalized = BTreeMap::new();
                for (key, value) in values {
                    normalized.insert(
                        key.to_string(),
                        value.normalize_with_array_order(preserve_array_order)?,
                    );
                }
                Ok(CanonicalValue::Object(normalized))
            }
        }
    }
}

fn normalize_json(
    value: &Value,
    preserve_array_order: bool,
) -> Result<CanonicalValue, FingerprintError> {
    match value {
        Value::Null => Ok(CanonicalValue::Null),
        Value::Bool(value) => Ok(CanonicalValue::Bool(*value)),
        Value::String(value) => Ok(CanonicalValue::String(value.clone())),
        Value::Number(number) if number.is_i64() || number.is_u64() => {
            Ok(CanonicalValue::Integer(number.to_string()))
        }
        Value::Number(number) => number
            .as_f64()
            .filter(|value| value.is_finite())
            .map(CanonicalValue::Float)
            .ok_or_else(|| FingerprintError("unsupported non-finite float".to_owned())),
        Value::Array(values) => {
            let inputs = values
                .iter()
                .cloned()
                .map(CanonicalInput::Json)
                .collect::<Vec<_>>();
            normalize_sequence(&inputs, preserve_array_order)
        }
        Value::Object(values) => {
            let mut normalized = BTreeMap::new();
            for (key, value) in values {
                normalized.insert(
                    key.to_string(),
                    normalize_json(value, preserve_array_order)?,
                );
            }
            Ok(CanonicalValue::Object(normalized))
        }
    }
}

fn normalize_sequence(
    values: &[CanonicalInput],
    preserve_array_order: bool,
) -> Result<CanonicalValue, FingerprintError> {
    let mut normalized = values
        .iter()
        .map(|value| value.normalize_with_array_order(preserve_array_order))
        .collect::<Result<Vec<_>, _>>()?;
    if !preserve_array_order {
        normalized.sort_by_cached_key(canonical_value_json);
    }
    Ok(CanonicalValue::Array(normalized))
}

/// Matches Python's recursive normalization and compact, ASCII JSON output.
pub fn canonical_json(input: &CanonicalInput) -> Result<String, FingerprintError> {
    Ok(canonical_value_json(&input.normalize()?))
}

/// The SHA-256 digest of Python-compatible canonical JSON.
pub fn canonical_fingerprint(input: &CanonicalInput) -> Result<String, FingerprintError> {
    Ok(fingerprint_sha256(&canonical_json(input)?))
}

/// Matches [`canonical_json`] but preserves JSON-array and tuple order.
pub fn canonical_json_preserving_array_order(
    input: &CanonicalInput,
) -> Result<String, FingerprintError> {
    Ok(canonical_value_json(
        &input.normalize_preserving_array_order()?,
    ))
}

/// The SHA-256 digest of canonical JSON that preserves JSON-array and tuple order.
pub fn canonical_fingerprint_preserving_array_order(
    input: &CanonicalInput,
) -> Result<String, FingerprintError> {
    Ok(fingerprint_sha256(&canonical_json_preserving_array_order(
        input,
    )?))
}

pub fn fingerprint_sha256(canonical_json: &str) -> String {
    hex_digest(Sha256::digest(canonical_json.as_bytes()))
}

pub(crate) fn hmac_sha256(key: &[u8], canonical_json: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts arbitrary-length keys");
    mac.update(canonical_json.as_bytes());
    hex_digest(mac.finalize().into_bytes())
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    let mut output = String::with_capacity(bytes.as_ref().len() * 2);
    for byte in bytes.as_ref() {
        use std::fmt::Write;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn canonical_value_json(value: &CanonicalValue) -> String {
    match value {
        CanonicalValue::Null => "null".to_owned(),
        CanonicalValue::Bool(value) => value.to_string(),
        CanonicalValue::Integer(value) => value.clone(),
        CanonicalValue::Float(value) => python_float(*value),
        CanonicalValue::String(value) => quote_ascii(value),
        CanonicalValue::Array(values) => {
            let body = values
                .iter()
                .map(canonical_value_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{body}]")
        }
        CanonicalValue::Object(values) => {
            let body = values
                .iter()
                .map(|(key, value)| format!("{}:{}", quote_ascii(key), canonical_value_json(value)))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
    }
}

fn quote_ascii(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 2);
    result.push('"');
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\u{08}' => result.push_str("\\b"),
            '\u{0c}' => result.push_str("\\f"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character <= '\u{1f}' => {
                use std::fmt::Write;
                write!(&mut result, "\\u{:04x}", character as u32).expect("String write");
            }
            character if character.is_ascii() => result.push(character),
            character => {
                for unit in character.encode_utf16(&mut [0; 2]) {
                    use std::fmt::Write;
                    write!(&mut result, "\\u{unit:04x}").expect("String write");
                }
            }
        }
    }
    result.push('"');
    result
}

/// Python's repr(float) spelling used by json.dumps: shortest-roundtrip digits,
/// a decimal point for integral finite floats, and signed two-digit exponents.
fn python_float(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_owned()
        } else {
            "0.0".to_owned()
        };
    }

    let mut buffer = ryu::Buffer::new();
    let raw = buffer.format_finite(value);
    let negative = raw.starts_with('-');
    let body = raw.strip_prefix('-').unwrap_or(raw);
    let (coefficient, exponent) = match body.split_once(['e', 'E']) {
        Some((coefficient, exponent)) => (
            coefficient,
            exponent.parse::<i32>().expect("ryu exponent is an i32"),
        ),
        None => (body, 0),
    };
    let decimal_at = coefficient
        .find('.')
        .map_or(coefficient.len(), |position| position) as i32
        + exponent;
    let digits = coefficient.replace('.', "");
    let first = digits
        .find(|character: char| character != '0')
        .expect("nonzero finite floats contain a nonzero digit");
    let significant = &digits[first..];
    let scientific_exponent = decimal_at - first as i32 - 1;
    let sign = if negative { "-" } else { "" };

    if !(-4..16).contains(&scientific_exponent) {
        let tail = &significant[1..];
        let mantissa = if tail.is_empty() {
            significant.to_owned()
        } else {
            format!("{}.{}", &significant[..1], tail)
        };
        return format!(
            "{sign}{mantissa}e{}{magnitude:02}",
            if scientific_exponent < 0 { '-' } else { '+' },
            magnitude = scientific_exponent.unsigned_abs()
        );
    }

    let decimal = decimal_at - first as i32;
    let mut fixed = if decimal <= 0 {
        format!(
            "0.{}{}",
            "0".repeat(decimal.unsigned_abs() as usize),
            significant
        )
    } else if decimal as usize >= significant.len() {
        format!(
            "{}{}",
            significant,
            "0".repeat(decimal as usize - significant.len())
        )
    } else {
        format!(
            "{}.{}",
            &significant[..decimal as usize],
            &significant[decimal as usize..]
        )
    };
    if !fixed.contains('.') {
        fixed.push_str(".0");
    }
    format!("{sign}{fixed}")
}

pub fn derive_active_brain_lane(config: &Map<String, Value>) -> LaneResolution {
    let (provider, model) = active_config(config);
    if provider == "none" {
        return LaneResolution {
            lane: Some("none".to_owned()),
            provider,
            model,
        };
    }
    if local_contract()
        .brain_state
        .cloud_byo_providers
        .iter()
        .any(|candidate| candidate == &provider)
    {
        LaneResolution {
            lane: Some("byo-cloud".to_owned()),
            provider,
            model,
        }
    } else if provider == "local" {
        let endpoint = local_endpoint(config);
        match endpoint.state.as_str() {
            "missing" => LaneResolution {
                lane: Some("bundled".to_owned()),
                provider,
                model,
            },
            "partial" => LaneResolution {
                lane: None,
                provider,
                model,
            },
            _ if spp_provenance_matches(config, &endpoint) => LaneResolution {
                lane: Some("spp".to_owned()),
                provider,
                model,
            },
            _ if confidential_block(config).is_some() => LaneResolution {
                lane: None,
                provider,
                model,
            },
            _ => LaneResolution {
                lane: Some("byo-endpoint".to_owned()),
                provider,
                model,
            },
        }
    } else {
        LaneResolution {
            lane: None,
            provider,
            model,
        }
    }
}

/// The 7-field bundled-runtime object published as `desired_fingerprint_sha256`.
#[derive(Debug, Clone, PartialEq)]
pub struct BundledRuntimeDesired {
    pub json: Value,
    pub sha256: String,
}

/// Sole constructor for the bundled-runtime desired fingerprint.
pub fn bundled_runtime_desired_fingerprint(
    backend: &str,
    model_id: &str,
    artifact_target_fingerprint_sha256: &str,
    binary_path: Option<&str>,
    model_path: &str,
    projector_path: Option<&str>,
) -> Result<BundledRuntimeDesired, FingerprintError> {
    let json = json!({
        "provider": "local",
        "backend": backend,
        "model_id": model_id,
        "artifact_target_fingerprint_sha256": artifact_target_fingerprint_sha256,
        "binary_path": binary_path,
        "model_path": model_path,
        "projector_path": projector_path,
    });
    Ok(BundledRuntimeDesired {
        sha256: canonical_fingerprint(&CanonicalInput::Json(json.clone()))?,
        json,
    })
}

pub fn build_active_brain_fingerprint(
    config: &Map<String, Value>,
    hmac_key: &[u8],
    bundled_runtime: Option<Value>,
) -> Result<Option<String>, FingerprintError> {
    let resolution = derive_active_brain_lane(config);
    let lane = resolution
        .lane
        .ok_or_else(|| FingerprintError("configuration_invalid".to_owned()))?;
    let provider = resolution.provider;
    let model = resolution.model.unwrap_or_default();
    let mut components = Map::new();
    components.insert(
        "schema_version".to_owned(),
        Value::from(local_contract().brain_state.fingerprint_schema_version),
    );
    components.insert("lane".to_owned(), Value::String(lane.clone()));
    components.insert(
        "active".to_owned(),
        Value::Object(Map::from_iter([
            ("provider".to_owned(), Value::String(provider.clone())),
            ("model".to_owned(), Value::String(model)),
        ])),
    );

    if lane == "byo-cloud" {
        components.insert(
            "cloud_credential".to_owned(),
            local_contract()
                .brain_state
                .provider_env_by_name
                .get(&provider)
                .and_then(|name| {
                    config
                        .get("env")
                        .and_then(Value::as_object)
                        .and_then(|env| env.get(name))
                })
                .and_then(Value::as_str)
                .filter(|credential| !credential.is_empty())
                .map(|credential| Value::String(hmac_sha256(hmac_key, credential)))
                .unwrap_or(Value::Null),
        );
    }
    if lane == "byo-endpoint" || lane == "spp" {
        let local = local_endpoint(config);
        let mut endpoint = Map::new();
        endpoint.insert("base_url".to_owned(), Value::String(local.base_url));
        endpoint.insert(
            "served_model_id".to_owned(),
            Value::String(local.served_model_id),
        );
        endpoint.insert(
            "credential".to_owned(),
            if local.credential.as_deref().is_none_or(str::is_empty) {
                Value::Null
            } else {
                Value::String(hmac_sha256(
                    hmac_key,
                    local.credential.as_deref().unwrap_or(""),
                ))
            },
        );
        components.insert("local_endpoint".to_owned(), Value::Object(endpoint));
    }
    if lane == "spp" {
        let confidential = confidential_block(config)
            .ok_or_else(|| FingerprintError("confidential_unavailable".to_owned()))?;
        let mut provenance = confidential.clone();
        provenance.remove("prior_active");
        provenance.remove("prior_local_endpoint");
        let wrapped = Value::Object(Map::from_iter([(
            "value".to_owned(),
            Value::Object(provenance),
        )]));
        let text = canonical_json(&CanonicalInput::Json(wrapped))?;
        components.insert(
            "confidential".to_owned(),
            Value::String(hmac_sha256(hmac_key, &text)),
        );
    }
    if lane == "bundled" {
        components.insert(
            "bundled_runtime".to_owned(),
            bundled_runtime
                .ok_or_else(|| FingerprintError("local_runtime_state_unavailable".to_owned()))?,
        );
    }
    canonical_fingerprint(&CanonicalInput::Json(Value::Object(components))).map(Some)
}

fn active_config(config: &Map<String, Value>) -> (String, Option<String>) {
    let active = config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("active"))
        .and_then(Value::as_object);
    let Some(active) = active else {
        return ("none".to_owned(), None);
    };
    let Some(provider) = active
        .get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return ("none".to_owned(), None);
    };
    if provider == "none" {
        return ("none".to_owned(), None);
    }
    let model = active
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        // Mirrors solstone/think/models.py:148-156 and :176-180.
        .unwrap_or_else(|| match provider {
            "google" => "gemini-3.5-flash".to_owned(),
            "openai" => "gpt-5.4-mini".to_owned(),
            "anthropic" => "claude-sonnet-4-6".to_owned(),
            "local" => "local/qwen3.5-4b".to_owned(),
            _ => String::new(),
        });
    (provider.to_owned(), Some(model))
}

struct LocalEndpoint {
    state: String,
    base_url: String,
    served_model_id: String,
    credential: Option<String>,
}

fn local_endpoint(config: &Map<String, Value>) -> LocalEndpoint {
    let local = config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("local"))
        .and_then(Value::as_object);
    let endpoint_url = local
        .and_then(|local| local.get("endpoint_url"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let served_model_id = local
        .and_then(|local| local.get("served_model_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let state = if endpoint_url.is_empty() && served_model_id.is_empty() {
        "missing"
    } else if endpoint_url.is_empty() || served_model_id.is_empty() {
        "partial"
    } else {
        "complete"
    };
    LocalEndpoint {
        state: state.to_owned(),
        base_url: normalize_local_endpoint(endpoint_url),
        served_model_id: served_model_id.to_owned(),
        credential: local
            .and_then(|local| local.get("credential"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

fn confidential_block(config: &Map<String, Value>) -> Option<&Map<String, Value>> {
    config
        .get("services")
        .and_then(Value::as_object)
        .and_then(|services| services.get("confidential"))
        .and_then(Value::as_object)
}

fn spp_provenance_matches(config: &Map<String, Value>, endpoint: &LocalEndpoint) -> bool {
    if endpoint.state != "complete" || endpoint.credential.is_none() {
        return false;
    }
    let Some(block) = confidential_block(config) else {
        return false;
    };
    let Some(block_url) = block.get("endpoint_url").and_then(Value::as_str) else {
        return false;
    };
    let Some(block_model) = block.get("served_model_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(block_fingerprint) = block
        .get("credential_fingerprint_sha256")
        .and_then(Value::as_str)
    else {
        return false;
    };
    normalize_local_endpoint(block_url) == endpoint.base_url
        && block_model == endpoint.served_model_id
        && block_fingerprint == sha256_text(endpoint.credential.as_deref().unwrap_or(""))
}

fn normalize_local_endpoint(value: &str) -> String {
    let trimmed = value.trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_owned()
}

fn sha256_text(value: &str) -> String {
    hex_digest(Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::{
        CanonicalInput, canonical_fingerprint, canonical_fingerprint_preserving_array_order,
        canonical_json, canonical_json_preserving_array_order,
    };
    use serde_json::json;

    #[test]
    fn formats_python_float_boundaries() {
        assert_eq!(
            canonical_json(&CanonicalInput::Json(json!(1e-7))).unwrap(),
            "1e-07"
        );
        assert_eq!(
            canonical_json(&CanonicalInput::Json(json!(1e22))).unwrap(),
            "1e+22"
        );
        assert_eq!(
            canonical_json(&CanonicalInput::Json(json!(-0.0))).unwrap(),
            "-0.0"
        );
        assert_eq!(
            canonical_json(&CanonicalInput::Json(json!(1.0))).unwrap(),
            "1.0"
        );
    }

    #[test]
    fn ordered_fingerprint_changes_when_array_order_changes() {
        let first = CanonicalInput::Json(json!(["first", "second"]));
        let second = CanonicalInput::Json(json!(["second", "first"]));

        assert_eq!(
            canonical_fingerprint(&first).unwrap(),
            canonical_fingerprint(&second).unwrap()
        );
        assert_ne!(
            canonical_fingerprint_preserving_array_order(&first).unwrap(),
            canonical_fingerprint_preserving_array_order(&second).unwrap()
        );
    }

    #[test]
    fn ordered_canonical_json_still_sorts_object_keys() {
        let first = CanonicalInput::Json(json!({"b": [2, 1], "a": "first"}));
        let second = CanonicalInput::Json(json!({"a": "first", "b": [2, 1]}));

        assert_eq!(
            canonical_json_preserving_array_order(&first).unwrap(),
            canonical_json_preserving_array_order(&second).unwrap()
        );
    }

    #[test]
    fn ordered_fingerprint_preserves_tuple_order() {
        let first = CanonicalInput::Tuple(vec![
            CanonicalInput::Json(json!("first")),
            CanonicalInput::Json(json!("second")),
        ]);
        let second = CanonicalInput::Tuple(vec![
            CanonicalInput::Json(json!("second")),
            CanonicalInput::Json(json!("first")),
        ]);

        assert_eq!(
            canonical_fingerprint(&first).unwrap(),
            canonical_fingerprint(&second).unwrap()
        );
        assert_ne!(
            canonical_fingerprint_preserving_array_order(&first).unwrap(),
            canonical_fingerprint_preserving_array_order(&second).unwrap()
        );
    }
}
