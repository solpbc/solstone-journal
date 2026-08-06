// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use crate::fixture::{request_allows_field, request_default, schema};
use crate::types::{
    ContentPart, GenerateRequest, GenerateResponse, GeneratedResponse, ProtocolError,
    ReasonCodeValue, RefusalReason, RefusedResponse, SessionTerminal,
};

fn object(value: Value) -> Result<Map<String, Value>, String> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "record must be a JSON object".to_owned())
}

fn string(object: &Map<String, Value>, name: &str) -> Result<String, String> {
    object
        .get(name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{name} must be a string"))
}

fn optional_string(object: &Map<String, Value>, name: &str) -> Result<Option<String>, String> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(format!("{name} must be a string or null")),
    }
}

fn optional_value(object: &Map<String, Value>, name: &str) -> Option<Value> {
    object.get(name).filter(|value| !value.is_null()).cloned()
}

fn optional_string_array(object: &Map<String, Value>, name: &str) -> Result<Vec<String>, String> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| format!("{name} must contain only strings"))
            })
            .collect(),
        _ => Err(format!("{name} must be an array or null")),
    }
}

fn value_or_default<'a>(object: &'a Map<String, Value>, name: &str) -> &'a Value {
    object
        .get(name)
        .filter(|value| !value.is_null())
        .unwrap_or_else(|| request_default(name))
}

fn optional_string_value(value: &Value, name: &str) -> Result<Option<String>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(value.clone())),
        _ => Err(format!("{name} must be a string or null")),
    }
}

fn optional_u64_value(value: &Value, name: &str) -> Result<Option<u64>, String> {
    match value {
        Value::Null => Ok(None),
        _ => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{name} must be an integer or null")),
    }
}

fn optional_f64_value(value: &Value, name: &str) -> Result<Option<f64>, String> {
    match value {
        Value::Null => Ok(None),
        _ => value
            .as_f64()
            .map(Some)
            .ok_or_else(|| format!("{name} must be a number or null")),
    }
}

fn require_schema(object: &Map<String, Value>, expected: &str) -> Result<(), String> {
    if object.get("schema").and_then(Value::as_str) == Some(expected) {
        Ok(())
    } else {
        Err("record schema is not supported".to_owned())
    }
}

pub fn encode_one_shot_request(request: &GenerateRequest) -> Result<String, String> {
    if request.contents.is_empty() {
        return Err("contents must be non-empty".to_owned());
    }
    let contents = request
        .contents
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({"type": "text", "text": text}),
            ContentPart::Image { mime_type, data } => {
                json!({"type": "image", "mime_type": mime_type, "data": data})
            }
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&json!({
        "schema": schema("request"),
        "id": request.id,
        "context": request.context,
        "contents": contents,
        "system_instruction": request.system_instruction,
        "temperature": request.temperature,
        "max_output_tokens": request.max_output_tokens,
        "thinking_budget": request.thinking_budget,
        "timeout_s": request.timeout_s,
        "json_output": request.json_output,
        "json_schema": request.json_schema,
        "enforce_responsiveness": request.enforce_responsiveness,
        "attempt_index": request.attempt_index,
        "exclusive_admission": request.exclusive_admission,
        "transport_retries": request.transport_retries,
    }))
    .map_err(|error| error.to_string())
}

