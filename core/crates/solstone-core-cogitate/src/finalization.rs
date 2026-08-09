// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// A configuration value retaining the distinction between boolean `true` and
/// other truthy values from the Python configuration contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinalizationValue<'a> {
    Null,
    Boolean(bool),
    String(&'a str),
    Truthy,
    Falsey,
}

impl FinalizationValue<'_> {
    fn is_truthy(self) -> bool {
        match self {
            Self::Boolean(true) | Self::Truthy => true,
            Self::String(value) => !value.is_empty(),
            Self::Null | Self::Boolean(false) | Self::Falsey => false,
        }
    }
}

/// The fields consulted when selecting a cogitate finalization tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FinalizationConfig<'a> {
    pub diagnostic: Option<FinalizationValue<'a>>,
    pub output_path: Option<FinalizationValue<'a>>,
    pub schedule: Option<&'a str>,
}

/// Select `emit_final` versus the built-in finish tool.
pub fn expects_emit_final(config: FinalizationConfig<'_>) -> bool {
    matches!(config.diagnostic, Some(FinalizationValue::Boolean(true)))
        || config.output_path.is_some_and(FinalizationValue::is_truthy)
        || matches!(config.schedule, Some("daily" | "weekly" | "activity"))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::oracle;

    fn finalization_value(value: Option<&Value>) -> Option<FinalizationValue<'_>> {
        value.map(|value| match value {
            Value::Null => FinalizationValue::Null,
            Value::Bool(value) => FinalizationValue::Boolean(*value),
            Value::String(value) => FinalizationValue::String(value),
            Value::Number(value) if value.as_i64() == Some(0) || value.as_u64() == Some(0) => {
                FinalizationValue::Falsey
            }
            Value::Number(_) => FinalizationValue::Truthy,
            Value::Array(value) if value.is_empty() => FinalizationValue::Falsey,
            Value::Array(_) => FinalizationValue::Truthy,
            Value::Object(value) if value.is_empty() => FinalizationValue::Falsey,
            Value::Object(_) => FinalizationValue::Truthy,
        })
    }

    #[test]
    fn finalization_vectors_match_the_oracle() {
        let fixture = oracle::fixture();
        assert_eq!(fixture.expects_emit_final.len(), 16);
        for vector in &fixture.expects_emit_final {
            let config = FinalizationConfig {
                diagnostic: finalization_value(vector.config.get("diagnostic")),
                output_path: finalization_value(vector.config.get("output_path")),
                schedule: vector.config.get("schedule").and_then(Value::as_str),
            };
            assert_eq!(expects_emit_final(config), vector.expect, "{}", vector.id);
        }
    }
}
