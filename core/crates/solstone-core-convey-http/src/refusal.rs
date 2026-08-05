// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Repair-required refusal payloads shared with the Python entity operations.

use serde::Serialize;
use serde_json::Value;

use crate::envelope::ErrorEnvelope;

/// A merge refusal that leaves durable state requiring manual repair.
#[derive(Debug, Serialize)]
pub struct MergeRepairRequired {
    #[serde(flatten)]
    pub envelope: ErrorEnvelope,
    pub failed_phase: String,
    pub source_id: String,
    pub target_id: String,
    pub operation_state: String,
    pub mutation_applied: bool,
    pub source_state: Value,
    pub target_state: Value,
    pub safe_remediation: String,
}

/// An undo refusal that leaves durable state requiring manual repair.
#[derive(Debug, Serialize)]
pub struct UndoRepairRequired {
    #[serde(flatten)]
    pub envelope: ErrorEnvelope,
    pub merge_id: String,
    pub source_id: String,
    pub target_id: String,
    pub operation_state: String,
    pub mutation_applied: bool,
    pub source_state: Value,
    pub target_state: Value,
    pub safe_remediation: String,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::{Value, json, to_value};

    use super::{MergeRepairRequired, UndoRepairRequired};
    use crate::envelope::ErrorEnvelope;

    fn envelope() -> ErrorEnvelope {
        ErrorEnvelope {
            error: "Conflict".to_owned(),
            reason_code: "repair_required".to_owned(),
            detail: "repair needed".to_owned(),
        }
    }

    fn merge() -> MergeRepairRequired {
        MergeRepairRequired {
            envelope: envelope(),
            failed_phase: "commit".to_owned(),
            source_id: "source".to_owned(),
            target_id: "target".to_owned(),
            operation_state: "partially_applied".to_owned(),
            mutation_applied: true,
            source_state: json!({"id": "source"}),
            target_state: json!({"id": "target"}),
            safe_remediation: "repair manually".to_owned(),
        }
    }

    fn undo() -> UndoRepairRequired {
        UndoRepairRequired {
            envelope: envelope(),
            merge_id: "merge-1".to_owned(),
            source_id: "source".to_owned(),
            target_id: "target".to_owned(),
            operation_state: "partially_undone".to_owned(),
            mutation_applied: true,
            source_state: json!({"id": "source"}),
            target_state: json!({"id": "target"}),
            safe_remediation: "repair manually".to_owned(),
        }
    }

    fn keys(value: &Value) -> BTreeSet<String> {
        value
            .as_object()
            .expect("repair refusal serializes as an object")
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn repair_refusals_flatten_exactly_eleven_top_level_keys() {
        let merge_keys = keys(&to_value(merge()).unwrap());
        let undo_keys = keys(&to_value(undo()).unwrap());

        assert_eq!(
            merge_keys,
            BTreeSet::from([
                "detail".to_owned(),
                "error".to_owned(),
                "failed_phase".to_owned(),
                "mutation_applied".to_owned(),
                "operation_state".to_owned(),
                "reason_code".to_owned(),
                "safe_remediation".to_owned(),
                "source_id".to_owned(),
                "source_state".to_owned(),
                "target_id".to_owned(),
                "target_state".to_owned(),
            ])
        );
        assert_eq!(
            undo_keys,
            BTreeSet::from([
                "detail".to_owned(),
                "error".to_owned(),
                "merge_id".to_owned(),
                "mutation_applied".to_owned(),
                "operation_state".to_owned(),
                "reason_code".to_owned(),
                "safe_remediation".to_owned(),
                "source_id".to_owned(),
                "source_state".to_owned(),
                "target_id".to_owned(),
                "target_state".to_owned(),
            ])
        );
    }

    #[test]
    fn repair_refusal_variants_cannot_carry_both_variant_keys() {
        let merge_keys = keys(&to_value(merge()).unwrap());
        let undo_keys = keys(&to_value(undo()).unwrap());
        let differing: BTreeSet<_> = merge_keys
            .symmetric_difference(&undo_keys)
            .cloned()
            .collect();

        assert_eq!(
            differing,
            BTreeSet::from(["failed_phase".to_owned(), "merge_id".to_owned()])
        );
        assert!(!merge_keys.contains("merge_id") && !undo_keys.contains("failed_phase"));
    }
}
