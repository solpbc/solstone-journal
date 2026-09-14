// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-local, self-contained opaque MCP references.

use base64::Engine as _;
use ring::{
    aead, hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};

const TOKEN_VERSION: u8 = 1;
const NONCE_BYTES: usize = 12;
const HMAC_BYTES: usize = 32;

/// Closed reference kinds; a token minted for one tool cannot be reinterpreted
/// by another tool.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReferenceKind {
    Entry,
    Segment,
    Entity,
    Cursor,
}

impl ReferenceKind {
    const fn tag(self) -> u8 {
        match self {
            Self::Entry => 1,
            Self::Segment => 2,
            Self::Entity => 3,
            Self::Cursor => 4,
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Entry),
            2 => Some(Self::Segment),
            3 => Some(Self::Entity),
            4 => Some(Self::Cursor),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct EntryReference {
    pub day: String,
    pub stream: String,
    pub path: String,
    pub idx: i64,
    pub row_id: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SegmentReference {
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct EntityReference {
    pub facet_id: String,
    pub entity_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct CursorReference {
    pub query_hash: String,
    pub categories: Vec<String>,
    pub scope_ids: Vec<String>,
    pub whole_journal: bool,
    pub day: String,
    pub stream: String,
    pub path: String,
    pub idx: i64,
    pub row_id: i64,
    pub content_fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub(crate) enum ReferenceTarget {
    Entry(EntryReference),
    Segment(SegmentReference),
    Entity(EntityReference),
    Cursor(CursorReference),
}

impl ReferenceTarget {
    pub(crate) const fn kind(&self) -> ReferenceKind {
        match self {
            Self::Entry(_) => ReferenceKind::Entry,
            Self::Segment(_) => ReferenceKind::Segment,
            Self::Entity(_) => ReferenceKind::Entity,
            Self::Cursor(_) => ReferenceKind::Cursor,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct BoundReference {
    connection: String,
    generation: u64,
    target: ReferenceTarget,
}

/// A self-contained reference codec. Its random keys are intentionally never
/// persisted, so a restart invalidates every outstanding token.
pub(crate) struct ReferenceCodec {
    encryption: aead::LessSafeKey,
    mac: hmac::Key,
    random: SystemRandom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReferenceError {
    NotFound,
}

impl ReferenceCodec {
    pub(crate) fn new() -> Result<Self, ReferenceError> {
        let random = SystemRandom::new();
        let mut key = [0_u8; 32];
        random
            .fill(&mut key)
            .map_err(|_| ReferenceError::NotFound)?;
        let encryption = aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::AES_256_GCM, &key)
                .map_err(|_| ReferenceError::NotFound)?,
        );
        Ok(Self {
            encryption,
            mac: hmac::Key::new(hmac::HMAC_SHA256, &key),
            random,
        })
    }

    pub(crate) fn mint(
        &self,
        connection: &str,
        generation: u64,
        target: ReferenceTarget,
    ) -> Result<String, ReferenceError> {
        let kind = target.kind();
        let bound = BoundReference {
            connection: connection.to_owned(),
            generation,
            target,
        };
        let mut ciphertext = serde_json::to_vec(&bound).map_err(|_| ReferenceError::NotFound)?;
        let mut nonce = [0_u8; NONCE_BYTES];
        self.random
            .fill(&mut nonce)
            .map_err(|_| ReferenceError::NotFound)?;
        self.encryption
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(&[TOKEN_VERSION, kind.tag()]),
                &mut ciphertext,
            )
            .map_err(|_| ReferenceError::NotFound)?;
        let mut token = Vec::with_capacity(2 + NONCE_BYTES + ciphertext.len() + HMAC_BYTES);
        token.extend([TOKEN_VERSION, kind.tag()]);
        token.extend(nonce);
        token.extend(ciphertext);
        let tag = hmac::sign(&self.mac, &token);
        token.extend(tag.as_ref());
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token))
    }

    pub(crate) fn resolve(
        &self,
        value: &str,
        expected_kind: ReferenceKind,
        connection: &str,
        generation: u64,
    ) -> Result<ReferenceTarget, ReferenceError> {
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| ReferenceError::NotFound)?;
        if token.len() <= 2 + NONCE_BYTES + HMAC_BYTES || token[0] != TOKEN_VERSION {
            return Err(ReferenceError::NotFound);
        }
        let Some(kind) = ReferenceKind::from_tag(token[1]) else {
            return Err(ReferenceError::NotFound);
        };
        if kind != expected_kind {
            return Err(ReferenceError::NotFound);
        }
        let signed_len = token.len() - HMAC_BYTES;
        hmac::verify(&self.mac, &token[..signed_len], &token[signed_len..])
            .map_err(|_| ReferenceError::NotFound)?;
        let nonce: [u8; NONCE_BYTES] = token[2..2 + NONCE_BYTES]
            .try_into()
            .map_err(|_| ReferenceError::NotFound)?;
        let mut ciphertext = token[2 + NONCE_BYTES..signed_len].to_vec();
        let plaintext = self
            .encryption
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(&[TOKEN_VERSION, kind.tag()]),
                &mut ciphertext,
            )
            .map_err(|_| ReferenceError::NotFound)?;
        let bound: BoundReference =
            serde_json::from_slice(plaintext).map_err(|_| ReferenceError::NotFound)?;
        if bound.connection != connection
            || bound.generation != generation
            || bound.target.kind() != expected_kind
        {
            return Err(ReferenceError::NotFound);
        }
        Ok(bound.target)
    }
}

#[cfg(test)]
mod tests {
    use super::{EntryReference, ReferenceCodec, ReferenceError, ReferenceKind, ReferenceTarget};

    #[test]
    fn references_are_opaque_and_bound_to_connection_generation_and_kind() {
        let codec = ReferenceCodec::new().unwrap();
        let token = codec
            .mint(
                "bearer:a",
                7,
                ReferenceTarget::Entry(EntryReference {
                    day: "20260914".to_owned(),
                    stream: "default".to_owned(),
                    path: "notes/private.md".to_owned(),
                    idx: 2,
                    row_id: 9,
                }),
            )
            .unwrap();
        assert!(!token.contains("notes"));
        assert!(matches!(
            codec.resolve(&token, ReferenceKind::Entry, "bearer:a", 7),
            Ok(ReferenceTarget::Entry(_))
        ));
        assert_eq!(
            codec.resolve(&token, ReferenceKind::Entry, "bearer:b", 7),
            Err(ReferenceError::NotFound)
        );
        assert_eq!(
            codec.resolve("notes/a:b.txt:42", ReferenceKind::Entry, "bearer:a", 7),
            Err(ReferenceError::NotFound)
        );
        let mut tampered = token.into_bytes();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        assert_eq!(
            codec.resolve(
                std::str::from_utf8(&tampered).unwrap(),
                ReferenceKind::Entry,
                "bearer:a",
                7
            ),
            Err(ReferenceError::NotFound)
        );
    }
}
