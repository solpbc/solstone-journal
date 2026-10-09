// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Verified artifact download primitives shared by native installers.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};
use solstone_core_assets::RuntimeFetchHandle;
use thiserror::Error;

const MAX_REDIRECT_HOPS: u8 = 5;
const OWNER_ORIGIN_HOST: &str = "updates.solstone.app";

const BUILDER_INPUT_ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "codeload.github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "github-releases.githubusercontent.com",
    "ziglang.org",
    "cmake.org",
    "static.rust-lang.org",
    "www.python.org",
    "files.pythonhosted.org",
    // Controlled Windows FFmpeg builder inputs. These remain builder-only;
    // every archive is byte-pinned by the distribution input table.
    "repo.msys2.org",
    "www.nasm.us",
];

#[derive(Debug, Clone, Copy)]
struct DownloadHostPolicy<'a> {
    allowed_hosts: &'a [&'a str],
    allow_http: bool,
}

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("archive member escapes destination: {0}")]
    PathEscape(String),
    #[error("sha256 mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },
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
    #[error("download origin unavailable at {host}: {message}")]
    OriginUnavailable { host: String, message: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("download failed: {0}")]
    Download(String),
}

#[cfg(feature = "runtime-fetch-test")]
pub trait FakeRuntimeFetch: 'static {
    fn fetch(&self, url: &str, sha256: &str, size_bytes: u64) -> Result<Vec<u8>, ArchiveError>;
}

#[cfg(feature = "runtime-fetch-test")]
#[derive(Debug, Clone)]
pub struct RuntimeFetchLoopback {
    pub base_url: String,
    pub allowed_hosts: Vec<String>,
    pub allow_http: bool,
}

#[cfg(feature = "runtime-fetch-test")]
thread_local! {
    static FAKE_RUNTIME_FETCH: std::cell::RefCell<Option<std::rc::Rc<dyn FakeRuntimeFetch>>> =
        const { std::cell::RefCell::new(None) };
    static RUNTIME_FETCH_LOOPBACK: std::cell::RefCell<Option<RuntimeFetchLoopback>> =
        const { std::cell::RefCell::new(None) };
    static BUILDER_FETCH_LOOPBACK: std::cell::RefCell<Option<(Vec<String>, bool)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(feature = "runtime-fetch-test")]
pub fn with_fake_runtime_fetch<F: FakeRuntimeFetch, T>(
    fake: &std::rc::Rc<F>,
    f: impl FnOnce() -> T,
) -> T {
    struct Guard(Option<std::rc::Rc<dyn FakeRuntimeFetch>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            FAKE_RUNTIME_FETCH.with(|cell| *cell.borrow_mut() = self.0.take());
        }
    }
    let prev = FAKE_RUNTIME_FETCH.with(|cell| cell.borrow_mut().replace(fake.clone()));
    let _guard = Guard(prev);
    f()
}

#[cfg(feature = "runtime-fetch-test")]
pub fn with_runtime_fetch_loopback<T>(loopback: RuntimeFetchLoopback, f: impl FnOnce() -> T) -> T {
    struct Guard(Option<RuntimeFetchLoopback>);
    impl Drop for Guard {
        fn drop(&mut self) {
            RUNTIME_FETCH_LOOPBACK.with(|cell| *cell.borrow_mut() = self.0.take());
        }
    }
    let prev = RUNTIME_FETCH_LOOPBACK.with(|cell| cell.borrow_mut().replace(loopback));
    let _guard = Guard(prev);
    f()
}

#[cfg(feature = "runtime-fetch-test")]
pub fn with_builder_fetch_loopback<T>(
    extra_hosts: &[&str],
    allow_http: bool,
    f: impl FnOnce() -> T,
) -> T {
    struct Guard(Option<(Vec<String>, bool)>);
    impl Drop for Guard {
        fn drop(&mut self) {
            BUILDER_FETCH_LOOPBACK.with(|cell| *cell.borrow_mut() = self.0.take());
        }
    }
    let hosts = extra_hosts.iter().map(|s| (*s).to_owned()).collect();
    let prev = BUILDER_FETCH_LOOPBACK.with(|cell| cell.borrow_mut().replace((hosts, allow_http)));
    let _guard = Guard(prev);
    f()
}

pub fn verify_sha256(path: &Path, expected: &str) -> Result<String, ArchiveError> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut chunk = vec![0_u8; 1024 * 1024];
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
        return Err(ArchiveError::DigestMismatch {
            expected: expected.to_owned(),
            actual,
        });
    }
    Ok(actual)
}

