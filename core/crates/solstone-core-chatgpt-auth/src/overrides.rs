// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host loopback base-URL overrides for ChatGPT endpoints.

pub const AUTH_BASE_URL_OVERRIDE_ENV: &str = "SOLSTONE_CHATGPT_AUTH_BASE_URL_OVERRIDE";
pub const API_BASE_URL_OVERRIDE_ENV: &str = "SOLSTONE_CHATGPT_API_BASE_URL_OVERRIDE";

pub const DEFAULT_AUTH_BASE_URL: &str = "https://auth.openai.com";
pub const DEFAULT_API_BASE_URL: &str = "https://api.openai.com";

pub fn auth_base_url() -> String {
    configured_base_url(AUTH_BASE_URL_OVERRIDE_ENV, DEFAULT_AUTH_BASE_URL)
}

pub fn api_base_url() -> String {
    configured_base_url(API_BASE_URL_OVERRIDE_ENV, DEFAULT_API_BASE_URL)
}

#[cfg(any(test, feature = "test-hooks"))]
static AUTH_BASE_URL_OVERRIDE: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);
#[cfg(any(test, feature = "test-hooks"))]
static API_BASE_URL_OVERRIDE: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

#[cfg(any(test, feature = "test-hooks"))]
pub fn set_test_auth_base_url_override(url: Option<String>) {
    if let Ok(mut lock) = AUTH_BASE_URL_OVERRIDE.write() {
        *lock = url;
    }
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn set_test_api_base_url_override(url: Option<String>) {
    if let Ok(mut lock) = API_BASE_URL_OVERRIDE.write() {
        *lock = url;
    }
}

fn configured_base_url(env_name: &str, default: &str) -> String {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        let override_opt = if env_name == AUTH_BASE_URL_OVERRIDE_ENV {
            AUTH_BASE_URL_OVERRIDE.read().ok().and_then(|l| l.clone())
        } else if env_name == API_BASE_URL_OVERRIDE_ENV {
            API_BASE_URL_OVERRIDE.read().ok().and_then(|l| l.clone())
        } else {
            None
        };

        if let Some(url) = override_opt
            && is_loopback_base_url(&url)
        {
            return url;
        }
    }

    match std::env::var(env_name) {
        Ok(value) => {
            let trimmed = value.trim().trim_end_matches('/');
            if is_loopback_base_url(trimmed) {
                trimmed.to_owned()
            } else {
                default.to_owned()
            }
        }
        Err(_) => default.to_owned(),
    }
}

pub fn is_loopback_base_url(url: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_override_accepts_valid_loopbacks() {
        assert!(is_loopback_base_url("http://127.0.0.1:8080"));
        assert!(is_loopback_base_url("https://localhost:3000"));
        assert!(is_loopback_base_url("http://[::1]:9000"));
        assert!(is_loopback_base_url("http://127.0.0.2"));
    }

    #[test]
    fn loopback_override_rejects_userinfo_and_non_loopback() {
        assert!(!is_loopback_base_url("http://127.0.0.1@evil.test"));
        assert!(!is_loopback_base_url("https://example.com"));
        assert!(!is_loopback_base_url("ftp://127.0.0.1"));
        assert!(!is_loopback_base_url("http://192.168.1.1"));
    }
}
