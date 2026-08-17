// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::LoopbackAddr;
use crate::plan::Platform;
use crate::tier::{
    CAPABLE_CONTEXT_TOKENS, CAPABLE_PARALLEL_SLOTS, FLOOR_CONTEXT_TOKENS, FLOOR_PARALLEL_SLOTS,
};

const INPUT_SCHEMA: &str = "solstone-local-connect-input-v1";
const UNKNOWN_SLOTS: u32 = 1;
const TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectInput {
    pub schema: String,
    pub journal_path: String,
    pub bind_address: LoopbackAddr,
    pub default_model_id: String,
    pub platform: Platform,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConnectedServer {
    pub model_id: String,
    pub served_model_id: String,
    pub port: u16,
    pub base_url: String,
    pub parallel_slots: u32,
    pub capacity_source: String,
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum ConnectOutcome {
    Ready { server: ConnectedServer },
    Loading { reason: String },
    NotReady { reason: String },
    Failed { reason: String },
}

pub(crate) trait ConnectTransport {
    fn get(&self, base_url: &str, path: &str) -> Result<(u16, String), String>;
}

struct UreqConnectTransport;

impl ConnectTransport for UreqConnectTransport {
    fn get(&self, base_url: &str, path: &str) -> Result<(u16, String), String> {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(TIMEOUT))
            .timeout_recv_response(Some(TIMEOUT))
            .timeout_recv_body(Some(TIMEOUT))
            .timeout_global(Some(TIMEOUT * 2))
            .build();
        let response = ureq::Agent::new_with_config(config)
            .get(&format!("{base_url}{path}"))
            .call()
            .map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let mut body = response.into_body();
        body.read_to_string()
            .map(|text| (status, text))
            .map_err(|error| error.to_string())
    }
}

pub fn connect(input: ConnectInput) -> ConnectOutcome {
    connect_with(input, &UreqConnectTransport)
}

pub(crate) fn connect_with(
    input: ConnectInput,
    transport: &dyn ConnectTransport,
) -> ConnectOutcome {
    if input.schema != INPUT_SCHEMA {
        return ConnectOutcome::Failed {
            reason: "unsupported connect input schema".into(),
        };
    }
    let health_dir = PathBuf::from(&input.journal_path).join("health");
    let port = match std::fs::read_to_string(health_dir.join("local.port"))
        .ok()
        .and_then(|text| text.trim().parse::<u16>().ok())
    {
        Some(port) => port,
        None => {
            return ConnectOutcome::NotReady {
                reason: "no local service port".into(),
            };
        }
    };
    let base_url = format!("http://{}:{port}", input.bind_address);
    let health = transport.get(&base_url, "/health");
    let served_model_id = match health {
        Ok((200, text)) => match serde_json::from_str::<Value>(&text).ok() {
            Some(Value::Object(body)) => match body.get("loaded_model") {
                None => input.default_model_id.clone(),
                Some(Value::String(value)) if !value.trim().is_empty() => value.clone(),
                Some(_) => {
                    return ConnectOutcome::NotReady {
                        reason: "health loaded_model is blank or invalid".into(),
                    };
                }
            },
            _ => input.default_model_id.clone(),
        },
        Ok((503, text)) if text.to_ascii_lowercase().contains("loading model") => {
            return ConnectOutcome::Loading {
                reason: "loading model".into(),
            };
        }
        Ok((status, text)) => {
            return ConnectOutcome::Failed {
                reason: format!("HTTP {status}: {}", truncate(&text)),
            };
        }
        Err(reason) => return ConnectOutcome::Failed { reason },
    };
    let capacity = discover_capacity(transport, &base_url, &health_dir, input.platform);
    ConnectOutcome::Ready {
        server: ConnectedServer {
            model_id: input.default_model_id,
            served_model_id,
            port,
            base_url,
            parallel_slots: capacity.0,
            capacity_source: capacity.1,
            profile: profile_for_slots(input.platform, capacity.0).into(),
        },
    }
}

fn discover_capacity(
    transport: &dyn ConnectTransport,
    base_url: &str,
    health_dir: &std::path::Path,
    _platform: Platform,
) -> (u32, String) {
    if let Ok((200, text)) = transport.get(base_url, "/props")
        && let Some(slots) = total_slots(&text)
    {
        return (slots, "props".into());
    }
    let context = std::fs::read_to_string(health_dir.join("local.ctx"))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok());
    if let Some(slots) = slots_from_launched_tier(context) {
        return (slots, "local_ctx".into());
    }
    (UNKNOWN_SLOTS, "default".into())
}

