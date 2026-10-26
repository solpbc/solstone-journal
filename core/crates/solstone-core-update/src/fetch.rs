// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

pub const UPDATE_ORIGIN: &str = "https://updates.solstone.app";

pub const PLATFORM_INSTALLER_URL: &str = "https://solstone.app/install.sh";
pub const PLATFORM_HOST: &str = "solstone.app";

pub const NATIVE_BOOTSTRAP_URL: &str = "https://updates.solstone.app/solstone-journal/install.sh";
pub const NATIVE_HOST: &str = "updates.solstone.app";

pub const MAX_REDIRECT_HOPS: u8 = 5;
pub const MAX_SCRIPT_SIZE: u64 = 1024 * 1024; // 1 MiB

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallerSource {
    Platform,
    Native,
}

impl InstallerSource {
    pub fn url(&self) -> &'static str {
        match self {
            Self::Platform => PLATFORM_INSTALLER_URL,
            Self::Native => NATIVE_BOOTSTRAP_URL,
        }
    }

    pub fn host(&self) -> &'static str {
        match self {
            Self::Platform => PLATFORM_HOST,
            Self::Native => NATIVE_HOST,
        }
    }
}

pub trait InstallerFetch: Send + Sync {
    fn fetch(&self, source: InstallerSource) -> std::io::Result<PathBuf>;
    fn cleanup(&self, _path: &std::path::Path) {}
}

pub struct RealInstallerFetch;

impl InstallerFetch for RealInstallerFetch {
    fn cleanup(&self, path: &std::path::Path) {
        let _ = fs::remove_file(path);
    }
    fn fetch(&self, source: InstallerSource) -> std::io::Result<PathBuf> {
        download_installer(source.url(), source.host())
    }
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

fn parse_absolute_url(url: &str) -> std::io::Result<AbsoluteUrl> {
    let url = url.split('#').next().unwrap_or_default();
    let (scheme, rest) = url.split_once("://").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "URL must be absolute")
    })?;

    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "URL scheme must be https",
        ));
    }

    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.contains('@') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "userinfo is not permitted in URL",
        ));
    }

    let host = if let Some((h, _port)) = authority.split_once(':') {
        h.to_ascii_lowercase()
    } else {
        authority.to_ascii_lowercase()
    };

    if host.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "URL host cannot be empty",
        ));
    }

    let path_and_query = match &rest[authority_end..] {
        "" => "/".to_owned(),
        query if query.starts_with('?') => format!("/{query}"),
        path => path.to_owned(),
    };

    Ok(AbsoluteUrl {
        scheme,
        authority: authority.to_owned(),
        host,
        path_and_query,
    })
}

fn resolve_redirect(
    current: &AbsoluteUrl,
    location: &str,
    expected_host: &str,
) -> std::io::Result<AbsoluteUrl> {
    let location = location.split('#').next().unwrap_or_default();
    if location.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "redirect Location header is empty",
        ));
    }

    let resolved = if location.starts_with("https://") {
        parse_absolute_url(location)?
    } else if location.starts_with("http://") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "redirect to insecure http is refused",
        ));
    } else if location.starts_with("//") {
        parse_absolute_url(&format!("https:{location}"))?
    } else if location.starts_with('/') {
        AbsoluteUrl {
            scheme: current.scheme.clone(),
            authority: current.authority.clone(),
            host: current.host.clone(),
            path_and_query: location.to_owned(),
        }
    } else {
        let base_path = current.path_and_query.split('?').next().unwrap_or("/");
        let dir = match base_path.rfind('/') {
            Some(idx) => &base_path[..=idx],
            None => "/",
        };
        AbsoluteUrl {
            scheme: current.scheme.clone(),
            authority: current.authority.clone(),
            host: current.host.clone(),
            path_and_query: format!("{dir}{location}"),
        }
    };

    if !resolved.host.eq_ignore_ascii_case(expected_host) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("redirect to different host '{}' refused", resolved.host),
        ));
    }

    Ok(resolved)
}

/// Follow at most [`MAX_REDIRECT_HOPS`] https redirects confined to
/// `expected_host`, and return the body of the final 200 response.
fn open_confined(initial_url: &str, expected_host: &str) -> std::io::Result<ureq::Body> {
    let mut current = parse_absolute_url(initial_url)?;
    if !current.host.eq_ignore_ascii_case(expected_host) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "initial host '{}' does not match expected '{}'",
                current.host, expected_host
            ),
        ));
    }

    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(Duration::from_secs(20)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .new_agent();

    let mut followed_hops = 0_u8;

    let response = loop {
        let resp = agent.get(current.as_str()).call().map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, e.to_string())
        })?;

        let status = resp.status().as_u16();
        if (300..400).contains(&status) {
            if followed_hops >= MAX_REDIRECT_HOPS {
                return Err(std::io::Error::other(
                    "download redirect hop limit exceeded",
                ));
            }

            let loc = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "missing Location header on redirect",
                    )
                })?;

            current = resolve_redirect(&current, loc, expected_host)?;
            followed_hops += 1;
            continue;
        }

        if status != 200 {
            return Err(std::io::Error::other(format!(
                "download failed with status {status}"
            )));
        }

        break resp;
    };
    Ok(response.into_body())
}

pub fn download_installer(initial_url: &str, expected_host: &str) -> std::io::Result<PathBuf> {
    let mut body = open_confined(initial_url, expected_host)?.into_reader();
    let mut file = tempfile::Builder::new()
        .prefix("solstone-installer-")
        .suffix(".sh")
        .tempfile()?;
    let mut buf = [0_u8; 32 * 1024];
    let mut total_read = 0_u64;
    loop {
        let n = body.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total_read += n as u64;
        if total_read > MAX_SCRIPT_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "installer script exceeds 1 MiB body cap",
            ));
        }
        file.write_all(&buf[..n])?;
    }
    file.as_file().sync_all()?;
    let (_file, path) = file.keep().map_err(std::io::Error::other)?;
    Ok(path)
}

/// Download a small document whole, refusing a body over `max_bytes`.
pub fn download_bytes(
    initial_url: &str,
    expected_host: &str,
    max_bytes: u64,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_confined(initial_url, expected_host)?
        .into_reader()
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("response exceeds {max_bytes} bytes"),
        ));
    }
    Ok(bytes)
}
