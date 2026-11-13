// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private face evidence and owner decisions. Embeddings are journal-local.

pub mod accumulation;
mod codec;
pub mod panel;
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

pub fn enabled(journal: &std::path::Path) -> Result<bool> {
    let config =
        solstone_core_journal_config::read_journal_config(journal).map_err(|e| e.to_string())?;
    Ok(config
        .config
        .as_ref()
        .and_then(|v| v.get("faces").and_then(|v| v.get("enabled")))
        .and_then(serde_json::Value::as_bool)
        == Some(true))
}

pub(super) fn valid_embedding(e: &[f32]) -> bool {
    e.len() == WIDTH
        && e.iter().all(|x| x.is_finite())
        && (e.iter().map(|x| x * x).sum::<f32>() - 1.).abs() < 0.01
}