fn total_slots(props: &str) -> Option<u32> {
    serde_json::from_str::<Value>(props)
        .ok()
        .and_then(|value| value.get("total_slots")?.as_u64())
        .and_then(|slots| u32::try_from(slots).ok())
        .filter(|slots| *slots > 0)
}

fn slots_from_launched_tier(context: Option<u32>) -> Option<u32> {
    match context {
        Some(FLOOR_CONTEXT_TOKENS) => Some(FLOOR_PARALLEL_SLOTS),
        Some(CAPABLE_CONTEXT_TOKENS) => Some(CAPABLE_PARALLEL_SLOTS),
        _ => None,
    }
}
fn profile_for_slots(platform: Platform, slots: u32) -> &'static str {
    if platform == Platform::Darwin {
        "apple"
    } else if slots == 2 {
        "capable"
    } else if slots == 1 {
        "floor"
    } else {
        "advertised"
    }
}
fn truncate(text: &str) -> &str {
    &text[..text
        .char_indices()
        .nth(200)
        .map_or(text.len(), |(index, _)| index)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::Path;

    struct ScriptedConnect {
        responses: RefCell<Vec<Result<(u16, String), String>>>,
        calls: RefCell<Vec<String>>,
    }

    impl ConnectTransport for ScriptedConnect {
        fn get(&self, _: &str, path: &str) -> Result<(u16, String), String> {
            self.calls.borrow_mut().push(path.to_owned());
            self.responses.borrow_mut().remove(0)
        }
    }

    // Port is written only so connect_with's file parse succeeds. The scripted
    // transport never opens a socket, so any parseable u16 is fine.
    fn journal(port: Option<u16>, context: Option<&str>) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("temp journal");
        let health = root.path().join("health");
        std::fs::create_dir_all(&health).expect("health directory");
        if let Some(port) = port {
            std::fs::write(health.join("local.port"), port.to_string()).expect("port");
        }
        if let Some(context) = context {
            std::fs::write(health.join("local.ctx"), context).expect("context");
        }
        root
    }

    fn input(root: &Path) -> ConnectInput {
        ConnectInput {
            schema: INPUT_SCHEMA.into(),
            journal_path: root.display().to_string(),
            bind_address: LoopbackAddr::IPV4_LOOPBACK,
            default_model_id: "default".into(),
            platform: Platform::Linux,
        }
    }

    fn ready(outcome: ConnectOutcome) -> ConnectedServer {
        match outcome {
            ConnectOutcome::Ready { server } => server,
            other => panic!("expected ready, got {other:?}"),
        }
    }

    fn scripted(responses: Vec<Result<(u16, String), String>>) -> ScriptedConnect {
        ScriptedConnect {
            responses: RefCell::new(responses),
            calls: RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn slots_and_profiles_match_python_precedence_rules() {
        assert_eq!(total_slots(r#"{"total_slots": 3}"#), Some(3));
        assert_eq!(
            slots_from_launched_tier(Some(FLOOR_CONTEXT_TOKENS)),
            Some(FLOOR_PARALLEL_SLOTS)
        );
        assert_eq!(
            slots_from_launched_tier(Some(CAPABLE_CONTEXT_TOKENS)),
            Some(CAPABLE_PARALLEL_SLOTS)
        );
        assert_eq!(slots_from_launched_tier(None), None);
        assert_eq!(UNKNOWN_SLOTS, 1);
        assert_eq!(profile_for_slots(Platform::Darwin, 99), "apple");
        assert_eq!(profile_for_slots(Platform::Linux, 2), "capable");
        assert_eq!(profile_for_slots(Platform::Linux, 1), "floor");
        assert_eq!(profile_for_slots(Platform::Linux, 3), "advertised");
    }

    #[test]
    fn context_is_unmultiplied_tier_value() {
        let context = CAPABLE_CONTEXT_TOKENS;
        let launched = context * slots_from_launched_tier(Some(context)).expect("capable slots");
        assert_eq!(launched, context * 2);
    }

    #[test]
    fn connect_defaults_served_model_and_prefers_props_capacity() {
        let transport = scripted(vec![
            Ok((200, "{}".into())),
            Ok((200, r#"{"total_slots":3}"#.into())),
        ]);
        let root = journal(Some(1), Some("32768"));
        let server = ready(connect_with(input(root.path()), &transport));
        assert_eq!(server.served_model_id, "default");
        assert_eq!(
            (server.parallel_slots, server.capacity_source.as_str()),
            (3, "props")
        );
        assert_eq!(*transport.calls.borrow(), ["/health", "/props"]);
    }

    #[test]
    fn connect_uses_served_model_and_context_capacity_fallback() {
        let transport = scripted(vec![
            Ok((200, r#"{"loaded_model":"served"}"#.into())),
            Ok((200, "{}".into())),
        ]);
        let root = journal(Some(1), Some("32768"));
        let server = ready(connect_with(input(root.path()), &transport));
        assert_eq!(server.served_model_id, "served");
        assert_eq!(
            (server.parallel_slots, server.capacity_source.as_str()),
            (2, "local_ctx")
        );
    }

    #[test]
    fn connect_rejects_blank_served_model() {
        let transport = scripted(vec![Ok((200, r#"{"loaded_model":""}"#.into()))]);
        let root = journal(Some(1), None);
        assert!(matches!(
            connect_with(input(root.path()), &transport),
            ConnectOutcome::NotReady { .. }
        ));
    }

    #[test]
    fn connect_rejects_non_string_served_model() {
        let transport = scripted(vec![Ok((200, r#"{"loaded_model":1}"#.into()))]);
        let root = journal(Some(1), None);
        assert!(matches!(
            connect_with(input(root.path()), &transport),
            ConnectOutcome::NotReady { .. }
        ));
    }

    #[test]
    fn connect_rejects_unsupported_schema_and_missing_port() {
        let transport = scripted(vec![]);
        let mut invalid_schema = input(Path::new("/unused"));
        invalid_schema.schema = "unsupported".into();
        assert!(matches!(
            connect_with(invalid_schema, &transport),
            ConnectOutcome::Failed { ref reason } if reason == "unsupported connect input schema"
        ));
        assert!(transport.calls.borrow().is_empty());

        let root = journal(None, None);
        assert!(matches!(
            connect_with(input(root.path()), &transport),
            ConnectOutcome::NotReady { ref reason } if reason == "no local service port"
        ));
        assert!(transport.calls.borrow().is_empty());
    }

    #[test]
    fn connect_reports_loading_and_unexpected_http_status() {
        let loading = scripted(vec![Ok((503, "loading model".into()))]);
        let loading_root = journal(Some(1), None);
        assert!(matches!(
            connect_with(input(loading_root.path()), &loading),
            ConnectOutcome::Loading { ref reason } if reason == "loading model"
        ));

        let failed = scripted(vec![Ok((500, String::new()))]);
        let failed_root = journal(Some(1), None);
        assert!(matches!(
            connect_with(input(failed_root.path()), &failed),
            ConnectOutcome::Failed { ref reason } if reason.starts_with("HTTP 500:")
        ));
    }

    #[test]
    fn connect_uses_default_capacity_when_props_and_context_are_unavailable() {
        let transport = scripted(vec![Ok((200, "{}".into())), Ok((500, String::new()))]);
        let root = journal(Some(1), None);
        let server = ready(connect_with(input(root.path()), &transport));
        assert_eq!(
            (server.parallel_slots, server.capacity_source.as_str()),
            (UNKNOWN_SLOTS, "default")
        );
    }
}
