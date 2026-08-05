use serde_json::{Number, Value};

use super::error::EntityStoreError;
use super::history::HistoryEvent;
use super::identity::IdentitySnapshot;

/// Pure outcome for one prepared identity event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparedHistoryOutcome {
    Publish,
    Discard,
    RepairRequired,
}

/// Classify one prepared event without acquiring locks or changing durable state.
pub fn classify_prepared_history(
    entity_dir: &str,
    event: &HistoryEvent,
    current_identity: Option<&IdentitySnapshot>,
) -> Result<PreparedHistoryOutcome, EntityStoreError> {
    let event_entity_id = event.value().get("entity_id");
    if event_entity_id.and_then(Value::as_str) != Some(entity_dir) {
        return Err(EntityStoreError::PreparedEntityIdMismatch {
            entity_id: entity_dir.to_owned(),
            event_entity_id: event_entity_id.cloned(),
        });
    }

    let current = current_identity.map(IdentitySnapshot::value);
    if python_optional_json_equal(current, event.value().get("identity_after")) {
        Ok(PreparedHistoryOutcome::Publish)
    } else if python_optional_json_equal(current, event.value().get("identity_before")) {
        Ok(PreparedHistoryOutcome::Discard)
    } else {
        Ok(PreparedHistoryOutcome::RepairRequired)
    }
}

fn python_optional_json_equal(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (None, None) | (None, Some(Value::Null)) | (Some(Value::Null), None) => true,
        (Some(left), Some(right)) => python_json_equal(left, right),
        _ => false,
    }
}

/// Compare JSON values with Python's structural numeric equality.
///
/// `serde_json::Value::eq` keeps integer and float Number variants distinct,
/// while Python considers equal numeric values such as `1` and `1.0` equal.
pub(crate) fn python_json_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::String(left), Value::String(right)) => left == right,
        (Value::Number(left), Value::Number(right)) => python_number_equal(left, right),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| python_json_equal(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .get(key)
                        .is_some_and(|right| python_json_equal(left, right))
                })
        }
        _ => false,
    }
}

fn python_number_equal(left: &Number, right: &Number) -> bool {
    if let (Some(left), Some(right)) = (integer_value(left), integer_value(right)) {
        return left == right;
    }
    match (
        integer_value(left),
        integer_value(right),
        left.as_f64(),
        right.as_f64(),
    ) {
        (Some(integer), None, _, Some(float)) | (None, Some(integer), Some(float), _) => {
            float_to_integer(float).is_some_and(|float_integer| integer == float_integer)
        }
        (None, None, Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn integer_value(number: &Number) -> Option<i128> {
    number
        .as_i64()
        .map(i128::from)
        .or_else(|| number.as_u64().map(i128::from))
}

fn float_to_integer(value: f64) -> Option<i128> {
    if !value.is_finite() || value.fract() != 0.0 {
        return None;
    }
    if value >= i64::MIN as f64 && value <= i64::MAX as f64 {
        return Some(i128::from(value as i64));
    }
    if value >= 0.0 && value <= u64::MAX as f64 {
        return Some(i128::from(value as u64));
    }
    None
}
