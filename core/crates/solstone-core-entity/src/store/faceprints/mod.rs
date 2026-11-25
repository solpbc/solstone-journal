// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private face evidence and owner decisions. Embeddings are journal-local.

pub mod accumulation;
mod codec;
pub mod cross;
pub mod panel;
pub mod photo_match;
mod storage;
pub use storage::*;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const WIDTH: usize = 128;
pub const ARCHIVE: &str = "faceprints.npz";
pub const STATE: &str = "faces_state.json";
pub const PHOTO: &str = "photo.jpg";
pub type Result<T> = std::result::Result<T, String>;

/// Leave polling writers a real turn during long face recovery walks. Call only
/// between authoritative units, after all trust, source and record guards drop.
pub struct ReconcileYield(std::time::Instant);

impl Default for ReconcileYield {
    fn default() -> Self {
        Self(std::time::Instant::now())
    }
}

impl ReconcileYield {
    pub fn after_unit(&mut self) {
        if self.0.elapsed() >= std::time::Duration::from_secs(1) {
            std::thread::sleep(
                solstone_core_journal_io::DEFAULT_LOCK_POLL_INTERVAL.saturating_mul(2),
            );
            self.0 = std::time::Instant::now();
        }
    }
}

/// Owner-action refusals that HTTP callers can classify without parsing text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionError {
    FacesOn,
    UnknownKey,
    NoEvidence,
    StaleAction,
    BusySource(String),
    Other(String),
}
pub type ActionResult<T> = std::result::Result<T, ActionError>;
impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FacesOn => f.write_str("turn off face suggestions before clearing face data"),
            Self::UnknownKey => f.write_str("face is no longer available"),
            Self::NoEvidence => f.write_str("no face evidence"),
            Self::StaleAction => {
                f.write_str("that face changed or its action is no longer the last one")
            }
            Self::BusySource(message) | Self::Other(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for ActionError {}
impl From<String> for ActionError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}
impl From<&str> for ActionError {
    fn from(message: &str) -> Self {
        Self::Other(message.to_owned())
    }
}
impl ActionError {
    pub(super) fn source(error: std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Self::BusySource(error.to_string())
        } else {
            Self::Other(error.to_string())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FaceKey {
    pub day: String,
    pub stream: String,
    pub stream_layout: String,
    pub segment_key: String,
    pub source_name: String,
    pub frame_id: u64,
    pub face_idx: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Anchored,
    Box,
    Photo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub schema_version: u32,
    #[serde(flatten)]
    pub key: FaceKey,
    /// JSON-encoded [facet, day, activity id], never an id alone.
    pub activities: BTreeSet<String>,
    pub source: Source,
    pub label_text: Option<String>,
    pub quality: f32,
    pub det_score: f32,
    pub face_w: f32,
    pub category: String,
    pub crop: Option<String>,
    pub added_at: String,
    pub rejected: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Face {
    pub embedding: Vec<f32>,
    pub metadata: Metadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Encoder {
    pub id: String,
    pub sha256: String,
    pub width: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub format: String,
    pub version: u32,
    pub encoder: Encoder,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Archive {
    pub envelope: Envelope,
    pub faces: Vec<Face>,
}

impl Archive {
    pub fn empty(encoder: Encoder) -> Self {
        Self {
            envelope: Envelope {
                format: "solstone-faceprint-envelope".into(),
                version: 1,
                encoder,
            },
            faces: vec![],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChosenPhoto {
    pub face: Face,
    pub encoder: Encoder,
    pub chosen_at: String,
    /// Authoritative copy: a crash between state and photo publication is repairable.
    pub jpeg: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub photo: Option<ChosenPhoto>,
    pub dismissed_at: Option<String>,
    pub dismissed_looks: Vec<Vec<f32>>,
    pub rejections: Vec<(FaceKey, bool)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Undo {
    pub token: String,
    pub decision: Decision,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FacesState {
    pub schema_version: u32,
    #[serde(flatten)]
    pub decision: Decision,
    pub last: Option<Undo>,
}
impl Default for FacesState {
    fn default() -> Self {
        Self {
            schema_version: 1,
            decision: Decision::default(),
            last: None,
        }
    }
}

/// Photos of people are on unless the owner turned them off: an absent switch
/// is on, `false` is off, and any other stored value is off, so a corrupted
/// value never turns face reading on.
pub fn enabled(journal: &std::path::Path) -> Result<bool> {
    let config =
        solstone_core_journal_config::read_journal_config(journal).map_err(|e| e.to_string())?;
    Ok(
        match config
            .config
            .as_ref()
            .and_then(|v| v.get("faces").and_then(|v| v.get("enabled")))
        {
            None => true,
            Some(value) => value == &serde_json::Value::Bool(true),
        },
    )
}

/// Matching faces in still photos against meeting faceprints is gated behind
/// its own explicit opt-in switch (`faces.photo_match_enabled`), OFF by default
/// when absent or anything other than `true`.
pub fn photo_match_enabled(journal: &std::path::Path) -> Result<bool> {
    let config =
        solstone_core_journal_config::read_journal_config(journal).map_err(|e| e.to_string())?;
    Ok(config
        .config
        .as_ref()
        .and_then(|v| v.get("faces"))
        .and_then(|v| v.get("photo_match_enabled"))
        == Some(&serde_json::Value::Bool(true)))
}

/// Whether still-photo face matching is active for this journal: both the
/// owner's face recognition switch and the photo-matching flag must be on.
pub fn photo_faces_active(journal: &std::path::Path) -> Result<bool> {
    Ok(enabled(journal)? && photo_match_enabled(journal)?)
}

pub(super) fn valid_embedding(e: &[f32]) -> bool {
    e.len() == WIDTH
        && e.iter().all(|x| x.is_finite())
        && (e.iter().map(|x| x * x).sum::<f32>() - 1.).abs() < 0.01
}
