// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Value, json};
use solstone_core_journal_io::{AtomicWriteError, AtomicWriteOptions, atomic_replace};

use crate::fixture::local_contract;
use crate::record::valid_spp_reason;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptionVerification {
    pub reason: String,
    pub observed_at: Option<String>,
    pub endpoint: String,
}

#[derive(Debug)]
pub enum TranscriptionVerificationError {
    Io(std::io::Error),
    Atomic(AtomicWriteError),
}

impl fmt::Display for TranscriptionVerificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "transcription verification I/O error: {error}"),
            Self::Atomic(error) => {
                write!(
                    formatter,
                    "transcription verification atomic replace error: {error}"
                )
            }
        }
    }
}

impl std::error::Error for TranscriptionVerificationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Atomic(error) => Some(error),
        }
    }
}

pub fn transcription_verification_path(journal: &Path) -> PathBuf {
    journal.join(
        &local_contract()
            .brain_state
            .paths
            .transcription_verification,
    )
}

pub fn record_transcription_verification(
    journal: &Path,
    raw_reason: &str,
    endpoint: &str,
) -> Result<(), TranscriptionVerificationError> {
    let path = transcription_verification_path(journal);
    let reason = valid_spp_reason(raw_reason);
    let observed_at = Utc::now().to_rfc3339();
    let body = json!({
        "reason": reason,
        "observed_at": observed_at,
        "endpoint": endpoint,
    });
    let bytes = serde_json::to_vec(&body).map_err(|error| {
        TranscriptionVerificationError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error,
        ))
    })?;
    atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(TranscriptionVerificationError::Atomic)?;
    Ok(())
}

pub fn clear_transcription_verification(
    journal: &Path,
) -> Result<(), TranscriptionVerificationError> {
    let path = transcription_verification_path(journal);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(TranscriptionVerificationError::Io(error)),
    }
}

pub fn read_transcription_verification(journal: &Path) -> Option<TranscriptionVerification> {
    let path = transcription_verification_path(journal);
    let bytes = std::fs::read(&path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let object = value.as_object()?;
    let reason = object.get("reason").and_then(Value::as_str)?;
    if reason.is_empty() {
        return None;
    }
    let endpoint = object.get("endpoint").and_then(Value::as_str)?;
    if endpoint.is_empty() {
        return None;
    }
    let observed_at = object
        .get("observed_at")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(TranscriptionVerification {
        reason: reason.to_owned(),
        observed_at,
        endpoint: endpoint.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use std::path::{Path, PathBuf};

    use super::*;

    struct TestJournal(PathBuf);

    impl TestJournal {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-transcription-verification-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestJournal {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn record_and_read_roundtrip() {
        let temp = TestJournal::new();
        let journal = temp.path();

        assert_eq!(read_transcription_verification(journal), None);

        record_transcription_verification(
            journal,
            "gateway_unreachable",
            "https://endpoint.example",
        )
        .unwrap();

        let status = read_transcription_verification(journal).expect("status is present");
        assert_eq!(status.reason, "attestation_not_verified");
        assert_eq!(status.endpoint, "https://endpoint.example");
        let observed = status.observed_at.expect("observed_at is present");
        assert!(DateTime::parse_from_rfc3339(&observed).is_ok());

        clear_transcription_verification(journal).unwrap();
        assert_eq!(read_transcription_verification(journal), None);
        // Repeated clear is success
        clear_transcription_verification(journal).unwrap();
    }

    #[test]
    fn read_malformed_and_missing_fields() {
        let temp = TestJournal::new();
        let journal = temp.path();
        let path = transcription_verification_path(journal);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        std::fs::write(&path, b"not-json").unwrap();
        assert_eq!(read_transcription_verification(journal), None);

        std::fs::write(&path, b"{\"reason\":\"\",\"endpoint\":\"https://e\"}").unwrap();
        assert_eq!(read_transcription_verification(journal), None);

        std::fs::write(
            &path,
            b"{\"reason\":\"attestation_rejected\",\"endpoint\":\"\"}",
        )
        .unwrap();
        assert_eq!(read_transcription_verification(journal), None);

        // Missing observed_at is allowed
        std::fs::write(
            &path,
            b"{\"reason\":\"attestation_rejected\",\"endpoint\":\"https://e\"}",
        )
        .unwrap();
        let status = read_transcription_verification(journal).unwrap();
        assert_eq!(status.reason, "attestation_rejected");
        assert_eq!(status.endpoint, "https://e");
        assert_eq!(status.observed_at, None);

        // Non-string observed_at returns None for observed_at
        std::fs::write(
            &path,
            b"{\"reason\":\"attestation_rejected\",\"endpoint\":\"https://e\",\"observed_at\":12345}",
        )
        .unwrap();
        let status = read_transcription_verification(journal).unwrap();
        assert_eq!(status.observed_at, None);
    }
}
