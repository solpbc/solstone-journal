// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest};
use solstone_core_local::{
    GenerateInput, GenerateResult, LoopbackAddr, Platform, generate, local_generate_input_schema,
};

pub const LOCAL_MODEL_ID: &str = "local/qwen3.5-4b";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundledError {
    UnsupportedPlatform,
    ValueOutOfRange,
}

pub fn bundled_generate(
    request: &GenerateRequest,
    journal_path: &Path,
) -> Result<GenerateResult, BundledError> {
    Ok(generate(bundled_input(request, journal_path)?))
}

pub fn bundled_input(
    request: &GenerateRequest,
    journal_path: &Path,
) -> Result<GenerateInput, BundledError> {
    let platform = match std::env::consts::OS {
        "linux" => Platform::Linux,
        "macos" => Platform::Darwin,
        _ => return Err(BundledError::UnsupportedPlatform),
    };
    Ok(GenerateInput {
        schema: local_generate_input_schema().to_owned(),
        journal_path: journal_path.display().to_string(),
        bind_address: LoopbackAddr::IPV4_LOOPBACK,
        default_model_id: LOCAL_MODEL_ID.to_owned(),
        platform,
        contents: Value::Array(request.contents.iter().map(content_value).collect()),
        system_instruction: request.system_instruction.clone(),
        temperature: request.temperature,
        max_output_tokens: u32::try_from(request.max_output_tokens)
            .map_err(|_| BundledError::ValueOutOfRange)?,
        json_output: request.json_output,
        json_schema: request.json_schema.clone(),
        timeout_s: request.timeout_s,
        exclusive_admission: request.exclusive_admission,
        attempt_index: u32::try_from(request.attempt_index)
            .map_err(|_| BundledError::ValueOutOfRange)?,
    })
}

fn content_value(content: &ContentPart) -> Value {
    match content {
        ContentPart::Text { text } => Value::String(text.clone()),
        ContentPart::Image { mime_type, data } => {
            json!({"type": "image", "mime_type": mime_type, "data": data})
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use solstone_core_generate::ContentPart;

    use super::*;

    fn request() -> GenerateRequest {
        GenerateRequest {
            id: None,
            context: "test.generate".into(),
            contents: vec![
                ContentPart::Text {
                    text: "look".into(),
                },
                ContentPart::Image {
                    mime_type: "image/png".into(),
                    data: "data".into(),
                },
            ],
            system_instruction: Some("system".into()),
            temperature: 0.2,
            max_output_tokens: 512,
            thinking_budget: None,
            timeout_s: Some(5.0),
            json_output: true,
            json_schema: Some(json!({"type": "object"})),
            enforce_responsiveness: true,
            attempt_index: 2,
            exclusive_admission: true,
            transport_retries: None,
        }
    }

    #[test]
    fn builds_the_local_generate_input() {
        let input = bundled_input(&request(), Path::new("/journal")).unwrap();
        assert_eq!(input.schema, local_generate_input_schema());
        assert_eq!(input.journal_path, "/journal");
        assert_eq!(input.default_model_id, LOCAL_MODEL_ID);
        assert_eq!(
            input.contents,
            json!(["look", {"type": "image", "mime_type": "image/png", "data": "data"}])
        );
        assert_eq!(input.attempt_index, 2);
        assert!(input.exclusive_admission);
    }

    #[test]
    fn rejects_values_outside_the_local_input_range() {
        let mut request = request();
        request.max_output_tokens = u64::from(u32::MAX) + 1;
        assert_eq!(
            bundled_input(&request, Path::new("/journal")),
            Err(BundledError::ValueOutOfRange)
        );
    }
}
