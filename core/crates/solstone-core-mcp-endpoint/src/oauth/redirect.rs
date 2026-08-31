// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Redirect-URI allowlist for journal-hosted MCP OAuth.

/// Scheme admitted for a parsed redirect URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedirectScheme {
    Http,
    Https,
}

/// Host admitted for a parsed redirect URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedirectHost {
    Localhost,
    V4Loopback,
    Claude,
}

/// A redirect URI that passed the closed allowlist parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedRedirectUri {
    pub(crate) scheme: RedirectScheme,
    pub(crate) host: RedirectHost,
    pub(crate) port: Option<u16>,
    pub(crate) path: String,
    pub(crate) query: Option<String>,
}

/// Why a redirect URI cannot be admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RedirectError;

/// Parse one redirect URI against the closed host/scheme grammar.
pub(crate) fn parse_redirect_uri(raw: &str) -> Result<ParsedRedirectUri, RedirectError> {
    if raw.is_empty() || !raw.bytes().all(is_allowed_redirect_byte) {
        return Err(RedirectError);
    }
    let (scheme, rest) = split_scheme(raw).ok_or(RedirectError)?;
    let (authority, path_query) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || authority.contains('@') {
        return Err(RedirectError);
    }
    let (host_text, port) = split_authority(authority).ok_or(RedirectError)?;
    let host = parse_host(host_text).ok_or(RedirectError)?;
    let (path, query) = split_path_query(path_query).ok_or(RedirectError)?;
    match host {
        RedirectHost::Claude => {
            if scheme != RedirectScheme::Https
                || port.is_some()
                || path != "/api/mcp/auth_callback"
                || query.is_some()
            {
                return Err(RedirectError);
            }
        }
        RedirectHost::Localhost | RedirectHost::V4Loopback => {}
    }
    Ok(ParsedRedirectUri {
        scheme,
        host,
        port,
        path,
        query,
    })
}

/// True when `presented` is allowlisted and matches one registered URI.
pub(crate) fn redirect_uri_is_allowed(presented: &str, registered: &[String]) -> bool {
    let Ok(presented) = parse_redirect_uri(presented) else {
        return false;
    };
    registered.iter().any(|candidate| {
        parse_redirect_uri(candidate)
            .is_ok_and(|registered| redirect_uris_match(&presented, &registered))
    })
}

fn redirect_uris_match(presented: &ParsedRedirectUri, registered: &ParsedRedirectUri) -> bool {
    if presented.scheme != registered.scheme
        || presented.host != registered.host
        || presented.path != registered.path
        || presented.query != registered.query
    {
        return false;
    }
    match presented.host {
        RedirectHost::Localhost | RedirectHost::V4Loopback => true,
        RedirectHost::Claude => presented.port == registered.port,
    }
}

fn is_allowed_redirect_byte(byte: u8) -> bool {
    byte.is_ascii_graphic() && !matches!(byte, b'%' | b'\\' | b'@' | b'#')
}

fn split_scheme(raw: &str) -> Option<(RedirectScheme, &str)> {
    let (scheme, rest) = raw.split_once("://")?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "http" => RedirectScheme::Http,
        "https" => RedirectScheme::Https,
        _ => return None,
    };
    Some((scheme, rest))
}

fn split_authority(authority: &str) -> Option<(&str, Option<u16>)> {
    if authority.starts_with('[') {
        return None;
    }
    match authority.rsplit_once(':') {
        Some((host, port))
            if port.bytes().all(|byte| byte.is_ascii_digit()) && !port.is_empty() =>
        {
            let port = port.parse::<u16>().ok()?;
            Some((host, Some(port)))
        }
        Some(_) => None,
        None => Some((authority, None)),
    }
}

fn parse_host(host: &str) -> Option<RedirectHost> {
    let host = host.to_ascii_lowercase();
    if host == "localhost" {
        Some(RedirectHost::Localhost)
    } else if host == "127.0.0.1" {
        Some(RedirectHost::V4Loopback)
    } else if host == "claude.ai" {
        Some(RedirectHost::Claude)
    } else {
        None
    }
}

