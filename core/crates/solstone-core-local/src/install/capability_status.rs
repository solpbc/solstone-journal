// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared status vocabulary for an optional native capability.
//!
//! States: ready, absent, integrity-invalid, unloadable-or-unrunnable,
//! wrong-ABI-or-protocol, and resource-or-owner-scope-unavailable. Every
//! non-ready variant carries the affected capability identifier and a
//! human-readable description.

use serde::{Deserialize, Serialize};
use solstone_core_cli_copy::{ENGLISH, MessageDetail};
use std::fmt;

pub type CapabilityId = String;

/// Serialize the source text. A live producer may also retain the facts used
/// to render the same diagnostic for the client that requested the work.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityDetail {
    english: String,
    #[serde(skip)]
    presentation: Option<Box<Presentation>>,
}

#[derive(Debug, Clone)]
enum Presentation {
    Message(MessageDetail),
    Guidance { technical: String, id: &'static str },
}

impl Presentation {
    fn render(&self, locale: &str) -> String {
        match self {
            Self::Message(message) => message.render(locale),
            Self::Guidance { technical, id } => format!(
                "{technical} {}",
                solstone_core_window_catalog::text(locale, id)
            ),
        }
    }
}

impl CapabilityDetail {
    fn new(presentation: Presentation) -> Self {
        Self {
            english: presentation.render(ENGLISH),
            presentation: Some(Box::new(presentation)),
        }
    }

    pub fn message<const N: usize>(
        id: &'static str,
        values: [(&'static str, &dyn fmt::Display); N],
    ) -> Self {
        MessageDetail::message(id, values).into()
    }

    pub(super) fn guidance(technical: String, id: &'static str) -> Self {
        Self::new(Presentation::Guidance { technical, id })
    }

    pub fn render(&self, locale: &str) -> String {
        self.presentation
            .as_ref()
            .map_or_else(|| self.english.clone(), |copy| copy.render(locale))
    }
}

impl From<MessageDetail> for CapabilityDetail {
    fn from(detail: MessageDetail) -> Self {
        Self::new(Presentation::Message(detail))
    }
}

impl From<String> for CapabilityDetail {
    fn from(english: String) -> Self {
        Self {
            english,
            presentation: None,
        }
    }
}

impl From<&str> for CapabilityDetail {
    fn from(english: &str) -> Self {
        english.to_owned().into()
    }
}

impl std::ops::Deref for CapabilityDetail {
    type Target = str;
    fn deref(&self) -> &str {
        &self.english
    }
}

impl fmt::Debug for CapabilityDetail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.english, formatter)
    }
}

impl fmt::Display for CapabilityDetail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.english)
    }
}

// Equality describes the serialized status, including across a round trip.
impl PartialEq for CapabilityDetail {
    fn eq(&self, other: &Self) -> bool {
        self.english == other.english
    }
}
impl Eq for CapabilityDetail {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CapabilityStatus {
    Ready,
    Absent {
        capability: CapabilityId,
        detail: CapabilityDetail,
    },
    IntegrityInvalid {
        capability: CapabilityId,
        detail: CapabilityDetail,
    },
    UnloadableOrUnrunnable {
        capability: CapabilityId,
        detail: CapabilityDetail,
    },
    WrongAbiOrProtocol {
        capability: CapabilityId,
        detail: CapabilityDetail,
    },
    ResourceOrOwnerScopeUnavailable {
        capability: CapabilityId,
        detail: CapabilityDetail,
    },
}

impl CapabilityStatus {
    /// The technical detail carried by any non-`Ready` variant, or `None` for
    /// `Ready`. This is the specific failure text (a digest mismatch, a
    /// missing file, a probe helper's stderr) behind a coarse `cause` bucket
    /// or a fixed owner-facing sentence -- a caller that discards it and
    /// reports only the fixed sentence loses the one thing an operator needs
    /// to root-cause a `Degraded` verdict.
    pub fn detail(&self) -> Option<&str> {
        self.failure_detail().map(|detail| &**detail)
    }

    pub fn display_detail(&self, locale: &str) -> Option<String> {
        self.failure_detail().map(|detail| detail.render(locale))
    }

    fn failure_detail(&self) -> Option<&CapabilityDetail> {
        match self {
            CapabilityStatus::Ready => None,
            CapabilityStatus::Absent { detail, .. }
            | CapabilityStatus::IntegrityInvalid { detail, .. }
            | CapabilityStatus::UnloadableOrUnrunnable { detail, .. }
            | CapabilityStatus::WrongAbiOrProtocol { detail, .. }
            | CapabilityStatus::ResourceOrOwnerScopeUnavailable { detail, .. } => Some(detail),
        }
    }

    /// The cause token identifying the degradation classification.
    pub fn cause_token(&self) -> Option<&'static str> {
        match self {
            CapabilityStatus::Ready => None,
            CapabilityStatus::Absent { .. } => Some("absent"),
            CapabilityStatus::IntegrityInvalid { .. } => Some("integrity_invalid"),
            CapabilityStatus::UnloadableOrUnrunnable { .. } => Some("unloadable"),
            CapabilityStatus::WrongAbiOrProtocol { .. } => Some("wrong_abi_or_protocol"),
            CapabilityStatus::ResourceOrOwnerScopeUnavailable { .. } => {
                Some("resource_or_owner_scope_unavailable")
            }
        }
    }
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
