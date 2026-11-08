// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    AtomicWriteOptions, LockOptions, atomic_replace, hold_lock, hold_lock_checked,
    install_retained_file,
};

use super::{
    ArchiveError, DownloadHostPolicy, MAX_REDIRECT_HOPS, resolve_location, validate_url,
    verify_sha256_with_cancel,
};

pub(super) fn checkpoint(cancelled: Option<&dyn Fn() -> bool>) -> Result<(), ArchiveError> {
    if cancelled.is_some_and(|cancelled| cancelled()) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "download cancelled").into());
    }
    Ok(())
}

// ureq's receive-body timeout covers the complete transfer, not an idle read.
// Bound each transport wait while retaining its default TLS/proxy connector.
#[derive(Debug)]
struct IdleBound;

#[derive(Debug)]
struct IdleTransport<T>(T);

fn idle_timeout(
    mut timeout: ureq::unversioned::transport::NextTimeout,
) -> ureq::unversioned::transport::NextTimeout {
    if *timeout.after > Duration::from_secs(2) {
        timeout.after = ureq::unversioned::transport::time::Duration::from_secs(2);
    }
    timeout
}

impl<T: ureq::unversioned::transport::Transport> ureq::unversioned::transport::Connector<T>
    for IdleBound
{
    type Out = IdleTransport<T>;

    fn connect(
        &self,
        _: &ureq::unversioned::transport::ConnectionDetails,
        chained: Option<T>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        Ok(chained.map(IdleTransport))
    }
}

