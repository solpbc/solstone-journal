// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Verified artifact download primitives shared by native installers.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha256};
use solstone_core_assets::Artifact;
use thiserror::Error;

const MAX_REDIRECT_HOPS: u8 = 5;
const DOWNLOAD_ALLOWED_HOSTS: &[&str] = &["updates.solstone.app"];

/// Build-time upstreams a developer machine may fetch when preparing a
/// distribution. These hosts are **not** an owner-facing origin.
///
/// `DOWNLOAD_ALLOWED_HOSTS` / `PRODUCTION_DOWNLOAD_POLICY` is a covenant
/// property of `P-system-models`: an owner's install may only reach
/// `updates.solstone.app`. Widening that list would let an owner-facing
/// fetch follow a redirect off our origin.
///
/// Builder-input acquisition is a different trust basis. It runs on a
/// developer or CI machine, never on an owner's host, and consumes
/// digest-pinned upstream sources (FFmpeg, zig, rust-std, ONNX Runtime
/// wheels, PDFium). The pin is the admission; the host list only stops
/// the fetch from wandering. The two policies stay separate so a
/// reviewer who finds two allow-lists can tell which is which without
/// guessing: production is the owner's origin, builder-input is the
/// packaging machine's pin table.
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
];

#[derive(Debug, Clone, Copy)]
pub struct DownloadHostPolicy<'a> {
    pub allowed_hosts: &'a [&'a str],
    pub allow_http: bool,
    pub origin_base_url: &'a str,
}

pub const PRODUCTION_DOWNLOAD_POLICY: DownloadHostPolicy<'static> = DownloadHostPolicy {
    allowed_hosts: DOWNLOAD_ALLOWED_HOSTS,
    allow_http: false,
    origin_base_url: "https://updates.solstone.app",
};

/// Policy for digest-pinned builder-input acquisition.
///
/// `origin_base_url` is unused: builder inputs are fetched from their
/// pin-table URL, not composed against a single origin. See
/// `BUILDER_INPUT_ALLOWED_HOSTS` for why this is not
/// `PRODUCTION_DOWNLOAD_POLICY`.
pub const BUILDER_INPUT_DOWNLOAD_POLICY: DownloadHostPolicy<'static> = DownloadHostPolicy {
    allowed_hosts: BUILDER_INPUT_ALLOWED_HOSTS,
    allow_http: false,
    origin_base_url: "",
};

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

/// A narrow byte-download seam for installers that must decode an archive in
/// memory after the authoritative compressed-byte digest has been checked.
pub trait ByteDownload {
    fn fetch(&self, url: &str, timeout: Duration) -> Result<Vec<u8>, ByteDownloadError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ByteDownloadError {
    HttpStatus(u16),
    Transport,
    DigestMismatch,
    InsecureUrl,
}

#[derive(Debug, Default)]
pub struct UreqByteDownload;

impl ByteDownload for UreqByteDownload {
    fn fetch(&self, url: &str, timeout: Duration) -> Result<Vec<u8>, ByteDownloadError> {
        let response = ureq::get(url)
            .config()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|_| ByteDownloadError::Transport)?;
        if !response.status().is_success() {
            return Err(ByteDownloadError::HttpStatus(response.status().as_u16()));
        }
        let mut reader = response.into_body().into_reader();
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .map_err(|_| ByteDownloadError::Transport)?;
        Ok(bytes)
    }
}

pub fn verify_sha256_bytes(bytes: &[u8], expected: &str) -> Result<(), ByteDownloadError> {
    let actual = Sha256::digest(bytes);
    let actual: String = actual.iter().map(|byte| format!("{byte:02x}")).collect();
    if actual == expected {
        Ok(())
    } else {
        Err(ByteDownloadError::DigestMismatch)
    }
}

trait Backoff {
    fn back_off(&self, duration: Duration);
}

struct ThreadBackoff;

