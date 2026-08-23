// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{ConvergenceError, DurableRole};

/// SHA-256 of canonical JSON, lowercase hex. Digest excludes the on-disk newline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordDigest(pub(crate) String);

impl RecordDigest {
    pub fn as_hex(&self) -> &str {
        &self.0
    }
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

pub(crate) fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ConvergenceError> {
    let mut json = serde_json::to_value(value).map_err(|source| ConvergenceError::Io {
        operation: "serialize canonical json",
        role: DurableRole::Record,
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
    })?;
    sort_value(&mut json);
    serde_json::to_vec(&json).map_err(|source| ConvergenceError::Io {
        operation: "emit canonical json",
        role: DurableRole::Record,
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
    })
}

pub(crate) fn digest_bytes(bytes: &[u8]) -> RecordDigest {
    RecordDigest(hex_encode(&Sha256::digest(bytes)))
}

pub(crate) fn digest_value<T: Serialize>(value: &T) -> Result<RecordDigest, ConvergenceError> {
    Ok(digest_bytes(&canonical_json_bytes(value)?))
}

fn sort_value(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let mut sorted = BTreeMap::new();
            for (key, mut child) in std::mem::take(object) {
                sort_value(&mut child);
                sorted.insert(key, child);
            }
            *object = sorted.into_iter().collect();
        }
        Value::Array(values) => {
            for child in values {
                sort_value(child);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use serde::Serialize;

    use super::*;

    #[derive(Serialize)]
    struct Pair {
        b: u8,
        a: u8,
    }

    #[test]
    fn canonical_json_sorts_keys() {
        let bytes = canonical_json_bytes(&Pair { b: 2, a: 1 }).unwrap();
        assert_eq!(bytes, br#"{"a":1,"b":2}"#);
    }

    #[test]
    fn digest_excludes_trailing_newline() {
        let body = canonical_json_bytes(&Pair { b: 2, a: 1 }).unwrap();
        let mut on_disk = body.clone();
        on_disk.push(b'\n');
        assert_eq!(digest_bytes(&body), digest_bytes(&body));
        assert_ne!(digest_bytes(&body), digest_bytes(&on_disk));
    }

    #[test]
    fn same_record_same_digest() {
        let left = digest_value(&Pair { b: 2, a: 1 }).unwrap();
        let right = digest_value(&Pair { a: 1, b: 2 }).unwrap();
        assert_eq!(left, right);
    }
}
