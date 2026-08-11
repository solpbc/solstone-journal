// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use solstone_core_assets::Artifact;
use thiserror::Error;

const MAX_REDIRECT_HOPS: u8 = 5;
const DOWNLOAD_ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "release-assets.githubusercontent.com",
    "updates.solstone.app",
    "huggingface.co",
    "us.aws.cdn.hf.co",
];

#[derive(Debug, Clone, Copy)]
pub(crate) struct DownloadHostPolicy<'a> {
    pub(crate) allowed_hosts: &'a [&'a str],
    pub(crate) allow_http: bool,
}

pub(crate) const PRODUCTION_DOWNLOAD_POLICY: DownloadHostPolicy<'static> = DownloadHostPolicy {
    allowed_hosts: DOWNLOAD_ALLOWED_HOSTS,
    allow_http: false,
};

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("archive member escapes destination: {0}")]
    PathEscape(String),
    #[error("sha256 mismatch")]
    DigestMismatch,
    #[error("download size mismatch: expected {expected} bytes, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("download host refused: {host}")]
    HostRefused { host: String },
    #[error("download scheme refused for {host}: {scheme}")]
    InsecureScheme { scheme: String, host: String },
    #[error("download redirect hop limit exceeded: {limit}")]
    RedirectHopLimitExceeded { limit: u8 },
    #[error("download URL authority must not include userinfo: {authority}")]
    UrlUserinfoRefused { authority: String },
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

pub(crate) fn download_verified(
    artifact: &Artifact,
    destination: &Path,
    policy: &DownloadHostPolicy<'_>,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<(), ArchiveError> {
    let mut current = validate_url(artifact.upstream_url, policy)?;
    let agent = ureq::agent();
    let mut followed = 0_u8;
    let response = loop {
        let response = agent
            .get(current.as_str())
            .config()
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|error| ArchiveError::Download(error.to_string()))?;
        if !response.status().is_redirection() {
            break response;
        }
        if followed == MAX_REDIRECT_HOPS {
            return Err(ArchiveError::RedirectHopLimitExceeded {
                limit: MAX_REDIRECT_HOPS,
            });
        }
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                ArchiveError::Download("redirect response has no Location header".to_owned())
            })?;
        let resolved = resolve_location(&current, location)?;
        current = validate_url(&resolved.as_str(), policy)?;
        followed += 1;
    };
    if !response.status().is_success() {
        return Err(ArchiveError::Download(format!(
            "unexpected HTTP status {}",
            response.status()
        )));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| ArchiveError::Download("destination has no parent".to_owned()))?;
    fs::create_dir_all(parent)?;
    let filename = destination
        .file_name()
        .ok_or_else(|| ArchiveError::Download("destination has no file name".to_owned()))?;
    let temporary = parent.join(format!(".{}.part", filename.to_string_lossy()));
    let result = (|| {
        let mut out = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        let mut body = response.into_body().into_reader();
        let mut received = 0_u64;
        let mut chunk = [0_u8; 64 * 1024];
        loop {
            let size = body.read(&mut chunk)?;
            if size == 0 {
                break;
            }
            out.write_all(&chunk[..size])?;
            received += size as u64;
            progress(received, Some(artifact.size_bytes));
        }
        out.sync_all()?;
        if received != artifact.size_bytes {
            return Err(ArchiveError::SizeMismatch {
                expected: artifact.size_bytes,
                actual: received,
            });
        }
        verify_sha256(&temporary, artifact.sha256)?;
        fs::rename(&temporary, destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[derive(Debug, Clone)]
struct AbsoluteUrl {
    scheme: String,
    authority: String,
    host: String,
    path_and_query: String,
}

impl AbsoluteUrl {
    fn as_str(&self) -> String {
        format!(
            "{}://{}{}",
            self.scheme, self.authority, self.path_and_query
        )
    }
}

fn validate_url(url: &str, policy: &DownloadHostPolicy<'_>) -> Result<AbsoluteUrl, ArchiveError> {
    let parsed = parse_absolute_url(url)?;
    if !policy
        .allowed_hosts
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&parsed.host))
    {
        return Err(ArchiveError::HostRefused {
            host: parsed.host.clone(),
        });
    }
    if parsed.scheme == "http" && !policy.allow_http {
        return Err(ArchiveError::InsecureScheme {
            scheme: parsed.scheme.clone(),
            host: parsed.host.clone(),
        });
    }
    Ok(parsed)
}

fn parse_absolute_url(url: &str) -> Result<AbsoluteUrl, ArchiveError> {
    let url = url.split('#').next().unwrap_or_default();
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| ArchiveError::Download("URL must be absolute http(s) URL".to_owned()))?;
    if !is_scheme(scheme) {
        return Err(ArchiveError::Download(
            "URL has malformed scheme".to_owned(),
        ));
    }
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(ArchiveError::Download(format!(
            "unsupported URL scheme: {scheme}"
        )));
    }
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.contains('@') {
        return Err(ArchiveError::UrlUserinfoRefused {
            authority: authority.to_owned(),
        });
    }
    let (host, authority) = parse_authority(authority)?;
    let path_and_query = match &rest[authority_end..] {
        "" => "/".to_owned(),
        query if query.starts_with('?') => format!("/{query}"),
        path => path.to_owned(),
    };
    Ok(AbsoluteUrl {
        scheme,
        authority,
        host,
        path_and_query,
    })
}