impl Backoff for ThreadBackoff {
    fn back_off(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Download and verify a pinned in-memory archive.
///
/// HTTP status responses fail immediately. Transport and timeout failures use
/// the Python reference backoff: 0.25 seconds multiplied by the attempt index.
pub fn download_verified_bytes(
    downloader: &dyn ByteDownload,
    url: &str,
    expected_sha256: &str,
    attempts: u8,
    timeout: Duration,
) -> Result<Vec<u8>, ByteDownloadError> {
    download_verified_bytes_with(
        downloader,
        url,
        expected_sha256,
        attempts,
        timeout,
        &ThreadBackoff,
    )
}

pub(crate) fn download_verified_bytes_with(
    downloader: &dyn ByteDownload,
    url: &str,
    expected_sha256: &str,
    attempts: u8,
    timeout: Duration,
    backoff: &dyn Backoff,
) -> Result<Vec<u8>, ByteDownloadError> {
    if !url.starts_with("https://") {
        return Err(ByteDownloadError::InsecureUrl);
    }
    let attempts = attempts.max(1);
    let mut last = ByteDownloadError::Transport;
    for attempt in 0..attempts {
        match downloader.fetch(url, timeout) {
            Ok(bytes) => {
                verify_sha256_bytes(&bytes, expected_sha256)?;
                return Ok(bytes);
            }
            Err(error @ ByteDownloadError::HttpStatus(_)) => return Err(error),
            Err(error) => {
                last = error;
                if attempt + 1 < attempts {
                    backoff.back_off(Duration::from_millis(250 * u64::from(attempt + 1)));
                }
            }
        }
    }
    Err(last)
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
        return Err(ArchiveError::DigestMismatch {
            expected: expected.to_owned(),
            actual,
        });
    }
    Ok(actual)
}

pub fn download_verified(
    artifact: &Artifact,
    destination: &Path,
    policy: &DownloadHostPolicy<'_>,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<(), ArchiveError> {
    download_verified_origin(
        artifact.origin_key,
        artifact.sha256,
        Some(artifact.size_bytes),
        destination,
        policy,
        progress,
    )
}

pub fn download_verified_origin(
    origin_key: &str,
    sha256: &str,
    expected_size: Option<u64>,
    destination: &Path,
    policy: &DownloadHostPolicy<'_>,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<(), ArchiveError> {
    let origin = origin_url(policy.origin_base_url, origin_key);
    download_verified_url(
        &origin,
        sha256,
        expected_size,
        destination,
        policy,
        progress,
    )
}

/// Fetch `url` through `policy`, write `destination`, and require `sha256`.
///
/// Use [`PRODUCTION_DOWNLOAD_POLICY`] for owner-facing origin keys and
/// [`BUILDER_INPUT_DOWNLOAD_POLICY`] for pin-table builder inputs. The
/// policies are not interchangeable: see `BUILDER_INPUT_ALLOWED_HOSTS`.
pub fn download_verified_url(
    url: &str,
    sha256: &str,
    expected_size: Option<u64>,
    destination: &Path,
    policy: &DownloadHostPolicy<'_>,
    mut progress: impl FnMut(u64, Option<u64>),
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

/// Like [`download_verified_url`], but skip the network when `destination`
/// already matches `sha256`. Returns `true` when a fetch ran.
pub fn ensure_verified_url(
    url: &str,
    sha256: &str,
    expected_size: Option<u64>,
    destination: &Path,
    policy: &DownloadHostPolicy<'_>,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<bool, ArchiveError> {
    if destination.is_file() {
        match verify_sha256(destination, sha256) {
            Ok(_) => return Ok(false),
            Err(ArchiveError::DigestMismatch { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    download_verified_url(url, sha256, expected_size, destination, policy, progress)?;
    Ok(true)
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

pub fn origin_url(base: &str, origin_key: &str) -> String {
    format!("{base}/{origin_key}")
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

pub fn make_executable(path: &Path) -> Result<(), ArchiveError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
pub fn clear_macos_quarantine(path: &Path) -> Result<(), ArchiveError> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("xattr")
            .args(["-d", "-r", "com.apple.quarantine"])
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::io::Write;

    struct ScriptedDownload {
        responses: RefCell<Vec<Result<Vec<u8>, ByteDownloadError>>>,
        calls: Cell<u8>,
    }
    impl ByteDownload for ScriptedDownload {
        fn fetch(&self, _: &str, _: Duration) -> Result<Vec<u8>, ByteDownloadError> {
            self.calls.set(self.calls.get() + 1);
            self.responses.borrow_mut().remove(0)
        }
    }

    struct RecordingBackoff {
        delays: RefCell<Vec<Duration>>,
    }

    impl Backoff for RecordingBackoff {
        fn back_off(&self, duration: Duration) {
            self.delays.borrow_mut().push(duration);
        }
    }

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
    fn origin_url_joins_the_authoritative_key() {
        assert_eq!(
            origin_url("https://updates.solstone.app", "assets/tool"),
            "https://updates.solstone.app/assets/tool"
        );
    }

    #[test]
    fn allowed_host_comparison_is_case_insensitive_without_a_network_request() {
        let policy = DownloadHostPolicy {
            allowed_hosts: &["MiXeD.ExAmPlE"],
            allow_http: false,
            origin_base_url: "https://mixed.example",
        };
        assert_eq!(
            validate_url("https://mixed.example/asset", &policy)
                .unwrap()
                .host,
            "mixed.example"
        );
    }

    #[test]
    fn byte_download_http_status_is_not_retried() {
        let download = ScriptedDownload {
            responses: RefCell::new(vec![Err(ByteDownloadError::HttpStatus(404))]),
            calls: Cell::new(0),
        };
        assert_eq!(
            download_verified_bytes(
                &download,
                "https://example.invalid/asset",
                "00",
                3,
                Duration::ZERO,
            ),
            Err(ByteDownloadError::HttpStatus(404))
        );
        assert_eq!(download.calls.get(), 1);
    }

    #[test]
    fn byte_download_retries_transport_then_verifies() {
        let bytes = b"fixture".to_vec();
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let download = ScriptedDownload {
            responses: RefCell::new(vec![
                Err(ByteDownloadError::Transport),
                Err(ByteDownloadError::Transport),
                Ok(bytes.clone()),
            ]),
            calls: Cell::new(0),
        };
        let backoff = RecordingBackoff {
            delays: RefCell::new(Vec::new()),
        };
        assert_eq!(
            download_verified_bytes_with(
                &download,
                "https://example.invalid/asset",
                &digest,
                3,
                Duration::ZERO,
                &backoff,
            )
            .unwrap(),
            bytes
        );
        assert_eq!(download.calls.get(), 3);
        assert_eq!(
            *backoff.delays.borrow(),
            [Duration::from_millis(250), Duration::from_millis(500)]
        );
    }

    #[test]
    fn production_policy_refuses_a_builder_input_host() {
        let error = validate_url(
            "https://github.com/FFmpeg/FFmpeg/archive/deadbeef.tar.gz",
            &PRODUCTION_DOWNLOAD_POLICY,
        )
        .expect_err("owner-facing policy must not admit github.com");
        assert!(matches!(
            error,
            ArchiveError::HostRefused { host } if host == "github.com"
        ));
    }

    #[test]
    fn builder_input_policy_refuses_the_owner_origin() {
        let error = validate_url(
            "https://updates.solstone.app/assets/tool",
            &BUILDER_INPUT_DOWNLOAD_POLICY,
        )
        .expect_err("builder-input policy must not admit the owner origin");
        assert!(matches!(
            error,
            ArchiveError::HostRefused { host } if host == "updates.solstone.app"
        ));
    }

    #[test]
    fn builder_input_policy_admits_each_pinned_upstream() {
        for url in [
            "https://github.com/FFmpeg/FFmpeg/archive/deadbeef.tar.gz",
            "https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz",
            "https://cmake.org/files/v3.31/cmake-3.31.12-windows-x86_64.zip",
            "https://static.rust-lang.org/dist/rust-std.tar.xz",
            "https://www.python.org/ftp/python/3.12.10/python-3.12.10-embed-amd64.zip",
            "https://files.pythonhosted.org/packages/onnxruntime.whl",
        ] {
            validate_url(url, &BUILDER_INPUT_DOWNLOAD_POLICY)
                .unwrap_or_else(|error| panic!("{url}: {error}"));
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
            &BUILDER_INPUT_DOWNLOAD_POLICY,
            |_, _| {},
        )
        .expect("matching cache must not fetch");
        assert!(!fetched);
    }
}