pub fn decode_one_shot_request(input: &str) -> Result<GenerateRequest, String> {
    let object = object(serde_json::from_str(input).map_err(|error| error.to_string())?)?;
    if let Some(name) = object.keys().find(|name| !request_allows_field(name)) {
        return Err(format!("unknown request field: {name}"));
    }
    require_schema(&object, schema("request"))?;
    let contents = object
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| "contents must be an array".to_owned())?
        .iter()
        .map(|value| {
            let part = value
                .as_object()
                .ok_or_else(|| "content must be an object".to_owned())?;
            match part.get("type").and_then(Value::as_str) {
                Some("text") => Ok(ContentPart::Text {
                    text: string(part, "text")?,
                }),
                Some("image") => Ok(ContentPart::Image {
                    mime_type: string(part, "mime_type")?,
                    data: string(part, "data")?,
                }),
                _ => Err("content type is not supported".to_owned()),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    if contents.is_empty() {
        return Err("contents must be non-empty".to_owned());
    }
    Ok(GenerateRequest {
        id: optional_string(&object, "id")?,
        context: string(&object, "context")?,
        contents,
        system_instruction: optional_string_value(
            value_or_default(&object, "system_instruction"),
            "system_instruction",
        )?,
        temperature: value_or_default(&object, "temperature")
            .as_f64()
            .ok_or_else(|| "temperature must be a number".to_owned())?,
        max_output_tokens: value_or_default(&object, "max_output_tokens")
            .as_u64()
            .ok_or_else(|| "max_output_tokens must be an integer".to_owned())?,
        thinking_budget: optional_u64_value(
            value_or_default(&object, "thinking_budget"),
            "thinking_budget",
        )?,
        timeout_s: optional_f64_value(value_or_default(&object, "timeout_s"), "timeout_s")?,
        json_output: value_or_default(&object, "json_output")
            .as_bool()
            .ok_or_else(|| "json_output must be a boolean".to_owned())?,
        json_schema: match value_or_default(&object, "json_schema") {
            Value::Null => None,
            value if value.is_object() => Some(value.clone()),
            _ => return Err("json_schema must be an object or null".to_owned()),
        },
        enforce_responsiveness: value_or_default(&object, "enforce_responsiveness")
            .as_bool()
            .ok_or_else(|| "enforce_responsiveness must be a boolean".to_owned())?,
        attempt_index: value_or_default(&object, "attempt_index")
            .as_u64()
            .ok_or_else(|| "attempt_index must be an integer".to_owned())?,
        exclusive_admission: value_or_default(&object, "exclusive_admission")
            .as_bool()
            .ok_or_else(|| "exclusive_admission must be a boolean".to_owned())?,
        transport_retries: optional_u64_value(
            value_or_default(&object, "transport_retries"),
            "transport_retries",
        )?,
    })
}

pub fn decode_one_shot_response(input: &str) -> Result<GenerateResponse, String> {
    let object = object(serde_json::from_str(input).map_err(|error| error.to_string())?)?;
    require_schema(&object, schema("response"))?;
    match string(&object, "outcome")?.as_str() {
        "generated" => Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: optional_string(&object, "id")?,
            text: string(&object, "text")?,
            model: string(&object, "model")?,
            usage: object
                .get("usage")
                .cloned()
                .ok_or_else(|| "usage is required".to_owned())?,
            finish_reason: string(&object, "finish_reason")?,
            thinking: optional_value(&object, "thinking"),
            schema_validation: optional_value(&object, "schema_validation"),
            input_budget: optional_value(&object, "input_budget"),
            request_budget: optional_value(&object, "request_budget"),
            inference: optional_value(&object, "inference"),
            hints_applied: optional_string_array(&object, "hints_applied")?,
        }))),
        "refused" => {
            let reason_code =
                optional_string(&object, "reason_code")?.map(ReasonCodeValue::from_wire);
            let mut retryable = object
                .get("retryable")
                .and_then(Value::as_bool)
                .ok_or_else(|| "retryable must be a boolean".to_owned())?;
            let mut blocking = object
                .get("blocking")
                .and_then(Value::as_bool)
                .ok_or_else(|| "blocking must be a boolean".to_owned())?;
            if matches!(reason_code, Some(ReasonCodeValue::Unknown(_))) {
                retryable = false;
                blocking = true;
            }
            Ok(GenerateResponse::Refused(RefusedResponse {
                id: optional_string(&object, "id")?,
                reason: RefusalReason::from_wire(&string(&object, "reason")?),
                reason_code,
                retryable,
                blocking,
                reset_at_ms: object.get("reset_at_ms").and_then(Value::as_u64),
                provider: optional_string(&object, "provider")?,
                detail: string(&object, "detail")?,
            }))
        }
        _ => Err("response outcome is not supported".to_owned()),
    }
}

