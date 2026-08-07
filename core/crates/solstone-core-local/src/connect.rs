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

pub fn connect(input: ConnectInput) -> ConnectOutcome {
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
    let health = get(&base_url, "/health");
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
    let capacity = discover_capacity(&base_url, &health_dir, input.platform);
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

fn get(base_url: &str, path: &str) -> Result<(u16, String), String> {
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

fn discover_capacity(
    base_url: &str,
    health_dir: &std::path::Path,
    _platform: Platform,
) -> (u32, String) {
    if let Ok((200, text)) = get(base_url, "/props")
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
    use std::io::Write;
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::Instant;

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
    fn health_response_timeout_is_bounded() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let thread = thread::spawn(move || {
            let _ = listener.accept();
            thread::sleep(Duration::from_secs(2));
        });
        let started = Instant::now();
        assert!(get(&format!("http://{address}"), "/health").is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        thread.join().expect("join");
    }
    #[test]
    fn context_is_unmultiplied_tier_value() {
        let context = CAPABLE_CONTEXT_TOKENS;
        let launched = context * slots_from_launched_tier(Some(context)).expect("capable slots");
        assert_eq!(launched, context * 2);
    }

    fn serve(responses: Vec<&'static str>) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept");
                stream.write_all(response.as_bytes()).expect("response");
            }
        });
        (port, handle)
    }

    fn journal(port: u16, context: Option<&str>) -> PathBuf {
        let root = std::env::temp_dir().join(format!("solstone-local-connect-{port}"));
        let health = root.join("health");
        std::fs::create_dir_all(&health).expect("health directory");
        std::fs::write(health.join("local.port"), port.to_string()).expect("port");
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

    #[test]
    fn connect_defaults_served_model_and_prefers_props_capacity() {
        let (port, handle) = serve(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\n{\"total_slots\":3}",
        ]);
        let root = journal(port, Some("32768"));
        let server = ready(connect(input(&root)));
        assert_eq!(server.served_model_id, "default");
        assert_eq!(
            (server.parallel_slots, server.capacity_source.as_str()),
            (3, "props")
        );
        handle.join().expect("server");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn connect_uses_served_model_and_context_capacity_fallback() {
        let (port, handle) = serve(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 25\r\n\r\n{\"loaded_model\":\"served\"}",
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
        ]);
        let root = journal(port, Some("32768"));
        let server = ready(connect(input(&root)));
        assert_eq!(server.served_model_id, "served");
        assert_eq!(
            (server.parallel_slots, server.capacity_source.as_str()),
            (2, "local_ctx")
        );
        handle.join().expect("server");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn connect_rejects_blank_served_model() {
        let (port, handle) = serve(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 19\r\n\r\n{\"loaded_model\":\"\"}",
        ]);
        let root = journal(port, None);
        assert!(matches!(
            connect(input(&root)),
            ConnectOutcome::NotReady { .. }
        ));
        handle.join().expect("server");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn connect_uses_default_capacity_when_props_and_context_are_unavailable() {
        let (port, handle) = serve(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n",
        ]);
        let root = journal(port, None);
        let server = ready(connect(input(&root)));
        assert_eq!(
            (server.parallel_slots, server.capacity_source.as_str()),
            (UNKNOWN_SLOTS, "default")
        );
        handle.join().expect("server");
        let _ = std::fs::remove_dir_all(root);
    }
}