impl<T: ureq::unversioned::transport::Transport> ureq::unversioned::transport::Transport
    for IdleTransport<T>
{
    fn buffers(&mut self) -> &mut dyn ureq::unversioned::transport::Buffers {
        self.0.buffers()
    }

    fn transmit_output(
        &mut self,
        amount: usize,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<(), ureq::Error> {
        self.0.transmit_output(amount, idle_timeout(timeout))
    }

    fn await_input(
        &mut self,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<bool, ureq::Error> {
        self.0.await_input(idle_timeout(timeout))
    }

    fn is_open(&mut self) -> bool {
        self.0.is_open()
    }

    fn is_tls(&self) -> bool {
        self.0.is_tls()
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Binding {
    sha256: String,
    size_bytes: Option<u64>,
    destination_name: String,
    representation: String,
}

fn partial_paths(
    destination: &Path,
    sha256: &str,
) -> Result<(PathBuf, PathBuf, Binding), ArchiveError> {
    if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ArchiveError::Download("invalid artifact sha256 pin".into()));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| ArchiveError::Download("destination has no parent".into()))?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ArchiveError::Download("destination has no UTF-8 file name".into()))?;
    let sha256 = sha256.to_ascii_lowercase();
    let partial = parent.join(format!(".{name}.{sha256}.part"));
    let metadata = partial.with_extension("part.json");
    Ok((
        partial,
        metadata,
        Binding {
            sha256,
            size_bytes: None,
            destination_name: name.into(),
            representation: "identity".into(),
        },
    ))
}

fn regular_length(path: &Path) -> io::Result<Option<u64>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "artifact cache entry is a reparse point",
                    ));
                }
            }
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "artifact cache entry is not a regular file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_binding(path: &Path) -> Option<Binding> {
    let length = regular_length(path).ok().flatten()?;
    if length > 16 * 1024 {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Bytes already allocated to this exact artifact. Completed files receive
/// credit only after digest proof; incomplete files need their exact binding.
pub fn retained_prefix_bytes(destination: &Path, sha256: &str, size: u64) -> u64 {
    retained_prefix_bytes_with_cancel(destination, sha256, size, None).unwrap_or(0)
}

pub fn retained_prefix_bytes_with_cancel(
    destination: &Path,
    sha256: &str,
    size: u64,
    cancelled: Option<&dyn Fn() -> bool>,
) -> Result<u64, ArchiveError> {
    checkpoint(cancelled)?;
    if regular_length(destination).ok().flatten() == Some(size)
        && verified_or_mismatch(destination, sha256, cancelled)?
    {
        return Ok(size);
    }
    let Ok((partial, metadata, mut binding)) = partial_paths(destination, sha256) else {
        return Ok(0);
    };
    binding.size_bytes = Some(size);
    if read_binding(&metadata).as_ref() != Some(&binding) {
        return Ok(0);
    }
    Ok(regular_length(&partial)
        .ok()
        .flatten()
        .filter(|length| *length <= size)
        .unwrap_or(0))
}

// Missing or changed cached bytes are reusable only after a fresh download.
// Cancellation is distinct: it must stop the operation without deleting them.
pub(super) fn verified_or_mismatch(
    path: &Path,
    sha256: &str,
    cancelled: Option<&dyn Fn() -> bool>,
) -> Result<bool, ArchiveError> {
    match verify_sha256_with_cancel(path, sha256, cancelled) {
        Ok(_) => Ok(true),
        Err(ArchiveError::Io(error)) if error.kind() == io::ErrorKind::Interrupted => {
            Err(error.into())
        }
        Err(_) => Ok(false),
    }
}

fn sync_file(file: &fs::File) -> io::Result<()> {
    loop {
        match file.sync_all() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

// An interrupted metadata write, or an unrecognized cache entry, must not
// permanently prevent retry. Preserve uncertain bytes in a fresh directory.
fn preserve_unbound(partial: &Path, metadata: &Path) -> Result<(), ArchiveError> {
    if regular_length(partial)?.is_none() && regular_length(metadata)?.is_none() {
        return Ok(());
    }
    let parent = partial.parent().expect("partial has a parent");
    let name = partial
        .file_name()
        .expect("partial has a name")
        .to_string_lossy();
    for index in 0..1000_u32 {
        let preserved = parent.join(format!("{name}.unusable-{index}"));
        match fs::create_dir(&preserved) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
            Ok(()) => {
                for (source, target) in [(partial, "payload"), (metadata, "binding.json")] {
                    match fs::rename(source, preserved.join(target)) {
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        result => result?,
                    }
                }
                return Ok(());
            }
        }
    }
    Err(ArchiveError::Download(
        "too many unrecognized partial cache entries".into(),
    ))
}

fn content_range(value: &str, prefix: u64, expected: Option<u64>) -> Option<u64> {
    let (span, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = span.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    let total = total.parse::<u64>().ok()?;
    if start != prefix || end < start || end >= total || expected.is_some_and(|size| size != total)
    {
        return None;
    }
    end.checked_sub(start)?.checked_add(1)
}

fn publish(
    partial: &Path,
    metadata: &Path,
    destination: &Path,
    sha256: &str,
    cancelled: Option<&dyn Fn() -> bool>,
) -> Result<(), ArchiveError> {
    if let Err(error) = verify_sha256_with_cancel(partial, sha256, cancelled) {
        if matches!(error, ArchiveError::DigestMismatch { .. }) {
            remove_file(partial)?;
            remove_file(metadata)?;
        }
        return Err(error);
    }
    checkpoint(cancelled)?;
    install_retained_file(partial, destination, AtomicWriteOptions::default())
        .map_err(|error| ArchiveError::Download(error.to_string()))?;
    remove_file(metadata)?;
    Ok(())
}

pub(super) fn download_verified_url_internal(
    url: &str,
    sha256: &str,
    expected_size: Option<u64>,
    destination: &Path,
    policy: &DownloadHostPolicy<'_>,
    progress: &mut dyn FnMut(u64, Option<u64>),
    cancelled: Option<&dyn Fn() -> bool>,
) -> Result<bool, ArchiveError> {
    checkpoint(cancelled)?;
    let mut current = validate_url(url, policy)?;
    let (partial, metadata, mut binding) = partial_paths(destination, sha256)?;
    binding.size_bytes = expected_size;
    fs::create_dir_all(destination.parent().expect("validated destination parent"))?;
    // Serialize different pins as well as retries of the same pin. The stable
    // sidecar is outside the temporary file being renamed at publication.
    let options = LockOptions {
        timeout: Duration::from_secs(1800),
        ..LockOptions::default()
    };
    let _lock = if cancelled.is_some() {
        hold_lock_checked(destination, options, &|| {
            checkpoint(cancelled).map_err(|error| error.to_string())
        })
    } else {
        hold_lock(destination, options)
    }
    .map_err(|error| {
        checkpoint(cancelled)
            .err()
            .unwrap_or_else(|| ArchiveError::Download(error.to_string()))
    })?;
    checkpoint(cancelled)?;
    if regular_length(destination)?
        .is_some_and(|size| expected_size.is_none_or(|expected| size == expected))
    {
        match verify_sha256_with_cancel(destination, &binding.sha256, cancelled) {
            Ok(_) => {
                checkpoint(cancelled)?;
                progress(regular_length(destination)?.unwrap_or(0), expected_size);
                return Ok(false);
            }
            Err(ArchiveError::DigestMismatch { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    let mut prefix = regular_length(&partial)?.unwrap_or(0);
    if read_binding(&metadata).as_ref() != Some(&binding) {
        preserve_unbound(&partial, &metadata)?;
        prefix = 0;
        atomic_replace(
            &metadata,
            &serde_json::to_vec(&binding)
                .map_err(|error| ArchiveError::Download(error.to_string()))?,
            AtomicWriteOptions::default(),
        )
        .map_err(|error| ArchiveError::Download(error.to_string()))?;
    }
    if expected_size.is_some_and(|size| prefix > size) {
        remove_file(&partial)?;
        remove_file(&metadata)?;
        return Err(ArchiveError::SizeMismatch {
            expected: expected_size.unwrap(),
            actual: prefix,
        });
    }
    if expected_size == Some(prefix)
        || (expected_size.is_none()
            && prefix > 0
            && verified_or_mismatch(&partial, &binding.sha256, cancelled)?)
    {
        // A zero-byte artifact still needs a file to verify and publish.
        if regular_length(&partial)?.is_none() {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)?;
        }
        checkpoint(cancelled)?;
        publish(&partial, &metadata, destination, &binding.sha256, cancelled)?;
        checkpoint(cancelled)?;
        progress(prefix, expected_size);
        return Ok(true);
    }
    let agent = if cancelled.is_some() {
        use ureq::unversioned::transport::{Connector, DefaultConnector};
        ureq::Agent::with_parts(
            ureq::config::Config::default(),
            DefaultConnector::default().chain(IdleBound),
            ureq::unversioned::resolver::DefaultResolver::default(),
        )
    } else {
        ureq::agent()
    };
    let mut followed = 0_u8;
    let response = loop {
        checkpoint(cancelled)?;
        let mut request = agent
            .get(current.as_str())
            .config()
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_resolve(cancelled.map(|_| Duration::from_secs(5)))
            .timeout_connect(Some(Duration::from_secs(if cancelled.is_some() {
                5
            } else {
                30
            })))
            .timeout_recv_response(Some(Duration::from_secs(if cancelled.is_some() {
                5
            } else {
                30
            })))
            .timeout_recv_body(Some(Duration::from_secs(1800)))
            .build()
            .header("accept-encoding", "identity");
        if prefix > 0 {
            request = request.header("range", &format!("bytes={prefix}-"));
        }
        let response = request.call().map_err(|error| {
            checkpoint(cancelled)
                .err()
                .unwrap_or_else(|| ArchiveError::OriginUnavailable {
                    host: current.host.clone(),
                    message: error.to_string(),
                })
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
                ArchiveError::Download("redirect response has no Location header".into())
            })?;
        current = validate_url(&resolve_location(&current, location)?.as_str(), policy)?;
        followed += 1;
    };
    let refused = |message: String| ArchiveError::OriginUnavailable {
        host: current.host.clone(),
        message,
    };
    if response
        .headers()
        .get("content-encoding")
        .is_some_and(|value| value.as_bytes() != b"identity")
    {
        return Err(refused("artifact response is not identity encoded".into()));
    }
    let declared = response
        .headers()
        .get("content-length")
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| refused("invalid Content-Length".into()))
        })
        .transpose()?;
    let (start, frame_size) = if response.status() == 206 && prefix > 0 {
        let range = response
            .headers()
            .get("content-range")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| content_range(value, prefix, expected_size))
            .ok_or_else(|| refused("invalid resumed Content-Range".into()))?;
        if declared.is_some_and(|size| size != range) {
            return Err(refused(
                "Content-Length disagrees with Content-Range".into(),
            ));
        }
        (prefix, Some(range))
    } else if response.status() == 200 {
        if response.headers().contains_key("content-range") {
            return Err(refused("unexpected Content-Range in full response".into()));
        }
        (0, declared)
    } else {
        return Err(refused(format!(
            "unexpected artifact HTTP status {}",
            response.status()
        )));
    };
    if let (Some(expected), Some(body_size)) = (expected_size, frame_size)
        && let Some(actual) = start.checked_add(body_size)
        && actual > expected
    {
        return Err(ArchiveError::SizeMismatch { expected, actual });
    }
    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(start == 0)
        .open(&partial)?;
    if start == 0 {
        sync_file(&out)?;
        // Unix exposes a directory sync for the new name. Windows checkpoints
        // the file handle above; it has no corresponding directory flush.
        #[cfg(unix)]
        solstone_core_journal_io::sync_root(partial.parent().expect("partial parent"))
            .map_err(|e| ArchiveError::Download(e.to_string()))?;
    }
    if start > 0 {
        use std::io::{Seek, SeekFrom};
        out.seek(SeekFrom::Start(start))?;
    }
    if start > 0 {
        progress(start, expected_size);
    }
    let mut received = start;
    let mut body_received = 0_u64;
    let mut body = response.into_body().into_reader();
    let mut buffer = [0_u8; 64 * 1024];
    let mut checkpoint_at = Instant::now();
    let mut checkpoint_bytes = start;
    loop {
        if let Err(error) = checkpoint(cancelled) {
            sync_file(&out)?;
            return Err(error);
        }
        let count = match body.read(&mut buffer) {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                sync_file(&out)?;
                checkpoint(cancelled)?;
                if cancelled.is_some() {
                    // The short idle bound exists to observe cancellation.
                    // Without a stop request this remains a resumable network
                    // failure, eligible for the installer's bounded retries.
                    return Err(refused(error.to_string()));
                }
                return Err(error.into());
            }
        };
        if count == 0 {
            break;
        }
        let next = received
            .checked_add(count as u64)
            .ok_or_else(|| refused("artifact byte count overflow".into()))?;
        let next_body = body_received
            .checked_add(count as u64)
            .ok_or_else(|| refused("artifact body count overflow".into()))?;
        if frame_size.is_some_and(|size| next_body > size)
            || expected_size.is_some_and(|size| next > size)
        {
            out.set_len(start)?;
            sync_file(&out)?;
            return Err(refused(
                "artifact body exceeds its declared range or pinned size".into(),
            ));
        }
        if let Err(error) = out.write_all(&buffer[..count]) {
            sync_file(&out)?;
            return Err(error.into());
        }
        received = next;
        body_received = next_body;
        if received.saturating_sub(checkpoint_bytes) >= 8 * 1024 * 1024
            || checkpoint_at.elapsed() >= Duration::from_secs(1)
        {
            sync_file(&out)?;
            checkpoint_at = Instant::now();
            checkpoint_bytes = received;
        }
        progress(received, expected_size);
    }
    sync_file(&out)?;
    drop(out);
    checkpoint(cancelled)?;
    if frame_size.is_some_and(|size| body_received != size)
        || expected_size.is_some_and(|size| received != size)
    {
        // A useful prefix survives clean short EOFs as well as read/write errors.
        return Err(ArchiveError::SizeMismatch {
            expected: expected_size.unwrap_or(start.saturating_add(frame_size.unwrap_or(0))),
            actual: received,
        });
    }
    checkpoint(cancelled)?;
    publish(&partial, &metadata, destination, &binding.sha256, cancelled)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn ranges_refuse_unknown_totals_wrong_offsets_and_overflow() {
        assert_eq!(content_range("bytes 50-99/100", 50, Some(100)), Some(50));
        for value in [
            "bytes 50-99/*",
            "bytes 40-99/100",
            "bytes 50-100/100",
            "bytes 0-18446744073709551615/18446744073709551615",
            "bytes 50-99/200",
        ] {
            assert_eq!(content_range(value, 50, Some(100)), None, "{value}");
        }
    }

    #[test]
    fn completed_cache_credit_requires_digest_and_partial_credit_requires_binding() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("model");
        let digest = format!("{:x}", Sha256::digest(b"abcd"));
        fs::write(&target, b"bad!").unwrap();
        assert_eq!(retained_prefix_bytes(&target, &digest, 4), 0);
        fs::write(&target, b"abcd").unwrap();
        assert_eq!(retained_prefix_bytes(&target, &digest, 4), 4);
        fs::remove_file(&target).unwrap();
        let (part, meta, mut binding) = partial_paths(&target, &digest).unwrap();
        binding.size_bytes = Some(4);
        fs::write(&part, b"ab").unwrap();
        assert_eq!(retained_prefix_bytes(&target, &digest, 4), 0);
        fs::write(&meta, serde_json::to_vec(&binding).unwrap()).unwrap();
        assert_eq!(retained_prefix_bytes(&target, &digest, 4), 2);
        assert_eq!(retained_prefix_bytes(&target, &digest, 5), 0);
    }

    #[test]
    fn failed_publication_retains_complete_transfer_for_offline_retry() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("model");
        let digest = format!("{:x}", Sha256::digest(b"abcd"));
        let (part, meta, mut binding) = partial_paths(&target, &digest).unwrap();
        binding.size_bytes = Some(4);
        fs::write(&part, b"abcd").unwrap();
        fs::write(&meta, serde_json::to_vec(&binding).unwrap()).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(target.join("owner-file"), b"preserve").unwrap();
        assert!(publish(&part, &meta, &target, &digest, None).is_err());
        assert_eq!(fs::read(&part).unwrap(), b"abcd");
        assert_eq!(fs::read(target.join("owner-file")).unwrap(), b"preserve");
        fs::remove_file(target.join("owner-file")).unwrap();
        fs::remove_dir(&target).unwrap();
        let policy = DownloadHostPolicy {
            allowed_hosts: &["example.com"],
            allow_http: false,
        };
        assert!(
            download_verified_url_internal(
                "https://example.com/model",
                &digest,
                Some(4),
                &target,
                &policy,
                &mut |_, _| {},
                None,
            )
            .unwrap()
        );
        assert_eq!(fs::read(&target).unwrap(), b"abcd");
    }

    #[test]
    fn complete_partial_publishes_without_network_and_bad_digest_resets() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("model");
        let digest = format!("{:x}", Sha256::digest(b"abcd"));
        let (part, meta, mut binding) = partial_paths(&target, &digest).unwrap();
        binding.size_bytes = Some(4);
        let policy = DownloadHostPolicy {
            allowed_hosts: &["example.com"],
            allow_http: false,
        };
        for (bytes, good) in [(b"bad!", false), (b"abcd", true)] {
            fs::write(&part, bytes).unwrap();
            fs::write(&meta, serde_json::to_vec(&binding).unwrap()).unwrap();
            let result = download_verified_url_internal(
                "https://example.com/model",
                &digest,
                Some(4),
                &target,
                &policy,
                &mut |_, _| {},
                None,
            );
            assert_eq!(result.is_ok(), good);
            assert!(!part.exists());
            assert!(!meta.exists());
        }
        assert_eq!(fs::read(target).unwrap(), b"abcd");
    }

    #[test]
    fn cancellation_during_publication_keeps_complete_partial_and_binding_for_offline_retry() {
        use std::cell::Cell;
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("model");
        let bytes = vec![41_u8; 2 * 1024 * 1024 + 1];
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let (part, meta, mut binding) = partial_paths(&target, &digest).unwrap();
        binding.size_bytes = Some(bytes.len() as u64);
        fs::write(&part, &bytes).unwrap();
        let binding_bytes = serde_json::to_vec(&binding).unwrap();
        fs::write(&meta, &binding_bytes).unwrap();
        let checkpoints = Cell::new(0);
        let error = publish(
            &part,
            &meta,
            &target,
            &digest,
            Some(&|| {
                checkpoints.set(checkpoints.get() + 1);
                checkpoints.get() >= 3
            }),
        )
        .unwrap_err();
        assert!(
            matches!(error, ArchiveError::Io(error) if error.kind() == io::ErrorKind::Interrupted)
        );
        assert!(!target.exists());
        assert_eq!(fs::read(&part).unwrap(), bytes);
        assert_eq!(fs::read(&meta).unwrap(), binding_bytes);
        let policy = DownloadHostPolicy {
            allowed_hosts: &["example.com"],
            allow_http: false,
        };
        assert!(
            download_verified_url_internal(
                "https://example.com/model",
                &digest,
                binding.size_bytes,
                &target,
                &policy,
                &mut |_, _| {},
                Some(&|| false)
            )
            .unwrap()
        );
        assert_eq!(fs::read(&target).unwrap(), bytes);
        assert!(!part.exists() && !meta.exists());
        let checkpoints = Cell::new(0);
        assert!(
            matches!(retained_prefix_bytes_with_cancel(&target, &digest, bytes.len() as u64, Some(&|| {
            checkpoints.set(checkpoints.get() + 1);
            checkpoints.get() >= 4
        })), Err(ArchiveError::Io(error)) if error.kind() == io::ErrorKind::Interrupted)
        );
        assert_eq!(fs::read(&target).unwrap(), bytes);
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod transport_tests;