pub fn decode_protocol_error(input: &str) -> Result<ProtocolError, String> {
    let object = object(serde_json::from_str(input).map_err(|error| error.to_string())?)?;
    require_schema(&object, schema("error"))?;
    Ok(ProtocolError {
        id: optional_string(&object, "id")?,
        reason: string(&object, "reason")?,
        detail: string(&object, "detail")?,
    })
}

pub fn encode_session_request_line(request: &GenerateRequest) -> Result<String, String> {
    if request.id.is_none() {
        return Err("session request id is required".to_owned());
    }
    Ok(format!("{}\n", encode_one_shot_request(request)?))
}

pub fn decode_session_request_line(line: &str) -> Result<GenerateRequest, String> {
    let request = decode_one_shot_request(line.trim_end())?;
    if request.id.is_none() {
        return Err("session request id is required".to_owned());
    }
    Ok(request)
}

pub fn decode_session_response_line(line: &str) -> Result<GenerateResponse, String> {
    let response = decode_one_shot_response(line.trim_end())?;
    let id = match &response {
        GenerateResponse::Generated(value) => &value.id,
        GenerateResponse::Refused(value) => &value.id,
    };
    if id.is_none() {
        return Err("session response id is required".to_owned());
    }
    Ok(response)
}

pub fn encode_session_terminal_line(_: SessionTerminal) -> Result<String, String> {
    serde_json::to_string(&json!({"schema": schema("session_terminal")}))
        .map(|line| format!("{line}\n"))
        .map_err(|error| error.to_string())
}

pub fn decode_session_terminal_line(line: &str) -> Result<SessionTerminal, String> {
    let terminal =
        object(serde_json::from_str(line.trim_end()).map_err(|error| error.to_string())?)?;
    if terminal.len() != 1 || !terminal.contains_key("schema") {
        return Err("terminal record has unknown fields".to_owned());
    }
    require_schema(&terminal, schema("session_terminal"))?;
    Ok(SessionTerminal)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    MissingId,
    DuplicateId(String),
    UnknownOrRetiredId(String),
    Terminal,
}

#[derive(Debug, Default)]
pub struct SessionCorrelation {
    outstanding: HashSet<String>,
    retired: HashSet<String>,
    terminal: bool,
}

impl SessionCorrelation {
    pub fn submit(&mut self, id: impl Into<String>) -> Result<(), SessionError> {
        if self.terminal {
            return Err(SessionError::Terminal);
        }
        let id = id.into();
        if id.is_empty() {
            return Err(SessionError::MissingId);
        }
        if self.retired.contains(&id) {
            return Err(SessionError::UnknownOrRetiredId(id));
        }
        if !self.outstanding.insert(id.clone()) {
            return Err(SessionError::DuplicateId(id));
        }
        Ok(())
    }

    pub fn accept(&mut self, response: &GenerateResponse) -> Result<(), SessionError> {
        if self.terminal {
            return Err(SessionError::Terminal);
        }
        let id = match response {
            GenerateResponse::Generated(value) => value.id.as_deref(),
            GenerateResponse::Refused(value) => value.id.as_deref(),
        }
        .ok_or(SessionError::MissingId)?;
        if !self.outstanding.remove(id) || self.retired.contains(id) {
            self.terminal = true;
            return Err(SessionError::UnknownOrRetiredId(id.to_owned()));
        }
        self.retired.insert(id.to_owned());
        Ok(())
    }

