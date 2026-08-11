// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use thiserror::Error;

use solstone_core_assets::Artifact;

#[derive(Clone, Debug)]
enum HostRule {
    Exact(String),
    Suffix(String),
}

#[derive(Clone, Debug)]
struct PolicyEntry {
    host: HostRule,
    schemes: Vec<String>,
}

/// Fixed network destinations for journal-downloadable artifacts.
///
/// Hugging Face redirected one observed request from `huggingface.co` to
/// `us.aws.cdn.hf.co`. That is one machine's regional evidence, so pinning the
/// observed host would reject legitimate regional CDN names. The narrower
/// `*.cdn.hf.co` suffix admits that CDN zone without trusting the much broader
/// `*.hf.co` namespace. GitHub's observed release target is the exact
/// `release-assets.githubusercontent.com`; a `*.githubusercontent.com` suffix
/// would unnecessarily admit unrelated user-content endpoints.
#[derive(Clone, Debug)]
pub(crate) struct DownloadPolicy {
    entries: Vec<PolicyEntry>,
    max_redirects: u8,
}

impl DownloadPolicy {
    pub(crate) fn production() -> Self {
        Self {
            entries: vec![
                PolicyEntry {
                    host: HostRule::Exact("github.com".to_owned()),
                    schemes: vec!["https".to_owned()],
                },
                PolicyEntry {
                    host: HostRule::Exact("release-assets.githubusercontent.com".to_owned()),
                    schemes: vec!["https".to_owned()],
                },
                PolicyEntry {
                    host: HostRule::Exact("huggingface.co".to_owned()),
                    schemes: vec!["https".to_owned()],
                },
                PolicyEntry {
                    host: HostRule::Suffix("cdn.hf.co".to_owned()),
                    schemes: vec!["https".to_owned()],
                },
                PolicyEntry {
                    host: HostRule::Exact("updates.solstone.app".to_owned()),
                    schemes: vec!["https".to_owned()],
                },
            ],
            max_redirects: 5,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(hosts: &[&str], schemes: &[&str], max_redirects: u8) -> Self {
        Self {
            entries: hosts
                .iter()
                .map(|host| PolicyEntry {
                    host: HostRule::Exact((*host).to_owned()),
                    schemes: schemes.iter().map(|scheme| (*scheme).to_owned()).collect(),
                })
                .collect(),
            max_redirects,
        }
    }

    #[cfg(test)]
    pub(crate) fn permits(&self, url: &str) -> Result<(), ArchiveError> {
        self.check(&parse_absolute_url(url)?)
    }

    fn check(&self, url: &AbsoluteUrl) -> Result<(), ArchiveError> {
        if url.has_userinfo {
            return Err(ArchiveError::HostRefused {
                host: url.host.clone(),
            });
        }
        let allowed = self.entries.iter().any(|entry| {
            entry
                .schemes
                .iter()
                .any(|scheme| scheme.eq_ignore_ascii_case(&url.scheme))
                && match &entry.host {
                    HostRule::Exact(host) => host.eq_ignore_ascii_case(&url.host),
                    HostRule::Suffix(suffix) => {
                        let host = url.host.to_ascii_lowercase();
                        let suffix = suffix.to_ascii_lowercase();
                        host.len() > suffix.len()
                            && host.ends_with(&suffix)
                            && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
                    }
                }
        });
        if allowed {
            Ok(())
        } else {
            Err(ArchiveError::HostRefused {
                host: url.host.clone(),
            })
        }
    }
}

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("archive member escapes destination: {0}")]
    PathEscape(String),
    #[error("sha256 mismatch")]
    DigestMismatch,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("download failed: {0}")]
    Download(String),
    #[error("download host refused: {host}")]
    HostRefused { host: String },
    #[error("download redirect limit exceeded: {limit}")]
    RedirectLimitExceeded { limit: u8 },
    #[error("download size mismatch: expected {expected} bytes, received {received} bytes")]
    SizeMismatch { expected: u64, received: u64 },
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
    policy: &DownloadPolicy,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<(), ArchiveError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .build()
        .into();
    let mut current = parse_absolute_url(artifact.upstream_url)?;
    let mut redirects = 0_u8;
    let response = loop {
        policy.check(&current)?;
        let response = fetch_url(&agent, &current.as_url())?;
        if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            break response;
        }
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ArchiveError::Download("redirect response missing Location".to_owned())
            })?;
        let next = resolve_location(&current, location)?;
        policy.check(&next)?;
        if redirects >= policy.max_redirects {
            return Err(ArchiveError::RedirectLimitExceeded {
                limit: policy.max_redirects,
            });
        }
        redirects += 1;
        current = next;
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
        out.write_all(&chunk[..size])?;
        digest.update(&chunk[..size]);
        received += size as u64;
        progress(received, total);
    }
    out.sync_all()?;
    let actual: String = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if received != artifact.size_bytes {
        let _ = fs::remove_file(&temporary);
        return Err(ArchiveError::SizeMismatch {
            expected: artifact.size_bytes,
            received,
        });
    }
    if actual != artifact.sha256 {
        let _ = fs::remove_file(&temporary);
        return Err(ArchiveError::DigestMismatch);
    }
    fs::rename(temporary, destination)?;
    Ok(())
}

