// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Explicit repository/credential decoding. Namespace identity follows the
//! native destination, never the repository's presentation or current secret.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv6Addr};
use std::path::{Component, Path};

pub use crate::teardown_recovery::retained::NativeDestination as Destination;

pub enum Repository<'a> {
    Local(&'a str),
    S3 {
        destination: Destination,
        access: &'a str,
        secret: &'a str,
        token: &'a str,
        append_only: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepositoryRefusal;

fn printable(value: &str, cap: usize, optional: bool) -> bool {
    value.len() <= cap
        && (optional || !value.is_empty())
        && value.bytes().all(|byte| (33..=126).contains(&byte))
}

fn endpoint(authority: &str) -> Result<String, RepositoryRefusal> {
    if authority.is_empty()
        || authority.contains(['/', '\\', '@', '?', '#', '%'])
        || !authority.is_ascii()
    {
        return Err(RepositoryRefusal);
    }
    let (host, port, ipv6) = if let Some(ip) = authority.strip_prefix('[') {
        let (ip, rest) = ip.split_once(']').ok_or(RepositoryRefusal)?;
        let host = ip.parse::<Ipv6Addr>().map_err(|_| RepositoryRefusal)?;
        let port = if rest.is_empty() {
            "443"
        } else {
            rest.strip_prefix(':').ok_or(RepositoryRefusal)?
        };
        (host.to_string(), port, true)
    } else {
        let (host, port) = authority.split_once(':').unwrap_or((authority, "443"));
        if host.is_empty()
            || host.len() > 253
            || !host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
        {
            return Err(RepositoryRefusal);
        }
        let host = host
            .parse::<IpAddr>()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| host.to_ascii_lowercase());
        (host, port, false)
    };
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(RepositoryRefusal);
    }
    let port = port.parse::<u16>().map_err(|_| RepositoryRefusal)?;
    if port == 0 {
        return Err(RepositoryRefusal);
    }
    Ok(if ipv6 {
        format!("https://[{host}]:{port}")
    } else {
        format!("https://{host}:{port}")
    })
}

fn decode_http_path(path: &str) -> Result<String, RepositoryRefusal> {
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            decoded.push(byte);
            continue;
        }
        let hi = (bytes.next().ok_or(RepositoryRefusal)? as char)
            .to_digit(16)
            .ok_or(RepositoryRefusal)?;
        let lo = (bytes.next().ok_or(RepositoryRefusal)? as char)
            .to_digit(16)
            .ok_or(RepositoryRefusal)?;
        let byte = ((hi << 4) | lo) as u8;
        // Refuse an encoded separator rather than allowing a URL library to
        // change bucket/prefix boundaries. Ordinary encoded Unicode is exact.
        if matches!(byte, b'/' | b'\\') {
            return Err(RepositoryRefusal);
        }
        decoded.push(byte);
    }
    String::from_utf8(decoded).map_err(|_| RepositoryRefusal)
}