fn parse_authority(authority: &str) -> Result<(String, String), ArchiveError> {
    if authority.is_empty() || authority.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(ArchiveError::Download(
            "URL has malformed authority".to_owned(),
        ));
    }
    if let Some(bracketed) = authority.strip_prefix('[') {
        let Some((host, tail)) = bracketed.split_once(']') else {
            return Err(ArchiveError::Download(
                "URL has malformed bracketed IPv6 host".to_owned(),
            ));
        };
        let port = parse_port(tail)?;
        if host.is_empty() {
            return Err(ArchiveError::Download("URL has empty host".to_owned()));
        }
        let host = host.to_ascii_lowercase();
        let authority = match port {
            Some(port) => format!("[{host}]:{port}"),
            None => format!("[{host}]"),
        };
        return Ok((host, authority));
    }
    if authority.contains(['[', ']']) {
        return Err(ArchiveError::Download("URL has malformed host".to_owned()));
    }
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) if !port.contains(':') => (host, Some(parse_port_suffix(port)?)),
        Some(_) => return Err(ArchiveError::Download("URL has malformed host".to_owned())),
        None => (authority, None),
    };
    if host.is_empty() {
        return Err(ArchiveError::Download("URL has empty host".to_owned()));
    }
    let host = host.to_ascii_lowercase();
    let authority = port.map_or_else(|| host.clone(), |port| format!("{host}:{port}"));
    Ok((host, authority))
}

fn parse_port(tail: &str) -> Result<Option<u16>, ArchiveError> {
    if tail.is_empty() {
        return Ok(None);
    }
    let Some(port) = tail.strip_prefix(':') else {
        return Err(ArchiveError::Download(
            "URL has malformed bracketed IPv6 host".to_owned(),
        ));
    };
    Ok(Some(parse_port_suffix(port)?))
}

fn parse_port_suffix(port: &str) -> Result<u16, ArchiveError> {
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ArchiveError::Download("URL has malformed port".to_owned()));
    }
    port.parse()
        .map_err(|_| ArchiveError::Download("URL has malformed port".to_owned()))
}

fn resolve_location(current: &AbsoluteUrl, location: &str) -> Result<AbsoluteUrl, ArchiveError> {
    let location = location.split('#').next().unwrap_or_default();
    if location.is_empty() {
        return Err(ArchiveError::Download(
            "redirect Location is empty".to_owned(),
        ));
    }
    if has_scheme_prefix(location) {
        return parse_absolute_url(location);
    }
    if location.starts_with("//") {
        return parse_absolute_url(&format!("{}:{location}", current.scheme));
    }
    if looks_like_opaque_url(location) {
        return Err(ArchiveError::Download(
            "redirect Location has unsupported scheme".to_owned(),
        ));
    }
    let path_and_query =
        if location.starts_with('/') {
            location.to_owned()
        } else if location.starts_with('?') {
            let path = current.path_and_query.split('?').next().unwrap_or("/");
            format!("{path}{location}")
        } else {
            let (relative_path, query) = location
                .split_once('?')
                .map_or((location, None), |(path, query)| (path, Some(query)));
            let current_path = current.path_and_query.split('?').next().unwrap_or("/");
            let base = current_path.rsplit_once('/').map_or("/", |(parent, _)| {
                if parent.is_empty() { "/" } else { parent }
            });
            let joined = format!("{base}/{relative_path}");
            let path = normalize_path(&joined);
            query.map_or(path.clone(), |query| format!("{path}?{query}"))
        };
    parse_absolute_url(&format!(
        "{}://{}{}",
        current.scheme, current.authority, path_and_query
    ))
}

fn normalize_path(path: &str) -> String {
    let trailing_slash = path.ends_with('/');
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            value => components.push(value),
        }
    }
    let mut normalized = format!("/{}", components.join("/"));
    if trailing_slash && normalized != "/" {
        normalized.push('/');
    }
    normalized
}

fn has_scheme_prefix(value: &str) -> bool {
    value
        .split_once(':')
        .is_some_and(|(scheme, _)| is_scheme(scheme))
}

fn looks_like_opaque_url(value: &str) -> bool {
    value
        .split_once(':')
        .is_some_and(|(scheme, _)| is_scheme(scheme))
}

fn is_scheme(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic() || (index > 0 && matches!(byte, b'+' | b'-' | b'.'))
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