    pub(crate) fn fail_outstanding(&mut self) -> Vec<String> {
        self.terminal = true;
        let ids = self.outstanding.drain().collect::<Vec<_>>();
        self.retired.extend(ids.iter().cloned());
        ids
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::contract;
    use crate::types::{Outcome, ReasonCodeValue};

    fn request(id: Option<&str>) -> GenerateRequest {
        GenerateRequest {
            id: id.map(ToOwned::to_owned),
            context: "test.generate".to_owned(),
            contents: vec![ContentPart::Text {
                text: "OK".to_owned(),
            }],
            system_instruction: None,
            temperature: 0.3,
            max_output_tokens: 16,
            thinking_budget: None,
            timeout_s: None,
            json_output: false,
            json_schema: None,
            enforce_responsiveness: true,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        }
    }

    #[test]
    fn fixture_conformance_vectors_decode() {
        for vector in contract()["conformance_vectors"].as_array().unwrap() {
            match vector["framing"].as_str().unwrap() {
                "one_shot" => {
                    if let Some(response) = vector.get("response") {
                        decode_one_shot_response(&response.to_string()).unwrap();
                    }
                    if let Some(request) = vector.get("request") {
                        decode_one_shot_request(&request.to_string()).unwrap();
                    }
                }
                "protocol_error" => {
                    decode_protocol_error(&vector["protocol_error"].to_string()).unwrap();
                }
                framing => panic!("unexpected fixture framing {framing}"),
            }
        }
    }

    #[test]
    fn request_decoder_applies_fixture_defaults() {
        let request = decode_one_shot_request(
            &json!({
                "schema": schema("request"),
                "context": "test.generate",
                "contents": [{"type": "text", "text": "OK"}],
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(request.id, None);
        assert_eq!(request.system_instruction, None);
        assert_eq!(request.temperature, 0.3);
        assert_eq!(request.max_output_tokens, 16_384);
        assert_eq!(request.thinking_budget, None);
        assert_eq!(request.timeout_s, None);
        assert!(!request.json_output);
        assert_eq!(request.json_schema, None);
        assert!(request.enforce_responsiveness);
        assert_eq!(request.attempt_index, 0);
        assert!(!request.exclusive_admission);
        assert_eq!(request.transport_retries, None);
    }

    #[test]
    fn request_decoder_rejects_unknown_fields() {
        assert!(
            decode_one_shot_request(
                &json!({
                    "schema": schema("request"),
                    "context": "test.generate",
                    "contents": [{"type": "text", "text": "OK"}],
                    "unknown": true,
                })
                .to_string(),
            )
            .is_err()
        );
    }

    #[test]
    fn enum_members_match_fixture() {
        let outcomes = [Outcome::Generated, Outcome::Refused]
            .into_iter()
            .map(Outcome::as_str)
            .collect::<Vec<_>>();
        assert_eq!(
            outcomes,
            contract()["outcomes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>()
        );
        let reasons = [
            RefusalReason::AttestationNotVerified,
            RefusalReason::AttestationFailed,
            RefusalReason::AttestationStale,
            RefusalReason::NoEngineConfigured,
            RefusalReason::IncompleteJson,
            RefusalReason::IncompleteText,
            RefusalReason::ProviderResponseInvalid,
            RefusalReason::SchemaValidationFailed,
            RefusalReason::NonResponsiveOutput,
            RefusalReason::Unknown,
        ]
        .into_iter()
        .map(RefusalReason::as_str)
        .collect::<Vec<_>>();
        assert_eq!(
            reasons,
            contract()["refusal_reasons"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn tagged_union_round_trips_legal_variants_only() {
        let generated =
            decode_one_shot_response(&contract()["conformance_vectors"][0]["response"].to_string())
                .unwrap();
        let refused =
            decode_one_shot_response(&contract()["conformance_vectors"][1]["response"].to_string())
                .unwrap();
        assert!(matches!(generated, GenerateResponse::Generated(_)));
        assert!(matches!(refused, GenerateResponse::Refused(_)));
    }

    #[test]
    fn refused_round_trip_preserves_classification_and_metadata() {
        let vector = &contract()["conformance_vectors"][1]["response"];
        let response = decode_one_shot_response(&vector.to_string()).unwrap();
        let GenerateResponse::Refused(value) = response else {
            panic!("expected refusal")
        };
        assert_eq!(value.reason, RefusalReason::AttestationNotVerified);
        assert_eq!(
            value.reason_code.unwrap().as_wire(),
            "attestation_not_yet_verified"
        );
        assert!(!value.retryable || value.blocking);
        assert_eq!(value.reset_at_ms, None);
        assert_eq!(value.provider.as_deref(), Some("local"));
    }

    #[test]
    fn generated_round_trip_preserves_all_result_fields() {
        let response =
            decode_one_shot_response(&contract()["conformance_vectors"][0]["response"].to_string())
                .unwrap();
        let GenerateResponse::Generated(value) = response else {
            panic!("expected generated")
        };
        assert_eq!(value.text, "OK");
        assert_eq!(value.model, "fixture-model");
        assert_eq!(
            value.usage,
            json!({"input_tokens": 2, "output_tokens": 1, "total_tokens": 3})
        );
        assert_eq!(value.finish_reason, "stop");
        assert_eq!(value.thinking, None);
        assert_eq!(value.schema_validation, None);
        assert_eq!(value.input_budget, None);
        assert_eq!(value.request_budget, None);
        assert_eq!(value.inference, None);
        assert!(value.hints_applied.is_empty());
    }

    #[test]
    fn generated_response_decodes_hints_applied() {
        let mut value = contract()["conformance_vectors"][0]["response"].clone();
        value["hints_applied"] = json!(["attempt_index", "exclusive_admission"]);
        let response = decode_one_shot_response(&value.to_string()).unwrap();
        let GenerateResponse::Generated(value) = response else {
            panic!("expected generated")
        };
        assert_eq!(
            value.hints_applied,
            ["attempt_index", "exclusive_admission"]
        );
    }

    #[test]
    fn unknown_reason_code_is_safe() {
        let vector = &contract()["conformance_vectors"][12]["response"];
        let response = decode_one_shot_response(&vector.to_string()).unwrap();
        let GenerateResponse::Refused(value) = response else {
            panic!("expected refusal")
        };
        assert!(matches!(
            value.reason_code,
            Some(ReasonCodeValue::Unknown(_))
        ));
        assert!(!value.retryable);
        assert!(value.blocking);
    }

    #[test]
    fn wrong_schema_is_rejected() {
        let mut value = contract()["conformance_vectors"][0]["response"].clone();
        value["schema"] = json!("wrong");
        assert!(decode_one_shot_response(&value.to_string()).is_err());
    }

    #[test]
    fn session_codec_correlates_out_of_order_ids_and_rejects_missing_ids() {
        let first = request(Some("first"));
        let second = request(Some("second"));
        assert!(encode_session_request_line(&first).unwrap().ends_with('\n'));
        assert!(
            decode_session_request_line(&encode_session_request_line(&second).unwrap()).is_ok()
        );
        assert!(encode_session_request_line(&request(None)).is_err());
        let mut tracker = SessionCorrelation::default();
        tracker.submit("first").unwrap();
        tracker.submit("second").unwrap();
        let first_response = GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: Some("first".to_owned()),
            text: "one".to_owned(),
            model: "m".to_owned(),
            usage: json!({}),
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        }));
        let second_response = GenerateResponse::Refused(RefusedResponse {
            id: Some("second".to_owned()),
            reason: RefusalReason::NoEngineConfigured,
            reason_code: None,
            retryable: false,
            blocking: true,
            reset_at_ms: None,
            provider: Some("none".to_owned()),
            detail: "none".to_owned(),
        });
        tracker.accept(&second_response).unwrap();
        tracker.accept(&first_response).unwrap();
        assert_eq!(
            tracker.accept(&first_response),
            Err(SessionError::UnknownOrRetiredId("first".to_owned()))
        );
    }

    #[test]
    fn session_terminal_codec_rejects_request_decoding() {
        let line = encode_session_terminal_line(SessionTerminal).unwrap();
        assert!(line.ends_with('\n'));
        assert_eq!(decode_session_terminal_line(&line), Ok(SessionTerminal));
        assert!(decode_session_request_line(&line).is_err());
    }

    #[test]
    fn session_correlation_rejects_duplicate_and_retired_submissions() {
        let mut tracker = SessionCorrelation::default();
        tracker.submit("first").unwrap();
        assert_eq!(
            tracker.submit("first"),
            Err(SessionError::DuplicateId("first".to_owned()))
        );
        let response = GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: Some("first".to_owned()),
            text: "one".to_owned(),
            model: "m".to_owned(),
            usage: json!({}),
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        }));
        tracker.accept(&response).unwrap();
        assert_eq!(
            tracker.submit("first"),
            Err(SessionError::UnknownOrRetiredId("first".to_owned()))
        );
    }
}
