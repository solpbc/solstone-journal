// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-local overrides used only by native generate validation probes.

use serde_json::{Map, Value};

pub const API_KEY_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_API_KEY_OVERRIDE";
pub const BASE_URL_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_BASE_URL_OVERRIDE";
pub const MODEL_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_MODEL_OVERRIDE";
pub const PROVIDER_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_PROVIDER_OVERRIDE";

pub fn configured_api_key(config: &Map<String, Value>, config_key: &str) -> Option<String> {
    // Never consult conventional provider environment variables here: they may be
    // ambient host credentials. Only this dedicated child-only override may beat config.
    configured_api_key_with(config, config_key, non_blank_process_env)
}

pub(crate) fn configured_api_key_with(
    config: &Map<String, Value>,
    config_key: &str,
    env: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    take_env(&env, API_KEY_OVERRIDE_ENV).or_else(|| config_string(config, &["env", config_key]))
}

pub fn configured_model(config: &Map<String, Value>, default: &str) -> String {
    configured_model_with(config, default, non_blank_process_env)
}

pub(crate) fn configured_model_with(
    config: &Map<String, Value>,
    default: &str,
    env: impl Fn(&str) -> Option<String>,
) -> String {
    take_env(&env, MODEL_OVERRIDE_ENV)
        .or_else(|| config_string(config, &["providers", "active", "model"]))
        .unwrap_or_else(|| default.to_owned())
}

/// Resolve a provider base URL, honouring the override only for loopback hosts.
///
/// The override exists so a test can aim a cloud arm at a local stub. Honouring an
/// arbitrary host would let anything able to set an environment variable redirect
/// cloud traffic -- carrying the provider credential in its auth header and the
/// owner's prompt in its body -- to a destination of its choosing. Restricting it
/// to loopback keeps the test seam and removes that channel. This file already
/// refuses to read conventional provider environment variables for the same
/// reason: ambient process environment is not a trusted input.
pub fn configured_base_url(_config: &Map<String, Value>, default: &str) -> String {
    configured_base_url_with(_config, default, non_blank_process_env)
}

pub(crate) fn configured_base_url_with(
    _config: &Map<String, Value>,
    default: &str,
    env: impl Fn(&str) -> Option<String>,
) -> String {
    take_env(&env, BASE_URL_OVERRIDE_ENV)
        .filter(|url| is_loopback_base_url(url))
        .unwrap_or_else(|| default.to_owned())
}

/// True only for an http/https URL whose host is a loopback address.
///
/// Rejects userinfo (`http://127.0.0.1@elsewhere`), non-http schemes, and any
/// host that merely mentions a loopback literal elsewhere in the string.
fn is_loopback_base_url(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return false;
    }
    let host = if let Some(after_bracket) = authority.strip_prefix('[') {
        match after_bracket.split_once(']') {
            Some((inner, tail)) if tail.is_empty() || tail.starts_with(':') => inner,
            _ => return false,
        }
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    host == "localhost"
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|address| address.is_loopback())
}

pub fn configured_provider(config: &Map<String, Value>) -> String {
    configured_provider_with(config, non_blank_process_env)
}

pub(crate) fn configured_provider_with(
    config: &Map<String, Value>,
    env: impl Fn(&str) -> Option<String>,
) -> String {
    take_env(&env, PROVIDER_OVERRIDE_ENV)
        .or_else(|| config_string(config, &["providers", "active", "provider"]))
        .unwrap_or_else(|| "none".to_owned())
}

