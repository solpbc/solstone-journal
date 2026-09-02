// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared status vocabulary for an optional native capability.
//!
//! States: ready, absent, integrity-invalid, unloadable-or-unrunnable,
//! wrong-ABI-or-protocol, and resource-or-owner-scope-unavailable. Every
//! non-ready variant carries the affected capability identifier and a
//! human-readable description.

use serde::{Deserialize, Serialize};

pub type CapabilityId = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CapabilityStatus {
    Ready,
    Absent {
        capability: CapabilityId,
        detail: String,
    },
    IntegrityInvalid {
        capability: CapabilityId,
        detail: String,
    },
    UnloadableOrUnrunnable {
        capability: CapabilityId,
        detail: String,
    },
    WrongAbiOrProtocol {
        capability: CapabilityId,
        detail: String,
    },
    ResourceOrOwnerScopeUnavailable {
        capability: CapabilityId,
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use super::CapabilityStatus;
    use serde_json::json;

    fn all_variants() -> [CapabilityStatus; 6] {
        [
            CapabilityStatus::Ready,
            CapabilityStatus::Absent {
                capability: "widget".into(),
                detail: "missing".into(),
            },
            CapabilityStatus::IntegrityInvalid {
                capability: "widget".into(),
                detail: "digest mismatch".into(),
            },
            CapabilityStatus::UnloadableOrUnrunnable {
                capability: "widget".into(),
                detail: "failed to load".into(),
            },
            CapabilityStatus::WrongAbiOrProtocol {
                capability: "widget".into(),
                detail: "abi mismatch".into(),
            },
            CapabilityStatus::ResourceOrOwnerScopeUnavailable {
                capability: "widget".into(),
                detail: "scope unavailable".into(),
            },
        ]
    }

    #[test]
    fn six_states_are_pairwise_distinct_and_round_trip() {
        let variants = all_variants();
        for (i, left) in variants.iter().enumerate() {
            for (j, right) in variants.iter().enumerate() {
                if i == j {
                    assert_eq!(left, right);
                } else {
                    assert_ne!(left, right, "{left:?} vs {right:?}");
                }
            }
        }

        let ready = CapabilityStatus::Ready;
        let ready_json = serde_json::to_value(&ready).unwrap();
        assert_eq!(ready_json, json!({"status": "ready"}));
        assert_eq!(
            serde_json::from_value::<CapabilityStatus>(ready_json).unwrap(),
            ready
        );

        let absent = CapabilityStatus::Absent {
            capability: "widget".into(),
            detail: "missing".into(),
        };
        let absent_json = serde_json::to_value(&absent).unwrap();
        assert_eq!(
            absent_json,
            json!({"status": "absent", "capability": "widget", "detail": "missing"})
        );
        assert_eq!(
            serde_json::from_value::<CapabilityStatus>(absent_json).unwrap(),
            absent
        );
    }
}