fn destination(repository: &str, region: &str) -> Result<Destination, RepositoryRefusal> {
    let raw = repository.strip_prefix("s3:").ok_or(RepositoryRefusal)?;
    let (raw, url_form) = if let Some(raw) = raw.strip_prefix("https://") {
        (raw, true)
    } else if let Some(raw) = raw.strip_prefix("//") {
        (raw, false)
    } else {
        (raw, false)
    };
    if raw.contains(['?', '#', '\0', '\r', '\n', '\\']) {
        return Err(RepositoryRefusal);
    }
    let (authority, path) = raw.split_once('/').ok_or(RepositoryRefusal)?;
    let path = if url_form {
        decode_http_path(path)?
    } else {
        path.to_owned()
    };
    let (bucket, prefix) = path.split_once('/').ok_or(RepositoryRefusal)?;
    // restic cleans the prefix, so the operated `users/<account>/<instance>/`
    // names the same objects as its form without the trailing slash.
    let prefix = prefix.strip_suffix('/').unwrap_or(prefix);
    if !printable(region, 64, false)
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || !(3..=63).contains(&bucket.len())
        || bucket.starts_with("xn--")
        || ["--x-s3", "-s3alias", "--ol-s3", "--table-s3"]
            .iter()
            .any(|suffix| bucket.ends_with(suffix))
        || bucket.parse::<IpAddr>().is_ok()
        || bucket.split('.').any(|label| {
            label.is_empty()
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        || prefix.is_empty()
        || prefix.len() > 512
        || prefix.chars().any(|ch| ch < ' ' || ch == '\u{7f}')
        || prefix
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(RepositoryRefusal);
    }
    Ok(Destination {
        endpoint: endpoint(authority)?,
        region: region.to_owned(),
        bucket: bucket.to_owned(),
        prefix: prefix.to_owned(),
    })
}

pub fn parse<'a>(
    repository: &'a str,
    env: Option<&'a BTreeMap<String, Option<String>>>,
) -> Result<Repository<'a>, RepositoryRefusal> {
    if repository.is_empty() || repository.len() > 4096 {
        return Err(RepositoryRefusal);
    }
    if repository.starts_with('/') {
        let path = Path::new(repository);
        if env.is_some_and(|env| !env.is_empty())
            || repository.contains(['\0', '\r', '\n'])
            || path
                .components()
                .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        {
            return Err(RepositoryRefusal);
        }
        return Ok(Repository::Local(repository));
    }
    let env = env.ok_or(RepositoryRefusal)?;
    if env.keys().any(|key| {
        !matches!(
            key.as_str(),
            "AWS_ACCESS_KEY_ID"
                | "AWS_SECRET_ACCESS_KEY"
                | "AWS_SESSION_TOKEN"
                | "AWS_DEFAULT_REGION"
                | "SOLSTONE_APPEND_ONLY"
        )
    }) {
        return Err(RepositoryRefusal);
    }
    let get = |key: &str| {
        env.get(key)
            .and_then(Option::as_deref)
            .ok_or(RepositoryRefusal)
    };
    let access = get("AWS_ACCESS_KEY_ID")?;
    let secret = get("AWS_SECRET_ACCESS_KEY")?;
    let token = match env.get("AWS_SESSION_TOKEN") {
        None => "",
        Some(Some(token)) => token.as_str(),
        Some(None) => return Err(RepositoryRefusal),
    };
    if !printable(access, 256, false)
        || !printable(secret, 1024, false)
        || !printable(token, 16384, true)
    {
        return Err(RepositoryRefusal);
    }
    let append_only = match env.get("SOLSTONE_APPEND_ONLY") {
        None => false,
        Some(Some(value)) if value == "1" => true,
        Some(_) => return Err(RepositoryRefusal),
    };
    Ok(Repository::S3 {
        destination: destination(repository, get("AWS_DEFAULT_REGION")?)?,
        access,
        secret,
        token,
        append_only,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_credentials_are_borrowed_and_bounded() {
        let mut env = BTreeMap::from([
            ("AWS_ACCESS_KEY_ID".into(), Some("access".into())),
            ("AWS_SECRET_ACCESS_KEY".into(), Some("secret".into())),
            ("AWS_DEFAULT_REGION".into(), Some("r-1".into())),
        ]);
        let repository = "s3:host/owner-bucket/prefix";
        let Repository::S3 {
            access,
            secret,
            token,
            ..
        } = parse(repository, Some(&env)).unwrap()
        else {
            panic!("not S3");
        };
        assert_eq!(
            access.as_ptr(),
            env["AWS_ACCESS_KEY_ID"].as_ref().unwrap().as_ptr()
        );
        assert_eq!(
            secret.as_ptr(),
            env["AWS_SECRET_ACCESS_KEY"].as_ref().unwrap().as_ptr()
        );
        assert_eq!(token, "");
        for (key, cap) in [
            ("AWS_ACCESS_KEY_ID", 256),
            ("AWS_SECRET_ACCESS_KEY", 1024),
            ("AWS_SESSION_TOKEN", 16384),
            ("AWS_DEFAULT_REGION", 64),
        ] {
            let previous = env.insert(key.into(), Some("a".repeat(cap + 1)));
            assert!(parse(repository, Some(&env)).is_err());
            match previous {
                Some(value) => {
                    env.insert(key.into(), value);
                }
                None => {
                    env.remove(key);
                }
            }
        }
        assert!(parse(repository, None).is_err());
        env.insert("HOME".into(), Some("/ambient".into()));
        assert!(parse(repository, Some(&env)).is_err());
        assert!(parse("/owned/local", Some(&env)).is_err());
        assert!(matches!(
            parse("/owned/local", None),
            Ok(Repository::Local("/owned/local"))
        ));
    }

    #[test]
    fn equivalent_native_destinations_share_namespace() {
        for authority in [
            "STORAGE.EXAMPLE",
            "storage.example:443",
            "storage.example:00443",
        ] {
            for start in ["s3:", "s3://", "s3:https://"] {
                let d = destination(
                    &format!("{start}{authority}/owner-bucket/prefix"),
                    "test-region-1",
                )
                .unwrap();
                let expected =
                    destination("s3:storage.example/owner-bucket/prefix", "test-region-1").unwrap();
                assert_eq!(d, expected);
                assert_eq!(d.namespace(), expected.namespace());
            }
        }
        assert_eq!(
            destination("s3:[2001:0db8:0:0::1]/owner-bucket/prefix", "r-1").unwrap(),
            destination("s3:https://[2001:db8::1]:443/owner-bucket/prefix", "r-1").unwrap()
        );
        let d = destination("s3:storage.example/owner-bucket/prefix", "r-1").unwrap();
        assert_ne!(
            d.namespace(),
            destination("s3:storage.example/owner-bucket/other", "r-1")
                .unwrap()
                .namespace()
        );
        assert_ne!(
            d.namespace(),
            destination("s3:storage.example/other-bucket/prefix", "r-1")
                .unwrap()
                .namespace()
        );
    }

    #[test]
    fn prefix_bytes_are_exact_or_refused() {
        assert_eq!(
            destination("s3:https://host/owner-bucket/+percent%25/%E9%9B%AA", "r-1")
                .unwrap()
                .prefix,
            "+percent%/雪"
        );
        assert_eq!(
            destination("s3:host/owner-bucket/+percent%/雪", "r-1")
                .unwrap()
                .prefix,
            "+percent%/雪"
        );
        for repository in [
            "s3:http://host/owner-bucket/prefix",
            "s3:host/owner-bucket/../prefix",
            "s3:host/owner-bucket/a//b",
            "s3:https://host/owner-bucket/%2e%2e/prefix",
            "s3:https://host/owner-bucket/a%2fb",
            "s3:https://user:secret@host/owner-bucket/prefix",
            "s3:host:0/owner-bucket/prefix",
            "s3:host/owner-bucket/prefix?other",
            "s3:host/owner-bucket/",
            "s3:host/owner-bucket",
        ] {
            assert!(
                destination(repository, "r-1").is_err(),
                "accepted {repository}"
            );
        }
    }
}