/// Download an owner runtime artifact verified by a [`RuntimeFetchHandle`].
///
/// Skips network when `destination` already exists and matches SHA-256 (returning `Ok(false)`).
/// Otherwise writes `destination` atomically via a temporary file and returns `Ok(true)`.
pub fn download_runtime_fetch(
    handle: &RuntimeFetchHandle,
    destination: &Path,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<bool, ArchiveError> {
    if destination.is_file() {
        match verify_sha256(destination, handle.sha256()) {
            Ok(_) => return Ok(false),
            Err(ArchiveError::DigestMismatch { .. }) => {}
            Err(error) => return Err(error),
        }
    }

    #[cfg(feature = "runtime-fetch-test")]
    {
        let fake_opt = FAKE_RUNTIME_FETCH.with(|cell| cell.borrow().clone());
        if let Some(fake) = fake_opt {
            let url = format!("https://{OWNER_ORIGIN_HOST}/{}", handle.origin_key());
            let bytes = fake.fetch(&url, handle.sha256(), handle.size_bytes())?;
            write_and_verify_bytes(&bytes, handle.sha256(), handle.size_bytes(), destination)?;
            return Ok(true);
        }
    }

    let origin_key = handle.origin_key();
    let expected_size = handle.size_bytes();
    let sha256 = handle.sha256();

    #[cfg(feature = "runtime-fetch-test")]
    let (url, loopback_hosts, allow_http) = {
        let loopback_opt = RUNTIME_FETCH_LOOPBACK.with(|cell| cell.borrow().clone());
        if let Some(loopback) = loopback_opt {
            let url = format!("{}/{}", loopback.base_url.trim_end_matches('/'), origin_key);
            (url, loopback.allowed_hosts, loopback.allow_http)
        } else {
            let url = format!("https://{OWNER_ORIGIN_HOST}/{origin_key}");
            (url, vec![OWNER_ORIGIN_HOST.to_string()], false)
        }
    };
    #[cfg(feature = "runtime-fetch-test")]
    let host_refs: Vec<&str> = loopback_hosts.iter().map(|s| s.as_str()).collect();
    #[cfg(feature = "runtime-fetch-test")]
    let policy = DownloadHostPolicy {
        allowed_hosts: &host_refs,
        allow_http,
    };

    #[cfg(not(feature = "runtime-fetch-test"))]
    let (url, policy) = {
        let url = format!("https://{OWNER_ORIGIN_HOST}/{origin_key}");
        let policy = DownloadHostPolicy {
            allowed_hosts: &[OWNER_ORIGIN_HOST],
            allow_http: false,
        };
        (url, policy)
    };

    download_verified_url_internal(
        &url,
        sha256,
        Some(expected_size),
        destination,
        &policy,
        &mut progress,
    )?;
    Ok(true)
}

#[cfg(feature = "runtime-fetch-test")]
fn write_and_verify_bytes(
    bytes: &[u8],
    expected_sha256: &str,
    expected_size: u64,
    destination: &Path,
) -> Result<(), ArchiveError> {
    if bytes.len() as u64 != expected_size {
        return Err(ArchiveError::SizeMismatch {
            expected: expected_size,
            actual: bytes.len() as u64,
        });
    }
    let actual_sha = format!("{:x}", Sha256::digest(bytes));
    if actual_sha != expected_sha256 {
        return Err(ArchiveError::DigestMismatch {
            expected: expected_sha256.to_owned(),
            actual: actual_sha,
        });
    }
    let parent = destination
        .parent()
        .ok_or_else(|| ArchiveError::Download("destination has no parent".to_owned()))?;
    fs::create_dir_all(parent)?;
    let filename = destination
        .file_name()
        .ok_or_else(|| ArchiveError::Download("destination has no file name".to_owned()))?;
    let temporary = parent.join(format!(".{}.part", filename.to_string_lossy()));
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, destination)?;
    Ok(())
}

