// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Spending permission is independent of which work is owed and its priority.

use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_config::work_priority::WorkBand;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingLane {
    Bundled,
    OwnKeys,
    Confidential,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkPolicy {
    pub lane: ThinkingLane,
    pub history_confirmed: bool,
}

impl WorkPolicy {
    pub fn read(journal: &Path) -> Result<Self, String> {
        let config = solstone_core_journal_config::read_journal_config(journal)
            .map_err(|error| error.to_string())?
            .config
            .unwrap_or_default();
        Self::from_config(&config)
    }

    pub fn from_config(config: &Map<String, Value>) -> Result<Self, String> {
        // The fingerprint reader intentionally tolerates absent values. Wrong
        // types must not be coerced to an absent endpoint and grant local spend.
        let providers = object(config.get("providers"), "providers")?;
        let active = object(providers.and_then(|v| v.get("active")), "providers.active")?;
        let local = object(providers.and_then(|v| v.get("local")), "providers.local")?;
        strings(active, &["provider", "model"], "providers.active")?;
        strings(
            local,
            &["endpoint_url", "served_model_id", "credential"],
            "providers.local",
        )?;
        let services = object(config.get("services"), "services")?;
        let confidential = object(
            services.and_then(|v| v.get("confidential")),
            "services.confidential",
        )?;
        strings(
            confidential,
            &[
                "endpoint_url",
                "served_model_id",
                "credential_fingerprint_sha256",
            ],
            "services.confidential",
        )?;
        solstone_core_journal_config::processing::validate_processing(config.get("processing"))?;
        let lane = match solstone_core_brain::derive_active_brain_lane(config)
            .lane
            .as_deref()
        {
            Some("bundled") => ThinkingLane::Bundled,
            Some("byo-cloud" | "byo-endpoint") => ThinkingLane::OwnKeys,
            Some("spp") => ThinkingLane::Confidential,
            _ => ThinkingLane::Unavailable,
        };
        let processing = config.get("processing");
        let history_confirmed = processing
            .and_then(|v| v.get("history_confirmed"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(Self {
            lane,
            history_confirmed,
        })
    }

    pub fn permits(self, band: WorkBand) -> bool {
        match self.lane {
            ThinkingLane::Bundled | ThinkingLane::Confidential => true,
            ThinkingLane::OwnKeys => band <= WorkBand::Import || self.history_confirmed,
            ThinkingLane::Unavailable => false,
        }
    }
}

fn object<'a>(
    value: Option<&'a Value>,
    name: &str,
) -> Result<Option<&'a Map<String, Value>>, String> {
    value
        .map(|v| {
            v.as_object()
                .ok_or_else(|| format!("{name} must be an object"))
        })
        .transpose()
}

fn strings(object: Option<&Map<String, Value>>, keys: &[&str], name: &str) -> Result<(), String> {
    for key in keys {
        if object
            .and_then(|v| v.get(*key))
            .is_some_and(|v| !v.is_string())
        {
            return Err(format!("{name}.{key} must be a string"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    #[test]
    fn each_real_lane_has_the_required_spending_boundary() {
        let confidential = json!({"providers":{"active":{"provider":"local"},"local":{"endpoint_url":"https://example.test/v1","served_model_id":"model","credential":"test"}},"services":{"confidential":{"endpoint_url":"https://example.test","served_model_id":"model","credential_fingerprint_sha256":format!("{:x}", Sha256::digest(b"test"))}}});
        let own_endpoint = json!({"providers":{"active":{"provider":"local"},"local":{"endpoint_url":"https://example.test","served_model_id":"model"}}});
        for (config, lane, automatic) in [
            (
                json!({"providers":{"active":{"provider":"local"}}}),
                ThinkingLane::Bundled,
                6,
            ),
            (
                json!({"providers":{"active":{"provider":"openai","model":"model"}}}),
                ThinkingLane::OwnKeys,
                4,
            ),
            (own_endpoint, ThinkingLane::OwnKeys, 4),
            (confidential.clone(), ThinkingLane::Confidential, 6),
            (json!({}), ThinkingLane::Unavailable, 0),
        ] {
            let mut policy = WorkPolicy::from_config(config.as_object().unwrap()).unwrap();
            assert_eq!(policy.lane, lane);
            for band in [
                WorkBand::Now,
                WorkBand::Waiting,
                WorkBand::Recent,
                WorkBand::Import,
                WorkBand::Completing,
                WorkBand::Remake,
            ] {
                assert_eq!(policy.permits(band), band as u8 <= automatic);
            }
            policy.history_confirmed = true;
            assert_eq!(
                policy.permits(WorkBand::Remake),
                lane != ThinkingLane::Unavailable
            );
        }
    }

    #[test]
    fn malformed_endpoint_or_permission_never_grants_local_spending() {
        for local in [
            json!(null),
            json!([]),
            json!({"endpoint_url":false,"served_model_id":42}),
            json!({"credential":false}),
        ] {
            let config = json!({"providers":{"active":{"provider":"local"},"local":local}});
            assert!(WorkPolicy::from_config(config.as_object().unwrap()).is_err());
        }
        for config in [
            json!({"providers":[]}),
            json!({"providers":{"active":{"provider":true}}}),
            json!({"services":{"confidential":false}}),
            json!({"processing":{"history_confirmed":"true"}}),
        ] {
            assert!(WorkPolicy::from_config(config.as_object().unwrap()).is_err());
        }
        let partial = json!({"providers":{"active":{"provider":"local"},"local":{"endpoint_url":"https://example.test"}}});
        assert!(
            !WorkPolicy::from_config(partial.as_object().unwrap())
                .unwrap()
                .permits(WorkBand::Now)
        );
    }
}
