// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use base64::Engine;
use serde_json::Value;
use solstone_core_generate::{GenerateRequest, contract, decode_one_shot_request};

pub fn parse_one_shot_request(input: &str) -> Result<GenerateRequest, String> {
    let raw: Value = serde_json::from_str(input).map_err(|error| error.to_string())?;
    validate_contents(&raw)?;
    decode_one_shot_request(input)
}

fn validate_contents(request: &Value) -> Result<(), String> {
    let contents = request
        .as_object()
        .and_then(|request| request.get("contents"))
        .ok_or_else(|| "contents must be a non-empty array".to_owned())?;
    let contents = contents
        .as_array()
        .filter(|contents| !contents.is_empty())
        .ok_or_else(|| "contents must be a non-empty array".to_owned())?;
    for (index, part) in contents.iter().enumerate() {
        let part = part
            .as_object()
            .ok_or_else(|| format!("contents[{index}] must be an object"))?;
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                require_fields(part, fixture_fields("text"), index, "text part")?;
                if !part.get("text").is_some_and(Value::is_string) {
                    return Err(format!("contents[{index}] text part is invalid"));
                }
            }
            Some("image") => {
                require_fields(part, fixture_fields("image"), index, "image part")?;
                let data = part.get("data").and_then(Value::as_str);
                let mime_type = part.get("mime_type").and_then(Value::as_str);
                let (Some(data), Some(mime_type)) = (data, mime_type) else {
                    return Err(format!("contents[{index}] image part has the wrong type"));
                };
                if !fixture_mime_types().contains(&mime_type) {
                    return Err(format!(
                        "contents[{index}] has an unsupported image MIME type"
                    ));
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| format!("contents[{index}] image is invalid"))?;
                image::load_from_memory(&bytes)
                    .map_err(|_| format!("contents[{index}] image is invalid"))?;
            }
            _ => return Err(format!("contents[{index}] has an unknown type")),
        }
    }
    Ok(())
}

fn fixture_fields(kind: &str) -> Vec<&str> {
    contract()["request"]["content_parts"][kind]["fields"]
        .as_array()
        .expect("generate contract content-part fields are an array")
        .iter()
        .map(|value| value.as_str().expect("generate contract field is a string"))
        .collect()
}

fn fixture_mime_types() -> Vec<&'static str> {
    contract()["request"]["content_parts"]["image"]["mime_types"]
        .as_array()
        .expect("generate contract image MIME types are an array")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("generate contract MIME type is a string")
        })
        .collect()
}

fn require_fields(
    part: &serde_json::Map<String, Value>,
    expected: Vec<&str>,
    index: usize,
    label: &str,
) -> Result<(), String> {
    if part.len() != expected.len() || part.keys().any(|field| !expected.contains(&field.as_str()))
    {
        return Err(format!("contents[{index}] {label} has unknown fields"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request(contents: Value) -> Value {
        json!({
            "schema": contract()["schema_identifiers"]["request"],
            "context": "test.generate",
            "contents": contents,
        })
    }

    #[test]
    fn accepts_valid_text_request() {
        let value = request(json!([{"type": "text", "text": "OK"}]));
        assert_eq!(
            parse_one_shot_request(&value.to_string()).unwrap().context,
            "test.generate"
        );
    }

    #[test]
    fn rejects_invalid_content_parts() {
        for contents in [
            json!([{"type": "text", "text": "OK", "extra": true}]),
            json!([{"type": "image", "mime_type": "image/png", "data": "AA==", "extra": true}]),
            json!([{"type": "audio", "data": "AA=="}]),
            json!([{"type": "image", "mime_type": "image/bmp", "data": "AA=="}]),
            json!([{"type": "image", "mime_type": "image/png", "data": "bm90IGFuIGltYWdl"}]),
            json!([{"type": "image", "mime_type": "image/png", "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4AWP4z8AAAAMBAQ=="}]),
        ] {
            assert!(parse_one_shot_request(&request(contents).to_string()).is_err());
        }
    }

    #[test]
    fn decoder_errors_remain_visible() {
        let mut wrong_schema = request(json!([{"type": "text", "text": "OK"}]));
        wrong_schema["schema"] = json!("wrong");
        assert!(parse_one_shot_request(&wrong_schema.to_string()).is_err());

        let mut unknown = request(json!([{"type": "text", "text": "OK"}]));
        unknown["unexpected"] = json!(true);
        assert!(
            parse_one_shot_request(&unknown.to_string())
                .unwrap_err()
                .contains("unexpected")
        );
    }
}
