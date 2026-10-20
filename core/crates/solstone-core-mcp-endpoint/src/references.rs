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
    MemoryRecall,
    TranscriptList,
    FacetList,
    EntityList,
    Activity,
    ActivityList,
}

impl ReferenceKind {
    const fn tag(self) -> u8 {
        match self {
            Self::Entry => 1,
            Self::Segment => 2,
            Self::Entity => 3,
            Self::Cursor => 4,
            Self::MemoryRecall => 5,
            Self::TranscriptList => 6,
            Self::FacetList => 7,
            Self::EntityList => 8,
            Self::Activity => 9,
            Self::ActivityList => 10,
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Entry),
            2 => Some(Self::Segment),
            3 => Some(Self::Entity),
            4 => Some(Self::Cursor),
            5 => Some(Self::MemoryRecall),
            6 => Some(Self::TranscriptList),
            7 => Some(Self::FacetList),
            8 => Some(Self::EntityList),
            9 => Some(Self::Activity),
            10 => Some(Self::ActivityList),
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
    /// File digest bound into an activity reference. Absent for every other entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_proof: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u64>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statement_ordinal: Option<u64>,
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
    /// File digest bound into an activity cursor. Absent for every other cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_proof: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct MemoryRecallCursor {
    pub source_key: String,
    pub credential_lineage: String,
    pub query: Option<String>,
    pub reference_date: chrono::NaiveDate,
    pub compiled_mode: solstone_core_indexer_query::OwnMemoryQueryMode,
    pub effective_date: solstone_core_indexer_query::EffectiveDateConstraint,
    pub anchor_day: String,
    pub anchor_path: String,
    pub anchor_inclusive: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct TranscriptListCursor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facet: Option<String>,
    pub last_day: String,
    pub last_stream: String,
    pub last_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct FacetListCursor {
    pub last_facet_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct EntityListCursor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facet: Option<String>,
    pub last_facet_id: String,
    pub last_entity_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ActivityReference {
    pub facet_id: String,
    pub facet_dir: String,
    pub day: String,
    pub record_identity: String,
    pub file_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_index: Option<usize>,
    /// Digest of the admitted transcript versions and composed text. Present on
    /// a source-page continuation. A later page refuses when this no longer
    /// matches the transcripts the activity points at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_binding: Option<String>,
}

impl ActivityReference {
    pub(crate) fn relative_path(&self) -> Result<String, crate::tools::ToolError> {
        activity_relative_path(&self.facet_dir, &self.day)
    }
}

pub(crate) fn activity_relative_path(
    facet_dir: &str,
    day: &str,
) -> Result<String, crate::tools::ToolError> {
    if facet_dir.is_empty()
        || facet_dir == "."
        || facet_dir == ".."
        || facet_dir.contains('/')
        || facet_dir.contains('\\')
        || !solstone_core_journal_io::is_day_key(day)
    {
        return Err(crate::tools::ToolError::ReferenceNotFound);
    }
    Ok(format!("facets/{facet_dir}/activities/{day}.jsonl"))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ActivityListCursor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    pub last_day: String,
    pub last_facet: String,
    pub last_created_at: i64,
    pub last_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub(crate) enum ReferenceTarget {
    Entry(EntryReference),
    Segment(SegmentReference),
    Entity(EntityReference),
    Cursor(CursorReference),
    MemoryRecall(MemoryRecallCursor),
    TranscriptList(TranscriptListCursor),
    FacetList(FacetListCursor),
    EntityList(EntityListCursor),
    Activity(ActivityReference),
    ActivityList(ActivityListCursor),
}

impl ReferenceTarget {
    pub(crate) const fn kind(&self) -> ReferenceKind {
        match self {
            Self::Entry(_) => ReferenceKind::Entry,
            Self::Segment(_) => ReferenceKind::Segment,
            Self::Entity(_) => ReferenceKind::Entity,
            Self::Cursor(_) => ReferenceKind::Cursor,
            Self::MemoryRecall(_) => ReferenceKind::MemoryRecall,
            Self::TranscriptList(_) => ReferenceKind::TranscriptList,
            Self::FacetList(_) => ReferenceKind::FacetList,
            Self::EntityList(_) => ReferenceKind::EntityList,
            Self::Activity(_) => ReferenceKind::Activity,
            Self::ActivityList(_) => ReferenceKind::ActivityList,
        }
    }
}

pub(crate) fn is_framed_entity_token(value: &str) -> bool {
    if let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(value) {
        bytes.len() >= 2 + NONCE_BYTES + 16 + HMAC_BYTES
            && bytes[0] == TOKEN_VERSION
            && bytes[1] == ReferenceKind::Entity.tag()
    } else {
        false
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
    signing_generation: u64,
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
        let mut signing_generation = [0_u8; 8];
        random
            .fill(&mut signing_generation)
            .map_err(|_| ReferenceError::NotFound)?;
        let encryption = aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::AES_256_GCM, &key)
                .map_err(|_| ReferenceError::NotFound)?,
        );
        Ok(Self {
            encryption,
            mac: hmac::Key::new(hmac::HMAC_SHA256, &key),
            random,
            signing_generation: u64::from_le_bytes(signing_generation),
        })
    }

    pub(crate) fn signing_generation(&self) -> u64 {
        self.signing_generation
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

    pub(crate) fn resolve_entry_or_activity(
        &self,
        value: &str,
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
        if kind != ReferenceKind::Entry && kind != ReferenceKind::Activity {
            return Err(ReferenceError::NotFound);
        }
        self.resolve(value, kind, connection, generation)
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
                    activity_proof: None,
                    record_identity: None,
                    source_digest: None,
                    byte_offset: None,
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

    #[test]
    fn reference_kind_tags_roundtrip_1_through_10() {
        for (kind, expected_tag) in [
            (ReferenceKind::Entry, 1),
            (ReferenceKind::Segment, 2),
            (ReferenceKind::Entity, 3),
            (ReferenceKind::Cursor, 4),
            (ReferenceKind::MemoryRecall, 5),
            (ReferenceKind::TranscriptList, 6),
            (ReferenceKind::FacetList, 7),
            (ReferenceKind::EntityList, 8),
            (ReferenceKind::Activity, 9),
            (ReferenceKind::ActivityList, 10),
        ] {
            assert_eq!(kind.tag(), expected_tag);
            assert_eq!(ReferenceKind::from_tag(expected_tag), Some(kind));
        }
        for unknown in [0, 11, 255] {
            assert_eq!(ReferenceKind::from_tag(unknown), None);
        }
    }

    #[test]
    fn list_cursors_fail_cross_kind_resolve() {
        let codec = ReferenceCodec::new().unwrap();
        let facet_token = codec
            .mint(
                "bearer:a",
                1,
                ReferenceTarget::FacetList(super::FacetListCursor {
                    last_facet_id: "facet-1".to_owned(),
                }),
            )
            .unwrap();
        assert_eq!(
            codec.resolve(&facet_token, ReferenceKind::EntityList, "bearer:a", 1),
            Err(ReferenceError::NotFound)
        );
        let entity_token = codec
            .mint(
                "bearer:a",
                1,
                ReferenceTarget::EntityList(super::EntityListCursor {
                    facet: None,
                    last_facet_id: "facet-1".to_owned(),
                    last_entity_id: "ent-1".to_owned(),
                }),
            )
            .unwrap();
        assert_eq!(
            codec.resolve(&entity_token, ReferenceKind::FacetList, "bearer:a", 1),
            Err(ReferenceError::NotFound)
        );
    }

    #[test]
    fn entry_without_activity_proof_decodes_as_absent() {
        let entry: EntryReference = serde_json::from_str(
            r#"{"day":"20260914","stream":"default","path":"notes/private.md","idx":2,"row_id":9}"#,
        )
        .expect("legacy entry");
        assert!(entry.activity_proof.is_none());
        let cursor: super::CursorReference = serde_json::from_str(
            r#"{"query_hash":"abc","categories":[],"scope_ids":[],"whole_journal":true,"day":"20260914","stream":"default","path":"notes/private.md","idx":2,"row_id":9,"content_fingerprint":"def"}"#,
        )
        .expect("legacy cursor");
        assert!(cursor.activity_proof.is_none());
    }
}