/// Fetch builder inputs from allowed upstream hosts.
///
/// Refuses `OWNER_ORIGIN_HOST` unconditionally before any network attempt or fake consultation.
pub fn ensure_verified_url(
    url: &str,
    sha256: &str,
    expected_size: Option<u64>,
    destination: &Path,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<bool, ArchiveError> {
    let parsed = parse_absolute_url(url)?;
    if parsed.host.eq_ignore_ascii_case(OWNER_ORIGIN_HOST) {
        return Err(ArchiveError::HostRefused { host: parsed.host });
    }

    if destination.is_file() {
        match verify_sha256(destination, sha256) {
            Ok(_) => return Ok(false),
            Err(ArchiveError::DigestMismatch { .. }) => {}
            Err(error) => return Err(error),
        }
    }

    #[cfg(feature = "runtime-fetch-test")]
    let (extra_hosts, allow_http) = BUILDER_FETCH_LOOPBACK
        .with(|cell| cell.borrow().clone())
        .unwrap_or_default();
    #[cfg(feature = "runtime-fetch-test")]
    let mut allowed_strings: Vec<String> = BUILDER_INPUT_ALLOWED_HOSTS
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    #[cfg(feature = "runtime-fetch-test")]
    for h in extra_hosts {
        if !h.eq_ignore_ascii_case(OWNER_ORIGIN_HOST) {
            allowed_strings.push(h);
        }
    }
    #[cfg(feature = "runtime-fetch-test")]
    let host_refs: Vec<&str> = allowed_strings.iter().map(|s| s.as_str()).collect();
    #[cfg(feature = "runtime-fetch-test")]
    let policy = DownloadHostPolicy {
        allowed_hosts: &host_refs,
        allow_http,
    };

    #[cfg(not(feature = "runtime-fetch-test"))]
    let policy = DownloadHostPolicy {
        allowed_hosts: BUILDER_INPUT_ALLOWED_HOSTS,
        allow_http: false,
    };

    download_verified_url_internal(
        url,
        sha256,
        expected_size,
        destination,
        &policy,
        &mut progress,
    )?;
    Ok(true)
}

fn download_verified_url_internal(
    url: &str,
    sha256: &str,
    expected_size: Option<u64>,
    destination: &Path,
    policy: &DownloadHostPolicy<'_>,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(), ArchiveError> {
    let mut current = validate_url(url, policy)?;
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
            .map_err(|error| ArchiveError::OriginUnavailable {
                host: current.host.clone(),
                message: error.to_string(),
            })?;
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
        return Err(ArchiveError::OriginUnavailable {
            host: current.host.clone(),
            message: format!("unexpected HTTP status {}", response.status()),
        });
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
            progress(received, expected_size);
        }
        out.sync_all()?;
        if let Some(expected) = expected_size
            && received != expected
        {
            return Err(ArchiveError::SizeMismatch {
                expected,
                actual: received,
            });
        }
        verify_sha256(&temporary, sha256)?;
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
            let path = normalize_path(&format!("{base}/{relative_path}"));
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

fn is_scheme(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic() || (index > 0 && matches!(byte, b'+' | b'-' | b'.'))
        })
}