fn split_path_query(path_query: &str) -> Option<(String, Option<String>)> {
    if !path_query.starts_with('/') {
        return None;
    }
    match path_query.split_once('?') {
        Some((path, query)) => Some((path.to_owned(), Some(query.to_owned()))),
        None => Some((path_query.to_owned(), None)),
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::{RedirectHost, RedirectScheme, parse_redirect_uri, redirect_uri_is_allowed};

    #[test]
    fn loopback_and_claude_shapes_parse() {
        let parsed = parse_redirect_uri("http://127.0.0.1:1234/callback/abc").unwrap();
        assert_eq!(parsed.scheme, RedirectScheme::Http);
        assert_eq!(parsed.host, RedirectHost::V4Loopback);
        assert_eq!(parsed.port, Some(1234));
        assert_eq!(parsed.path, "/callback/abc");

        let parsed = parse_redirect_uri("https://localhost/auth").unwrap();
        assert_eq!(parsed.host, RedirectHost::Localhost);
        assert_eq!(parsed.path, "/auth");

        let parsed = parse_redirect_uri("https://claude.ai/api/mcp/auth_callback").unwrap();
        assert_eq!(parsed.host, RedirectHost::Claude);
        assert_eq!(parsed.path, "/api/mcp/auth_callback");
        assert_eq!(parsed.port, None);
        assert_eq!(parsed.query, None);
    }

    #[test]
    fn parser_rejects_confused_and_non_allowlisted_hosts() {
        for raw in [
            "javascript:alert(1)",
            "data:text/html,x",
            "file:///tmp",
            "about:blank",
            "https://user@localhost/callback",
            "http://localhost/callback#frag",
            "http://localhost/%2ecallback",
            "http://localhost.example/callback",
            "https://evil-claude.ai/api/mcp/auth_callback",
            "https://claude.ai.evil/api/mcp/auth_callback",
            "http://127.0.0.1.nip.io/callback",
            "http://127.0.0.2/callback",
            "https://claude.ai/api/mcp/auth_callback/",
            "https://claude.ai/api/mcp/auth_callback?x=1",
            "http://claude.ai/api/mcp/auth_callback",
            "https://claude.ai:443/api/mcp/auth_callback",
            "http://[::1]/callback",
            "",
        ] {
            assert!(
                parse_redirect_uri(raw).is_err(),
                "expected reject for {raw:?}"
            );
        }
    }

    #[test]
    fn registered_match_ignores_loopback_port_and_requires_the_rest() {
        let registered = ["http://127.0.0.1/callback/abc".to_owned()];
        assert!(redirect_uri_is_allowed(
            "http://127.0.0.1:9999/callback/abc",
            &registered
        ));
        assert!(redirect_uri_is_allowed(
            "http://127.0.0.1/callback/abc",
            &registered
        ));
        assert!(!redirect_uri_is_allowed(
            "http://127.0.0.1:9999/callback/other",
            &registered
        ));
        assert!(!redirect_uri_is_allowed(
            "https://127.0.0.1/callback/abc",
            &registered
        ));
        assert!(!redirect_uri_is_allowed(
            "http://localhost/callback/abc",
            &registered
        ));
        assert!(!redirect_uri_is_allowed(
            "http://127.0.0.1/callback/abc?x=1",
            &registered
        ));
        assert!(!redirect_uri_is_allowed(
            "http://127.0.0.1/callback/abc",
            &[]
        ));
    }

    #[test]
    fn claude_registered_match_is_exact() {
        let registered = ["https://claude.ai/api/mcp/auth_callback".to_owned()];
        assert!(redirect_uri_is_allowed(
            "https://claude.ai/api/mcp/auth_callback",
            &registered
        ));
        assert!(!redirect_uri_is_allowed(
            "https://claude.ai/api/mcp/auth_callback/",
            &registered
        ));
        assert!(!redirect_uri_is_allowed(
            "http://127.0.0.1/callback",
            &registered
        ));
    }
}
