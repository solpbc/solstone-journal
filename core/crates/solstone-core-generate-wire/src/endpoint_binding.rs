// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A reject-only precondition on the lane already selected by native dispatch.
use crate::LaneOutcome;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// Bind the resolved public endpoint descriptor, including process overrides.
/// Credentials are neither hashed nor returned. This never selects an endpoint.
pub fn endpoint_configuration_binding(config: &Map<String, Value>) -> Option<String> {
    let (provider, lane) = crate::resolve_lane(config);
    resolved_binding(
        &provider,
        &lane,
        crate::overrides::configured_model(config).as_deref(),
    )
}

pub(crate) fn resolved_binding(
    provider: &str,
    lane: &LaneOutcome,
    model: Option<&str>,
) -> Option<String> {
    let endpoint = match lane {
        LaneOutcome::ConfidentialEndpoint(endpoint) | LaneOutcome::ByoEndpoint(endpoint) => {
            endpoint
        }
        _ => return None,
    };
    let descriptor = json!([
        1,
        provider,
        model,
        endpoint.base_url,
        endpoint.served_model_id,
        endpoint.is_confidential,
        endpoint.is_bundled
    ]);
    Some(format!(
        "{:x}",
        Sha256::digest(descriptor.to_string().as_bytes())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binding_changes_with_every_public_execution_coordinate() {
        let config = json!({"providers":{"active":{"provider":"local","model":"local/model"},"local":{"endpoint_url":"https://example.test","served_model_id":"model"}},"services":{"confidential":{}}});
        let config = config.as_object().unwrap();
        let (provider, lane) = crate::lane::resolve_lane_with(config, |_| None);
        let bound = resolved_binding(&provider, &lane, Some("local/model")).unwrap();
        for path in [
            "/providers/local/endpoint_url",
            "/providers/local/served_model_id",
        ] {
            let mut changed = Value::Object(config.clone());
            *changed.pointer_mut(path).unwrap() = json!("changed");
            let (p, l) = crate::lane::resolve_lane_with(changed.as_object().unwrap(), |_| None);
            assert_ne!(
                resolved_binding(&p, &l, Some("local/model")).as_deref(),
                Some(bound.as_str())
            );
        }
        assert_ne!(
            resolved_binding(&provider, &lane, Some("different")).as_deref(),
            Some(bound.as_str())
        );
        let mut changed = Value::Object(config.clone());
        changed.as_object_mut().unwrap().remove("services");
        let (p, l) = crate::lane::resolve_lane_with(changed.as_object().unwrap(), |_| None);
        assert_ne!(
            resolved_binding(&p, &l, Some("local/model")).as_deref(),
            Some(bound.as_str())
        );
        let (p, l) = crate::lane::resolve_lane_with(config, |name| {
            (name == crate::overrides::PROVIDER_OVERRIDE_ENV).then(|| "openai".into())
        });
        assert!(resolved_binding(&p, &l, Some("local/model")).is_none());
    }
}