#[cfg(unix)]
pub fn make_executable(path: &Path) -> Result<(), ArchiveError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path)?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
pub fn make_executable(_path: &Path) -> Result<(), ArchiveError> {
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn clear_macos_quarantine(path: &Path) -> Result<(), ArchiveError> {
    use std::process::Command;
    let status = Command::new("xattr")
        .arg("-d")
        .arg("com.apple.quarantine")
        .arg(path)
        .status();
    if let Ok(status) = status
        && !status.success()
    {
        let _ = status;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn clear_macos_quarantine(_path: &Path) -> Result<(), ArchiveError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn sha256_mismatch_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("asset");
        File::create(&path).unwrap().write_all(b"asset").unwrap();
        let error = verify_sha256(&path, "00").expect_err("must refuse wrong digest");
        let ArchiveError::DigestMismatch { expected, actual } = error else {
            panic!("expected digest mismatch");
        };
        assert_eq!(expected, "00");
        assert_eq!(
            actual,
            "d59386e0ae435e292fbe0ebcdb954b75ed5fb3922091277cb19f798fc5d50718"
        );
    }

    #[test]
    fn digest_verification_fits_a_one_mib_stack() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("asset");
        File::create(&path)
            .unwrap()
            .write_all(&vec![0x5a_u8; 3 * 1024 * 1024])
            .unwrap();
        let expected = {
            let mut digest = Sha256::new();
            digest.update(vec![0x5a_u8; 3 * 1024 * 1024]);
            digest
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let probe = path.clone();
        let reference = expected.clone();
        let actual = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(move || verify_sha256(&probe, &reference).expect("hash the asset"))
            .expect("spawn worker with a Windows-sized stack")
            .join()
            .expect("hashing worker finished");
        assert_eq!(actual, expected);
    }

    #[test]
    fn allowed_host_comparison_is_case_insensitive_without_a_network_request() {
        let policy = DownloadHostPolicy {
            allowed_hosts: &["MiXeD.ExAmPlE"],
            allow_http: false,
        };
        assert_eq!(
            validate_url("https://mixed.example/asset", &policy)
                .unwrap()
                .host,
            "mixed.example"
        );
    }

    #[test]
    fn builder_input_policy_refuses_the_owner_origin() {
        let error = ensure_verified_url(
            "https://updates.solstone.app/assets/tool",
            "00",
            Some(10),
            Path::new("/nonexistent"),
            |_, _| {},
        )
        .expect_err("builder-input policy must not admit the owner origin");
        assert!(matches!(
            error,
            ArchiveError::HostRefused { host } if host == "updates.solstone.app"
        ));
    }

    #[test]
    fn builder_input_policy_admits_each_pinned_upstream() {
        let policy = DownloadHostPolicy {
            allowed_hosts: BUILDER_INPUT_ALLOWED_HOSTS,
            allow_http: false,
        };
        for url in [
            "https://github.com/FFmpeg/FFmpeg/archive/deadbeef.tar.gz",
            "https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz",
            "https://cmake.org/files/v3.31/cmake-3.31.12-windows-x86_64.zip",
            "https://static.rust-lang.org/dist/rust-std.tar.xz",
            "https://www.python.org/ftp/python/3.12.10/python-3.12.10-embed-amd64.zip",
            "https://files.pythonhosted.org/packages/onnxruntime.whl",
        ] {
            validate_url(url, &policy).unwrap_or_else(|error| panic!("{url}: {error}"));
        }
    }

    #[test]
    fn ensure_verified_url_skips_a_matching_cache() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cached");
        File::create(&path).unwrap().write_all(b"cached").unwrap();
        let digest: String = Sha256::digest(b"cached")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let fetched = ensure_verified_url(
            "https://github.com/example/missing",
            &digest,
            Some(6),
            &path,
            |_, _| {},
        )
        .expect("matching cache must not fetch");
        assert!(!fetched);
    }

    #[cfg(feature = "runtime-fetch-test")]
    #[test]
    fn fake_runtime_fetch_records_url_digest_and_size() {
        use solstone_core_assets::runtime_fetch_handle_fixture;
        use std::cell::RefCell;

        struct TestFake {
            calls: RefCell<Vec<(String, String, u64)>>,
            body: Vec<u8>,
        }
        impl FakeRuntimeFetch for TestFake {
            fn fetch(
                &self,
                url: &str,
                sha256: &str,
                size_bytes: u64,
            ) -> Result<Vec<u8>, ArchiveError> {
                self.calls
                    .borrow_mut()
                    .push((url.to_owned(), sha256.to_owned(), size_bytes));
                Ok(self.body.clone())
            }
        }

        let body = b"valid payload".to_vec();
        let sha256: &'static str =
            Box::leak(format!("{:x}", Sha256::digest(&body)).into_boxed_str());
        let size = body.len() as u64;

        let fake = std::rc::Rc::new(TestFake {
            calls: RefCell::new(Vec::new()),
            body: body.clone(),
        });

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("target");
        let handle = runtime_fetch_handle_fixture("test-unit", "assets/test-key", sha256, size);

        with_fake_runtime_fetch(&fake, || {
            let fetched = download_runtime_fetch(&handle, &path, |_, _| {}).unwrap();
            assert!(fetched);
            assert_eq!(fs::read(&path).unwrap(), body);
        });

        assert_eq!(fake.calls.borrow().len(), 1);
        let (url, recorded_sha, recorded_size) = &fake.calls.borrow()[0];
        assert_eq!(url, "https://updates.solstone.app/assets/test-key");
        assert_eq!(recorded_sha, &sha256);
        assert_eq!(*recorded_size, size);

        // Wrong bytes -> digest mismatch
        let bad_fake = std::rc::Rc::new(TestFake {
            calls: RefCell::new(Vec::new()),
            body: b"wrong payload".to_vec(),
        });
        let bad_path = directory.path().join("bad_target");
        with_fake_runtime_fetch(&bad_fake, || {
            let err = download_runtime_fetch(&handle, &bad_path, |_, _| {}).unwrap_err();
            assert!(matches!(err, ArchiveError::DigestMismatch { .. }));
        });

        // Short body -> size mismatch
        let short_fake = std::rc::Rc::new(TestFake {
            calls: RefCell::new(Vec::new()),
            body: b"short".to_vec(),
        });
        with_fake_runtime_fetch(&short_fake, || {
            let err = download_runtime_fetch(&handle, &bad_path, |_, _| {}).unwrap_err();
            assert!(matches!(err, ArchiveError::SizeMismatch { .. }));
        });

        // Builder fetch of owner origin records nothing and refuses
        let err = ensure_verified_url(
            "https://updates.solstone.app/assets/test-key",
            sha256,
            Some(size),
            &bad_path,
            |_, _| {},
        )
        .unwrap_err();
        assert!(matches!(err, ArchiveError::HostRefused { .. }));
        assert_eq!(fake.calls.borrow().len(), 1);
    }
}
