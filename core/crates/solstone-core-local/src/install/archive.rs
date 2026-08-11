// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use solstone_core_assets::Artifact;
use thiserror::Error;

const MAX_REDIRECT_HOPS: u8 = 4;

// CDN endpoints vary by owner region. Exact rules for the measured US hosts would
// reject valid regional delivery hosts, so these HTTPS suffix rules intentionally
// cover the CDN families on a label boundary.
const PRODUCTION_HOST_POLICY: HostPolicy = HostPolicy {
    rules: &[
        HostRule::Exact {
            scheme: UrlScheme::Https,
            host: "github.com",
        },
        HostRule::Exact {
            scheme: UrlScheme::Https,
            host: "huggingface.co",
        },
        HostRule::Exact {
            scheme: UrlScheme::Https,
            host: "updates.solstone.app",
        },
        HostRule::DotSuffix {
            scheme: UrlScheme::Https,
            suffix: ".githubusercontent.com",
        },
        HostRule::DotSuffix {
            scheme: UrlScheme::Https,
            suffix: ".hf.co",
        },
    ],
    max_hops: MAX_REDIRECT_HOPS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UrlScheme {
    Http,
    Https,
}

impl UrlScheme {
    fn parse(value: &str) -> Result<Self, ArchiveError> {
        match value.to_ascii_lowercase().as_str() {
            "http" => Ok(Self::Http),
            "https" => Ok(Self::Https),
            _ => Err(ArchiveError::InvalidUrl(value.to_owned())),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum HostRule {
    Exact {
        scheme: UrlScheme,
        host: &'static str,
    },
    DotSuffix {
        scheme: UrlScheme,
        suffix: &'static str,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct HostPolicy {
    pub rules: &'static [HostRule],
    pub max_hops: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedUrl {
    scheme: UrlScheme,
    authority: String,
    host: String,
    path_and_query: String,
}

impl ParsedUrl {
    fn as_url(&self) -> String {
        format!(
            "{}://{}{}",
            self.scheme.as_str(),
            self.authority,
            self.path_and_query
        )
    }

    fn path_without_query(&self) -> &str {
        self.path_and_query
            .split_once('?')
            .map_or(self.path_and_query.as_str(), |(path, _)| path)
    }
}

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("archive member escapes destination: {0}")]
    PathEscape(String),
    #[error("sha256 mismatch")]
    DigestMismatch,
    #[error("download size mismatch: expected {expected} bytes, received {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("download host refused: {host}")]
    HostRefused { host: String },
    #[error("download redirect limit exceeded")]
    RedirectLimit,
    #[error("download redirect missing location")]
    RedirectMissingLocation,
    #[error("download redirect has invalid location: {0}")]
    RedirectInvalidLocation(String),
    #[error("download redirect status is unsupported: {0}")]
    RedirectUnsupportedStatus(u16),
    #[error("download URL is invalid: {0}")]
    InvalidUrl(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("download failed: {0}")]
    Download(String),
}

pub fn verify_sha256(path: &Path, expected: &str) -> Result<String, ArchiveError> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut chunk = [0_u8; 1024 * 1024];
    loop {
        let size = file.read(&mut chunk)?;
        if size == 0 {
            break;
        }
        digest.update(&chunk[..size]);
    }
    let actual: String = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != expected {
        return Err(ArchiveError::DigestMismatch);
    }
    Ok(actual)
}

pub fn download_verified(
    artifact: &Artifact,
    destination: &Path,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<(), ArchiveError> {
    download_verified_with(
        artifact.upstream_url,
        artifact.sha256,
        artifact.size_bytes,
        destination,
        &PRODUCTION_HOST_POLICY,
        progress,
    )
}

pub(crate) fn download_verified_with(
    url: &str,
    expected_sha256: &str,
    expected_size_bytes: u64,
    destination: &Path,
    policy: &HostPolicy,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<(), ArchiveError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .build()
        .into();
    let mut current = parse_absolute_url(url)?;
    let mut redirects = 0_u8;
    let response = loop {
        check_host_policy(&current, policy)?;
        let response = agent
            .get(current.as_url())
            .call()
            .map_err(|error| ArchiveError::Download(error.to_string()))?;
        let status = response.status();
        if !status.is_redirection() {
            if !status.is_success() {
                return Err(ArchiveError::Download(format!(
                    "unexpected HTTP status {status}"
                )));
            }
            break response;
        }
        if !matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
            return Err(ArchiveError::RedirectUnsupportedStatus(status.as_u16()));
        }
        if redirects == policy.max_hops {
            return Err(ArchiveError::RedirectLimit);
        }
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.trim().is_empty())
            .ok_or(ArchiveError::RedirectMissingLocation)?;
        current = resolve_redirect_location(&current, location)?;
        redirects += 1;
    };
    let total = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    let parent = destination
        .parent()
        .ok_or_else(|| ArchiveError::Download("destination has no parent".to_owned()))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.part",
        destination.file_name().unwrap().to_string_lossy()
    ));
    let result = (|| -> Result<(), ArchiveError> {
        let mut out = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        let mut body = response.into_body().into_reader();
        let mut digest = Sha256::new();
        let mut received = 0_u64;
        let mut chunk = [0_u8; 64 * 1024];
        loop {
            let size = body.read(&mut chunk)?;
            if size == 0 {
                break;
            }
            received = received
                .checked_add(size as u64)
                .ok_or(ArchiveError::SizeMismatch {
                    expected: expected_size_bytes,
                    actual: u64::MAX,
                })?;
            if received > expected_size_bytes {
                return Err(ArchiveError::SizeMismatch {
                    expected: expected_size_bytes,
                    actual: received,
                });
            }
            out.write_all(&chunk[..size])?;
            digest.update(&chunk[..size]);
            progress(received, total);
        }
        out.sync_all()?;
        if received != expected_size_bytes {
            return Err(ArchiveError::SizeMismatch {
                expected: expected_size_bytes,
                actual: received,
            });
        }
        let actual: String = digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if actual != expected_sha256 {
            return Err(ArchiveError::DigestMismatch);
        }
        fs::rename(&temporary, destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn check_host_policy(url: &ParsedUrl, policy: &HostPolicy) -> Result<(), ArchiveError> {
    if policy.rules.iter().any(|rule| match rule {
        HostRule::Exact { scheme, host } => *scheme == url.scheme && *host == url.host,
        HostRule::DotSuffix { scheme, suffix } => {
            *scheme == url.scheme && url.host.len() > suffix.len() && url.host.ends_with(suffix)
        }
    }) {
        Ok(())
    } else {
        Err(ArchiveError::HostRefused {
            host: url.host.clone(),
        })
    }
}

fn parse_absolute_url(input: &str) -> Result<ParsedUrl, ArchiveError> {
    let without_fragment = input.split('#').next().unwrap_or_default();
    let (scheme, rest) = without_fragment
        .split_once("://")
        .ok_or_else(|| ArchiveError::InvalidUrl(input.to_owned()))?;
    let scheme = UrlScheme::parse(scheme)?;
    let boundary = rest
        .char_indices()
        .find_map(|(index, character)| matches!(character, '/' | '?').then_some(index))
        .unwrap_or(rest.len());
    let authority = &rest[..boundary];
    if authority.is_empty() || authority.contains('@') {
        return Err(ArchiveError::InvalidUrl(input.to_owned()));
    }
    let host = if let Some(after_bracket) = authority.strip_prefix('[') {
        match after_bracket.split_once(']') {
            Some((host, tail)) if tail.is_empty() || tail.starts_with(':') => host,
            _ => return Err(ArchiveError::InvalidUrl(input.to_owned())),
        }
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    if host.is_empty() {
        return Err(ArchiveError::InvalidUrl(input.to_owned()));
    }
    let suffix = &rest[boundary..];
    let path_and_query = if suffix.is_empty() {
        "/".to_owned()
    } else if suffix.starts_with('?') {
        format!("/{suffix}")
    } else {
        suffix.to_owned()
    };
    Ok(ParsedUrl {
        scheme,
        authority: authority.to_owned(),
        host: host.to_ascii_lowercase(),
        path_and_query,
    })
}

fn resolve_redirect_location(
    current: &ParsedUrl,
    location: &str,
) -> Result<ParsedUrl, ArchiveError> {
    let location = location.trim();
    if location.is_empty() {
        return Err(ArchiveError::RedirectMissingLocation);
    }
    let scheme_end = location.find("://");
    let first_separator = location.find(['/', '?', '#']);
    let is_absolute =
        scheme_end.is_some_and(|offset| first_separator.is_none_or(|first| offset < first));
    let resolved = if is_absolute {
        location.to_owned()
    } else if let Some(authority) = location.strip_prefix("//") {
        format!("{}://{authority}", current.scheme.as_str())
    } else if location.starts_with('/') {
        format!(
            "{}://{}{}",
            current.scheme.as_str(),
            current.authority,
            location
        )
    } else if location.starts_with('?') {
        format!(
            "{}://{}{}{}",
            current.scheme.as_str(),
            current.authority,
            current.path_without_query(),
            location
        )
    } else {
        let path = current.path_without_query();
        let directory_end = if path.ends_with('/') {
            path.len()
        } else {
            path.rfind('/').map_or(1, |index| index + 1)
        };
        format!(
            "{}://{}{}{}",
            current.scheme.as_str(),
            current.authority,
            &path[..directory_end],
            location
        )
    };
    parse_absolute_url(&resolved).map_err(|error| match error {
        ArchiveError::InvalidUrl(_) => ArchiveError::RedirectInvalidLocation(location.to_owned()),
        other => other,
    })
}

pub fn extract_tar_gz(archive: &Path, destination: &Path) -> Result<(), ArchiveError> {
    let mut tar = tar::Archive::new(GzDecoder::new(File::open(archive)?));
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if escapes_destination(&path) {
            return Err(ArchiveError::PathEscape(path.display().to_string()));
        }
        let output = destination.join(&path);
        if !output.starts_with(destination) {
            return Err(ArchiveError::PathEscape(path.display().to_string()));
        }
        if (entry.header().entry_type().is_symlink() || entry.header().entry_type().is_hard_link())
            && let Some(link) = entry.link_name()?
            && !link_stays_within(
                destination,
                if entry.header().entry_type().is_symlink() {
                    output.parent().unwrap_or(destination)
                } else {
                    destination
                },
                &link,
            )
        {
            return Err(ArchiveError::PathEscape(format!(
                "{} -> {}",
                path.display(),
                link.display()
            )));
        }
        entry.unpack(output)?;
    }
    Ok(())
}

fn escapes_destination(path: &Path) -> bool {
    path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn link_stays_within(root: &Path, base: &Path, link: &Path) -> bool {
    if link.is_absolute() {
        return false;
    }
    let Ok(relative_base) = base.strip_prefix(root) else {
        return false;
    };
    let mut components: Vec<_> = relative_base
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value),
            _ => None,
        })
        .collect();
    for component in link.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => components.push(value),
            Component::ParentDir => {
                if components.pop().is_none() {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

pub fn make_executable(path: &Path) -> Result<(), ArchiveError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

pub fn clear_macos_quarantine(path: &Path) -> Result<(), ArchiveError> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("xattr")
            .args(["-d", "com.apple.quarantine"])
            .arg(path)
            .status()?;
        if !status.success() {
            return Err(ArchiveError::Download("xattr failed".to_owned()));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
    }
    Ok(())
}

pub fn snapshot_tree(parent: &Path) -> Result<Vec<PathBuf>, ArchiveError> {
    fn visit(root: &Path, here: &Path, output: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in fs::read_dir(here)? {
            let entry = entry?;
            let path = entry.path();
            output.push(path.strip_prefix(root).expect("under root").to_path_buf());
            if entry.file_type()?.is_dir() {
                visit(root, &path, output)?;
            }
        }
        Ok(())
    }
    let mut output = Vec::new();
    if parent.exists() {
        visit(parent, parent, &mut output)?;
    }
    output.sort();
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::thread;

    use super::{
        ArchiveError, HostPolicy, HostRule, PRODUCTION_HOST_POLICY, UrlScheme, check_host_policy,
        download_verified_with, parse_absolute_url, resolve_redirect_location,
    };

    const LOOPBACK_RULES: [HostRule; 1] = [HostRule::Exact {
        scheme: UrlScheme::Http,
        host: "127.0.0.1",
    }];
    const LOOPBACK_POLICY: HostPolicy = HostPolicy {
        rules: &LOOPBACK_RULES,
        max_hops: 4,
    };

    fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("solstone-archive-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn serve_once(listener: TcpListener, response: Vec<u8>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(&response).unwrap();
        })
    }

    #[test]
    fn production_host_policy_is_scheme_qualified_and_case_insensitive() {
        for accepted in [
            "https://github.com/x",
            "HTTPS://UPDATES.SOLSTONE.APP/x",
            "https://US.AWS.CDN.HF.CO/x",
            "https://release-assets.githubusercontent.com/x",
        ] {
            assert!(
                check_host_policy(
                    &parse_absolute_url(accepted).unwrap(),
                    &PRODUCTION_HOST_POLICY
                )
                .is_ok()
            );
        }
        for refused in [
            "http://github.com/x",
            "https://updates.solstone.app@evil.example/x",
            "https://evil-hf.co/x",
        ] {
            assert!(
                parse_absolute_url(refused)
                    .and_then(|url| check_host_policy(&url, &PRODUCTION_HOST_POLICY))
                    .is_err()
            );
        }
    }

    #[test]
    fn redirect_location_resolution_supports_all_relative_forms() {
        let current =
            parse_absolute_url("https://huggingface.co/a/b/file.gguf?download=1").unwrap();
        for (location, expected) in [
            ("https://github.com/x", "https://github.com/x"),
            ("//cdn.hf.co/x", "https://cdn.hf.co/x"),
            ("/api/resolve", "https://huggingface.co/api/resolve"),
            (
                "/api/resolve-cache/x?redirect=https://cdn.example/y",
                "https://huggingface.co/api/resolve-cache/x?redirect=https://cdn.example/y",
            ),
            ("next.gguf", "https://huggingface.co/a/b/next.gguf"),
        ] {
            assert_eq!(
                resolve_redirect_location(&current, location)
                    .unwrap()
                    .as_url(),
                expected
            );
        }
    }

    #[test]
    fn redirect_to_an_unapproved_host_names_the_target() {
        let root = temp("redirect-refusal");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = serve_once(
            listener,
            b"HTTP/1.1 302 Found\r\nLocation: http://outside.example/file\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        );
        let error = download_verified_with(
            &format!("http://{address}/start"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
            5,
            &root.join("artifact"),
            &LOOPBACK_POLICY,
            |_received, _total| {},
        )
        .unwrap_err();
        server.join().unwrap();
        assert!(
            matches!(error, ArchiveError::HostRefused { ref host } if host == "outside.example")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn redirect_within_policy_completes_and_verifies() {
        let root = temp("redirect-success");
        let final_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let final_address = final_listener.local_addr().unwrap();
        let final_server = serve_once(
            final_listener,
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".to_vec(),
        );
        let redirect_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let redirect_address = redirect_listener.local_addr().unwrap();
        let redirect_server = serve_once(
            redirect_listener,
            format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{final_address}/file\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .into_bytes(),
        );
        let destination = root.join("artifact");
        download_verified_with(
            &format!("http://{redirect_address}/start"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
            5,
            &destination,
            &LOOPBACK_POLICY,
            |_received, _total| {},
        )
        .unwrap();
        redirect_server.join().unwrap();
        final_server.join().unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"hello");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn supported_redirect_statuses_have_302_parity() {
        for status in [301, 302, 303, 307, 308] {
            let root = temp(&format!("redirect-{status}"));
            let final_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let final_address = final_listener.local_addr().unwrap();
            let final_server = serve_once(
                final_listener,
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".to_vec(),
            );
            let redirect_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let redirect_address = redirect_listener.local_addr().unwrap();
            let redirect_server = serve_once(
                redirect_listener,
                format!(
                    "HTTP/1.1 {status} Redirect\r\nLocation: http://{final_address}/file\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .into_bytes(),
            );
            download_verified_with(
                &format!("http://{redirect_address}/start"),
                "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
                5,
                &root.join("artifact"),
                &LOOPBACK_POLICY,
                |_received, _total| {},
            )
            .unwrap();
            redirect_server.join().unwrap();
            final_server.join().unwrap();
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn redirect_limit_is_distinct_and_bounded() {
        let root = temp("redirect-limit");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 302 Found\r\nLocation: http://{address}/again\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .unwrap();
            }
        });
        assert!(matches!(
            download_verified_with(
                &format!("http://{address}/start"),
                "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
                5,
                &root.join("artifact"),
                &LOOPBACK_POLICY,
                |_received, _total| {},
            ),
            Err(ArchiveError::RedirectLimit)
        ));
        server.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn size_and_digest_failures_leave_no_destination_or_part() {
        for (name, expected_size, expected_digest, expected_error) in [
            (
                "size",
                4,
                "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
                false,
            ),
            ("digest", 5, "00", true),
        ] {
            let root = temp(name);
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = serve_once(
                listener,
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".to_vec(),
            );
            let destination = root.join("artifact");
            let error = download_verified_with(
                &format!("http://{address}/file"),
                expected_digest,
                expected_size,
                &destination,
                &LOOPBACK_POLICY,
                |_received, _total| {},
            )
            .unwrap_err();
            server.join().unwrap();
            assert_eq!(
                matches!(error, ArchiveError::DigestMismatch),
                expected_error
            );
            if !expected_error {
                assert!(matches!(error, ArchiveError::SizeMismatch { .. }));
            }
            assert!(!destination.exists());
            assert!(!root.join(".artifact.part").exists());
            let _ = fs::remove_dir_all(root);
        }
    }
}