pub(crate) fn non_blank_process_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn take_env(env: &impl Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    env(name)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
pub(crate) fn lookup_leaks_conventional_keys(name: &str) -> Option<String> {
    match name {
        API_KEY_OVERRIDE_ENV
        | BASE_URL_OVERRIDE_ENV
        | MODEL_OVERRIDE_ENV
        | PROVIDER_OVERRIDE_ENV => None,
        _ => Some("process-only-secret".to_owned()),
    }
}

#[cfg(test)]
pub(crate) fn lookup_api_key_override(name: &str) -> Option<String> {
    (name == API_KEY_OVERRIDE_ENV).then(|| "override-secret".to_owned())
}

fn config_string(config: &Map<String, Value>, path: &[&str]) -> Option<String> {
    let (first, rest) = path.split_first()?;
    let mut value = config.get(*first)?;
    for key in rest {
        value = value.as_object()?.get(*key)?;
    }
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::is_loopback_base_url;

    #[test]
    fn base_url_override_is_honoured_only_for_loopback_hosts() {
        for accepted in [
            "http://127.0.0.1:8080",
            "http://127.0.0.1:8080/v1",
            "http://127.9.9.9:1",
            "http://localhost:3000",
            "http://[::1]:9000",
            "https://127.0.0.1:8443",
            "http://[::1]",
            "http://[::1]/v1",
            "http://127.0.0.1",
        ] {
            assert!(is_loopback_base_url(accepted), "should accept {accepted}");
        }
        for rejected in [
            "https://generativelanguage.googleapis.com",
            "http://evil.example",
            // userinfo cannot smuggle a loopback prefix past the host check
            "http://127.0.0.1@evil.example",
            "http://localhost@evil.example:80",
            "http://[::1]@evil.example",
            // a loopback literal in the path is not a loopback host
            "http://evil.example/127.0.0.1",
            "http://evil.example?h=localhost",
            "http://evil.example#127.0.0.1",
            // non-http schemes are never honoured
            "ftp://127.0.0.1",
            "file:///etc/passwd",
            "127.0.0.1:8080",
            "",
            // empty or port-only authority
            "http://",
            "http:///",
            "http://:8080",
        ] {
            assert!(!is_loopback_base_url(rejected), "should reject {rejected}");
        }
    }

    use serde_json::{Map, Value, json};

    use super::*;

    fn config(
        provider: Option<&str>,
        model: Option<&str>,
        key: Option<&str>,
    ) -> Map<String, Value> {
        let mut active = Map::new();
        if let Some(provider) = provider {
            active.insert("provider".into(), json!(provider));
        }
        if let Some(model) = model {
            active.insert("model".into(), json!(model));
        }
        let mut env = Map::new();
        if let Some(key) = key {
            env.insert("OPENAI_API_KEY".into(), json!(key));
        }
        Map::from_iter([
            (
                "providers".into(),
                Value::Object(Map::from_iter([("active".into(), Value::Object(active))])),
            ),
            ("env".into(), Value::Object(env)),
        ])
    }

    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn api_override_without_config_wins() {
        assert_eq!(
            configured_api_key_with(
                &config(None, None, None),
                "OPENAI_API_KEY",
                lookup(&[(API_KEY_OVERRIDE_ENV, "override")]),
            )
            .as_deref(),
            Some("override")
        );
    }

    #[test]
    fn api_override_beats_config() {
        assert_eq!(
            configured_api_key_with(
                &config(None, None, Some("stored")),
                "OPENAI_API_KEY",
                lookup(&[(API_KEY_OVERRIDE_ENV, "override")]),
            )
            .as_deref(),
            Some("override")
        );
    }

    #[test]
    fn api_config_ignores_conventional_process_env() {
        assert_eq!(
            configured_api_key_with(
                &config(None, None, Some("stored")),
                "OPENAI_API_KEY",
                lookup(&[("OPENAI_API_KEY", "ambient")]),
            )
            .as_deref(),
            Some("stored")
        );
    }

    #[test]
    fn provider_override_without_config_wins() {
        assert_eq!(
            configured_provider_with(
                &config(None, None, None),
                lookup(&[(PROVIDER_OVERRIDE_ENV, "google")]),
            ),
            "google"
        );
    }

    #[test]
    fn provider_override_beats_config() {
        assert_eq!(
            configured_provider_with(
                &config(Some("openai"), None, None),
                lookup(&[(PROVIDER_OVERRIDE_ENV, "google")]),
            ),
            "google"
        );
    }

    #[test]
    fn provider_config_is_used_without_override() {
        assert_eq!(
            configured_provider_with(&config(Some("openai"), None, None), lookup(&[])),
            "openai"
        );
    }

    #[test]
    fn model_override_without_config_wins() {
        assert_eq!(
            configured_model_with(
                &config(None, None, None),
                "default",
                lookup(&[(MODEL_OVERRIDE_ENV, "candidate")]),
            ),
            "candidate"
        );
    }

    #[test]
    fn model_override_beats_config() {
        assert_eq!(
            configured_model_with(
                &config(None, Some("stored"), None),
                "default",
                lookup(&[(MODEL_OVERRIDE_ENV, "candidate")]),
            ),
            "candidate"
        );
    }

    #[test]
    fn model_config_is_used_without_override() {
        assert_eq!(
            configured_model_with(&config(None, Some("stored"), None), "default", lookup(&[])),
            "stored"
        );
    }
}