fn fetch_url(
    agent: &ureq::Agent,
    url: &str,
) -> Result<ureq::http::Response<ureq::Body>, ArchiveError> {
    agent
        .get(url)
        .call()
        .map_err(|error| ArchiveError::Download(error.to_string()))
}

#[derive(Clone, Debug)]
struct AbsoluteUrl {
    scheme: String,
    authority: String,
    host: String,
    path: String,
    query: Option<String>,
    has_userinfo: bool,
}

impl AbsoluteUrl {
    fn as_url(&self) -> String {
        let mut output = format!("{}://{}{}", self.scheme, self.authority, self.path);
        if let Some(query) = &self.query {
            output.push('?');
            output.push_str(query);
        }
        output
    }

    fn origin(&self) -> String {
        format!("{}://{}", self.scheme, self.authority)
    }
}

fn parse_absolute_url(value: &str) -> Result<AbsoluteUrl, ArchiveError> {
    let value = value.split('#').next().unwrap_or_default();
    let Some((scheme, rest)) = value.split_once(':') else {
        return Err(ArchiveError::Download("URL has no scheme".to_owned()));
    };
    if scheme.is_empty()
        || !scheme.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic() || (index > 0 && matches!(byte, b'+' | b'-' | b'.'))
        })
    {
        return Err(ArchiveError::Download("URL has invalid scheme".to_owned()));
    }
    let Some(rest) = rest.strip_prefix("//") else {
        return Err(ArchiveError::Download("URL has no authority".to_owned()));
    };
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return Err(ArchiveError::Download("URL has empty authority".to_owned()));
    }
    let remainder = &rest[authority_end..];
    let (path, query) = split_path_query(remainder);
    let (has_userinfo, host_port) = match authority.rsplit_once('@') {
        Some((_userinfo, host_port)) => (true, host_port),
        None => (false, authority),
    };
    let host = parse_host(host_port)?;
    Ok(AbsoluteUrl {
        scheme: scheme.to_owned(),
        authority: authority.to_owned(),
        host,
        path,
        query,
        has_userinfo,
    })
}

fn parse_host(host_port: &str) -> Result<String, ArchiveError> {
    if let Some(after_open) = host_port.strip_prefix('[') {
        let Some((host, tail)) = after_open.split_once(']') else {
            return Err(ArchiveError::Download(
                "URL has invalid IPv6 host".to_owned(),
            ));
        };
        if host.is_empty()
            || (!tail.is_empty() && (!tail.starts_with(':') || !valid_port(&tail[1..])))
        {
            return Err(ArchiveError::Download(
                "URL has invalid authority".to_owned(),
            ));
        }
        return Ok(host.to_owned());
    }
    let (host, port) = match host_port.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (host_port, None),
    };
    if host.is_empty()
        || host_port.matches(':').count() > 1
        || host.bytes().any(|byte| byte.is_ascii_whitespace())
        || port.is_some_and(|port| !valid_port(port))
    {
        return Err(ArchiveError::Download("URL has invalid host".to_owned()));
    }
    Ok(host.to_owned())
}

fn valid_port(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u16>().is_ok()
}

fn split_path_query(value: &str) -> (String, Option<String>) {
    let (path, query) = match value.split_once('?') {
        Some((path, query)) => (path, Some(query.to_owned())),
        None => (value, None),
    };
    (
        if path.is_empty() {
            "/".to_owned()
        } else {
            path.to_owned()
        },
        query,
    )
}

fn resolve_location(current: &AbsoluteUrl, location: &str) -> Result<AbsoluteUrl, ArchiveError> {
    let location = location.split('#').next().unwrap_or_default();
    if location.is_empty() {
        return Err(ArchiveError::Download(
            "redirect Location is empty".to_owned(),
        ));
    }
    if has_absolute_scheme(location) {
        return parse_absolute_url(location);
    }
    if let Some(authority) = location.strip_prefix("//") {
        return parse_absolute_url(&format!("{}://{authority}", current.scheme));
    }
    if location.starts_with('/') {
        return parse_absolute_url(&format!("{}{}", current.origin(), location));
    }
    if location.starts_with('?') {
        return parse_absolute_url(&format!("{}{}{}", current.origin(), current.path, location));
    }
    let (relative_path, query) = split_path_query(location);
    let directory = current
        .path
        .rsplit_once('/')
        .map(|(directory, _)| format!("{directory}/"))
        .unwrap_or_else(|| "/".to_owned());
    let path = normalize_path(&format!("{directory}{relative_path}"));
    let mut resolved = format!("{}{}", current.origin(), path);
    if let Some(query) = query {
        resolved.push('?');
        resolved.push_str(&query);
    }
    parse_absolute_url(&resolved)
}

fn has_absolute_scheme(value: &str) -> bool {
    value
        .find(':')
        .is_some_and(|at| at > 0 && !value[..at].contains(['/', '?', '#']))
}

fn normalize_path(path: &str) -> String {
    let mut output = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                output.pop();
            }
            component => output.push(component),
        }
    }
    let mut normalized = format!("/{}", output.join("/"));
    if path.ends_with('/') && !normalized.ends_with('/') {
        normalized.push('/');
    }
    normalized
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
