// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;

pub const SAMPLE_RATE: u32 = 16_000;
pub const WINDOW_S: u32 = 10;
pub const STRIDE_S: u32 = 2;
pub const FRAMES_PER_WINDOW: usize = 589;
pub const SINGLE_SPEAKER_CLASSES: [u8; 3] = [1, 2, 3];
pub const MIN_INTERVAL_S: f64 = 0.5;
pub const MIN_FRAME_CONFIDENCE: f64 = 0.50;
pub const AHC_LINKAGE: &str = "average";
pub const AHC_METRIC: &str = "cosine";
pub const MAX_K: usize = 8;
pub const SILHOUETTE_IMPROVEMENT: f64 = 0.03;
pub const UNDEFINED_SILHOUETTE: f64 = -1.0;

const ROW_NORM_EPSILON: f64 = 1e-9;

pub(crate) fn row_norm_epsilon() -> f64 {
    ROW_NORM_EPSILON
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiarizationError {
    InvalidFrameShape {
        len: usize,
        num_classes: usize,
    },
    ShapeOverflow {
        rows: usize,
        cols: usize,
    },
    EmbeddingShapeMismatch {
        rows: usize,
        cols: usize,
        len: usize,
    },
    LabelLengthMismatch {
        intervals: usize,
        labels: usize,
    },
    /// A caller asked to stop before the next tile or merge.
    Cancelled,
    /// A bounded matrix tile could not be stored.
    MatrixStore {
        detail: String,
    },
}

impl fmt::Display for DiarizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFrameShape { len, num_classes } => write!(
                formatter,
                "frame log-prob matrix length mismatch: len={len} num_classes={num_classes}"
            ),
            Self::ShapeOverflow { rows, cols } => {
                write!(
                    formatter,
                    "row-major matrix shape overflow: rows={rows} cols={cols}"
                )
            }
            Self::EmbeddingShapeMismatch { rows, cols, len } => write!(
                formatter,
                "embedding matrix length mismatch: rows={rows} cols={cols} len={len}"
            ),
            Self::LabelLengthMismatch { intervals, labels } => write!(
                formatter,
                "sentence assignment label length mismatch: intervals={intervals} labels={labels}"
            ),
            Self::Cancelled => write!(formatter, "clustering cancelled"),
            Self::MatrixStore { detail } => write!(formatter, "clustering matrix store: {detail}"),
        }
    }
}

impl Error for DiarizationError {}

#[derive(Debug, Clone, Copy)]
pub struct FrameLogProbs<'a> {
    data: &'a [f32],
    num_classes: usize,
}

impl<'a> FrameLogProbs<'a> {
    pub fn from_row_major(data: &'a [f32], num_classes: usize) -> Result<Self, DiarizationError> {
        if num_classes == 0 || !data.len().is_multiple_of(num_classes) {
            return Err(DiarizationError::InvalidFrameShape {
                len: data.len(),
                num_classes,
            });
        }
        Ok(Self { data, num_classes })
    }

    pub fn num_frames(&self) -> usize {
        self.data.len() / self.num_classes
    }

    pub fn num_classes(&self) -> usize {
        self.num_classes
    }

    pub fn row(&self, index: usize) -> Option<&[f32]> {
        if index >= self.num_frames() {
            return None;
        }
        let start = index * self.num_classes;
        Some(&self.data[start..start + self.num_classes])
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeakerInterval {
    pub start_s: f64,
    pub end_s: f64,
    pub local_class: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SentenceTiming {
    pub start_s: Option<f64>,
    pub end_s: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterEmbeddingResult {
    pub labels: Vec<usize>,
    pub silhouette_k: Option<usize>,
    pub effective_k: Option<usize>,
}

#[cfg(all(test, feature = "full-tests"))]
#[derive(Debug, Clone, Copy, PartialEq)]
struct Merge {
    left: usize,
    right: usize,
    distance: f64,
    size: usize,
}

pub fn find_intervals(
    log_probs: FrameLogProbs<'_>,
    audio_len_samples: usize,
) -> Vec<SpeakerInterval> {
    let num_frames = log_probs.num_frames();
    let samples_per_frame = (WINDOW_S as f64 * SAMPLE_RATE as f64) / FRAMES_PER_WINDOW as f64;
    let audio_duration_s = audio_len_samples as f64 / SAMPLE_RATE as f64;

    let mut intervals = Vec::new();
    let mut run_class: Option<u8> = None;
    let mut run_start_frame = 0_usize;

    for frame in 0..num_frames {
        let row = log_probs.row(frame).expect("frame index is in range");
        let cls = argmax_raw(row);
        let confidence = softmax_confidence_at(row, cls);
        let is_single = single_speaker_class(cls) && confidence >= MIN_FRAME_CONFIDENCE;

        if is_single {
            let cls_u8 = cls as u8;
            if run_class != Some(cls_u8) {
                flush_interval(
                    &mut intervals,
                    run_class,
                    run_start_frame,
                    frame,
                    samples_per_frame,
                    audio_duration_s,
                );
                run_class = Some(cls_u8);
                run_start_frame = frame;
            }
        } else {
            flush_interval(
                &mut intervals,
                run_class,
                run_start_frame,
                frame,
                samples_per_frame,
                audio_duration_s,
            );
            run_class = None;
        }
    }

    flush_interval(
        &mut intervals,
        run_class,
        run_start_frame,
        num_frames,
        samples_per_frame,
        audio_duration_s,
    );
    intervals
}

pub fn assign_sentences(
    sentences: &[SentenceTiming],
    intervals: &[SpeakerInterval],
    global_labels: &[usize],
) -> Result<Vec<Option<usize>>, DiarizationError> {
    if intervals.len() != global_labels.len() {
        return Err(DiarizationError::LabelLengthMismatch {
            intervals: intervals.len(),
            labels: global_labels.len(),
        });
    }

    let mut result = Vec::with_capacity(sentences.len());
    for sentence in sentences {
        // Missing timestamps, non-finite timestamps, and inverted end <= start
        // boundaries all become unlabelled. Python would raise on a truly
        // non-numeric timestamp; treating it as unlabelled is deliberate
        // hardening, and the finite check is explicit because NaN comparisons
        // being all-false would otherwise make a naive port look correct by
        // accident.
        let (Some(start), Some(end)) = (sentence.start_s, sentence.end_s) else {
            result.push(None);
            continue;
        };
        if !start.is_finite() || !end.is_finite() || end <= start {
            result.push(None);
            continue;
        }

        let mut best_overlap = 0.0_f64;
        let mut best_label = None;
        for (idx, interval) in intervals.iter().enumerate() {
            let overlap = end.min(interval.end_s) - start.max(interval.start_s);
            let overlap = overlap.max(0.0);
            if overlap > best_overlap {
                best_overlap = overlap;
                best_label = Some(global_labels[idx] + 1);
            }
        }
        result.push(best_label);
    }
    Ok(result)
}

pub fn cluster_embeddings(
    embeddings: &[f32],
    rows: usize,
    cols: usize,
    n_speakers: Option<usize>,
    scratch: &std::path::Path,
) -> Result<ClusterEmbeddingResult, DiarizationError> {
    cluster_embeddings_cancellable(embeddings, rows, cols, n_speakers, scratch, &|| false)
}

/// Cluster interval rows. Cancellation is checked before each matrix tile and
/// between linkage merges. A true callback is [`DiarizationError::Cancelled`].
/// It is not an empty label list, an undefined silhouette, or a one-speaker success.
///
/// The distance and linkage matrices are the tiled store in `cluster_bound`.
/// A finished recording does not allocate `rows * rows` or `(2 * rows - 1)^2`
/// resident buffers.
pub fn cluster_embeddings_cancellable(
    embeddings: &[f32],
    rows: usize,
    cols: usize,
    n_speakers: Option<usize>,
    scratch: &std::path::Path,
    cancel: &dyn Fn() -> bool,
) -> Result<ClusterEmbeddingResult, DiarizationError> {
    validate_embedding_shape(embeddings, rows, cols)?;
    if rows == 0 {
        // The retired Python implementation had no rows==0 guard here; its behavior was accidental. The
        // analyzer short-circuits before clustering when no intervals exist, so
        // this path only protects direct callers and tests.
        return Ok(ClusterEmbeddingResult {
            labels: Vec::new(),
            silhouette_k: None,
            effective_k: None,
        });
    }
    stop_if_cancelled(cancel)?;
    let _scope = crate::cluster_bound::ClusterScope::enter(scratch);
    let mut stored = crate::cluster_bound::BoundRows::from_f32(embeddings, rows, cols, cancel)?;
    finish_cluster(&mut stored, rows, n_speakers, cancel)
}

/// Cluster embeddings already stored as little-endian `f32` rows.
///
/// One row is normalized at a time. The quadratic matrices use the same tiled
/// store as [`cluster_embeddings_cancellable`].
pub fn cluster_embeddings_f32le(
    path: &std::path::Path,
    rows: usize,
    cols: usize,
    n_speakers: Option<usize>,
    scratch: &std::path::Path,
    cancel: &dyn Fn() -> bool,
) -> Result<ClusterEmbeddingResult, DiarizationError> {
    if rows == 0 {
        return Ok(ClusterEmbeddingResult {
            labels: Vec::new(),
            silhouette_k: None,
            effective_k: None,
        });
    }
    stop_if_cancelled(cancel)?;
    let _scope = crate::cluster_bound::ClusterScope::enter(scratch);
    let mut stored = crate::cluster_bound::BoundRows::from_f32le(path, rows, cols, cancel)?;
    finish_cluster(&mut stored, rows, n_speakers, cancel)
}

/// Cluster file-backed rows and write `u32le` labels without a resident label vector.
///
/// The first cancellation poll is this function's first check, before any row
/// is normalized. Linkage flags, sizes, merges, and membership stay in the
/// private work directory under [`crate::cluster_bound::CLUSTER_LINKAGE_RESIDENT_BYTES`].
pub fn cluster_embeddings_f32le_into(
    path: &std::path::Path,
    rows: usize,
    cols: usize,
    n_speakers: Option<usize>,
    labels_out: &std::path::Path,
    scratch: &std::path::Path,
    cancel: &dyn Fn() -> bool,
) -> Result<(), DiarizationError> {
    if rows == 0 {
        return Ok(());
    }
    // Named resident ceiling for flags, sizes, one merge, member buffers, and labels.
    let _linkage_resident_bytes = crate::cluster_bound::CLUSTER_LINKAGE_RESIDENT_BYTES;
    stop_if_cancelled(cancel)?;
    let _scope = crate::cluster_bound::ClusterScope::enter(scratch);
    let mut stored = crate::cluster_bound::BoundRows::from_f32le(path, rows, cols, cancel)?;
    let (mut labels, _silhouette_k, _effective_k) =
        finish_cluster_labels(&mut stored, rows, n_speakers, cancel)?;
    labels.write_to(labels_out, cancel)
}

fn finish_cluster(
    stored: &mut crate::cluster_bound::BoundRows,
    rows: usize,
    n_speakers: Option<usize>,
    cancel: &dyn Fn() -> bool,
) -> Result<ClusterEmbeddingResult, DiarizationError> {
    let (mut labels, silhouette_k, effective_k) =
        finish_cluster_labels(stored, rows, n_speakers, cancel)?;
    Ok(ClusterEmbeddingResult {
        labels: labels.read_labels(cancel)?,
        silhouette_k,
        effective_k,
    })
}

fn finish_cluster_labels(
    stored: &mut crate::cluster_bound::BoundRows,
    rows: usize,
    n_speakers: Option<usize>,
    cancel: &dyn Fn() -> bool,
) -> Result<
    (
        crate::cluster_bound::LabelFile,
        Option<usize>,
        Option<usize>,
    ),
    DiarizationError,
> {
    let (requested_k, silhouette_k) = match n_speakers {
        Some(k) => (k, None),
        None => {
            let selected = select_k_for_stored_rows(stored, rows, cancel)?;
            (selected, Some(selected))
        }
    };
    let k = clamp_speaker_count(requested_k, rows);
    let labels = if k <= 1 {
        crate::cluster_bound::LabelFile::zeros(rows)?
    } else {
        ahc_labels_from_stored_rows(stored, rows, k, cancel)?
    };
    Ok((labels, silhouette_k, Some(k)))
}

pub fn normalize_embedding_rows(
    embeddings: &[f32],
    rows: usize,
    cols: usize,
) -> Result<Vec<f64>, DiarizationError> {
    validate_embedding_shape(embeddings, rows, cols)?;
    let mut out = Vec::with_capacity(embeddings.len());
    for row in embeddings.chunks(cols) {
        let norm = row
            .iter()
            .map(|value| {
                let value = f64::from(*value);
                value * value
            })
            .sum::<f64>()
            .sqrt();
        let denom = if norm > ROW_NORM_EPSILON { norm } else { 1.0 };
        out.extend(row.iter().map(|value| f64::from(*value) / denom));
    }
    Ok(out)
}

pub fn silhouette_score(
    embeddings: &[f32],
    rows: usize,
    cols: usize,
    labels: &[usize],
) -> Result<f64, DiarizationError> {
    if rows != labels.len() {
        return Err(DiarizationError::LabelLengthMismatch {
            intervals: rows,
            labels: labels.len(),
        });
    }
    let mut stored = crate::cluster_bound::BoundRows::from_f32(embeddings, rows, cols, &|| false)?;
    let mut labels = crate::cluster_bound::LabelFile::from_slice(labels, &|| false)?;
    silhouette_score_stored_rows(&mut stored, rows, &mut labels, &|| false)
}

pub fn select_k_from_silhouette_curve(curve: &[(usize, f64)]) -> usize {
    let mut best_k = 1_usize;
    let mut best_s = UNDEFINED_SILHOUETTE;
    for (k, score) in curve {
        let score = if score.is_finite() {
            *score
        } else {
            UNDEFINED_SILHOUETTE
        };
        // This is deliberately not an argmax. The 0.03 improvement threshold is
        // calibrated behavior, and k_selection_divergence pins the real rule at
        // k=2 while a plain argmax over the same fixture curve returns k=8.
        if score > best_s + SILHOUETTE_IMPROVEMENT {
            best_s = score;
            best_k = *k;
        }
    }
    best_k
}

fn flush_interval(
    intervals: &mut Vec<SpeakerInterval>,
    run_class: Option<u8>,
    run_start_frame: usize,
    end_frame: usize,
    samples_per_frame: f64,
    audio_duration_s: f64,
) {
    let Some(local_class) = run_class else {
        return;
    };
    let start_s = (run_start_frame as f64 * samples_per_frame) / SAMPLE_RATE as f64;
    let end_s = ((end_frame as f64 * samples_per_frame) / SAMPLE_RATE as f64).min(audio_duration_s);
    if end_s - start_s >= MIN_INTERVAL_S {
        intervals.push(SpeakerInterval {
            start_s,
            end_s,
            local_class,
        });
    }
}

fn argmax_raw(row: &[f32]) -> usize {
    let mut best_idx = 0_usize;
    let mut best_value = row[0];
    for (idx, value) in row.iter().enumerate().skip(1) {
        if *value > best_value {
            best_value = *value;
            best_idx = idx;
        }
    }
    best_idx
}

fn softmax_confidence_at(row: &[f32], index: usize) -> f64 {
    let max = row
        .iter()
        .fold(f64::NEG_INFINITY, |best, value| best.max(f64::from(*value)));
    let denom = row
        .iter()
        .map(|value| (f64::from(*value) - max).exp())
        .sum::<f64>();
    (f64::from(row[index]) - max).exp() / denom
}

fn single_speaker_class(cls: usize) -> bool {
    SINGLE_SPEAKER_CLASSES
        .iter()
        .any(|single| usize::from(*single) == cls)
}

fn validate_embedding_shape(
    embeddings: &[f32],
    rows: usize,
    cols: usize,
) -> Result<(), DiarizationError> {
    let expected = rows
        .checked_mul(cols)
        .ok_or(DiarizationError::ShapeOverflow { rows, cols })?;
    if embeddings.len() != expected {
        return Err(DiarizationError::EmbeddingShapeMismatch {
            rows,
            cols,
            len: embeddings.len(),
        });
    }
    Ok(())
}

fn clamp_speaker_count(k: usize, n: usize) -> usize {
    std::cmp::max(1, std::cmp::min(k, if n > 1 { n - 1 } else { 1 }))
}

fn select_k_for_stored_rows(
    stored: &mut crate::cluster_bound::BoundRows,
    rows: usize,
    cancel: &dyn Fn() -> bool,
) -> Result<usize, DiarizationError> {
    let effective_max = MAX_K.min(rows.saturating_sub(1));
    if effective_max < 2 {
        return Ok(1);
    }
    let mut curve = Vec::with_capacity(effective_max - 1);
    for k in 2..=effective_max {
        stop_if_cancelled(cancel)?;
        let mut labels = ahc_labels_from_stored_rows(stored, rows, k, cancel)?;
        let score = silhouette_score_stored_rows(stored, rows, &mut labels, cancel)?;
        curve.push((k, score));
    }
    Ok(select_k_from_silhouette_curve(&curve))
}

fn ahc_labels_from_stored_rows(
    stored: &mut crate::cluster_bound::BoundRows,
    rows: usize,
    k: usize,
    cancel: &dyn Fn() -> bool,
) -> Result<crate::cluster_bound::LabelFile, DiarizationError> {
    if rows == 0 {
        return crate::cluster_bound::LabelFile::zeros(0);
    }
    let k = clamp_speaker_count(k, rows);
    if k <= 1 {
        return crate::cluster_bound::LabelFile::zeros(rows);
    }
    let mut merges = {
        let mut tiles = crate::cluster_bound::LinkageTiles::from_unclipped(stored, rows, cancel)?;
        average_linkage_tree_from_tiles(&mut tiles, rows, cancel)?
    };
    cut_tree_canonical_labels(rows, &mut merges, k, cancel)
}

fn stop_if_cancelled(cancel: &dyn Fn() -> bool) -> Result<(), DiarizationError> {
    if cancel() {
        Err(DiarizationError::Cancelled)
    } else {
        Ok(())
    }
}

// A finished cluster keeps the unclipped cosine matrix in fixed tiles.
// Silhouette reads clipped cosine from the stored rows and does not allocate
// a second rows*rows buffer. Active flags, sizes, merges, membership, and
// labels stay in private files. Two tiles of each are resident, bounded by
// CLUSTER_LINKAGE_RESIDENT_BYTES. Unclipped linkage distance stays 1-dot.
// Silhouette stays max(1-dot, 0).

#[cfg(all(test, feature = "full-tests"))]
fn average_linkage_tree_from_distances(
    distances: &[f64],
    n: usize,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<Merge>, DiarizationError> {
    if n <= 1 {
        return Ok(Vec::new());
    }
    let scratch = tempfile::tempdir().map_err(|error| DiarizationError::MatrixStore {
        detail: error.to_string(),
    })?;
    let _scope = crate::cluster_bound::ClusterScope::enter(scratch.path());
    let mut tiles = crate::cluster_bound::LinkageTiles::from_square(distances, n, cancel)?;
    let mut log = average_linkage_tree_from_tiles(&mut tiles, n, cancel)?;
    let mut merges = Vec::with_capacity(log.len());
    for index in 0..log.len() {
        let stored = log.get(index)?;
        merges.push(Merge {
            left: stored.left,
            right: stored.right,
            distance: stored.distance,
            size: stored.size,
        });
    }
    Ok(merges)
}

fn average_linkage_tree_from_tiles(
    tiles: &mut crate::cluster_bound::LinkageTiles,
    n: usize,
    cancel: &dyn Fn() -> bool,
) -> Result<crate::cluster_bound::MergeLog, DiarizationError> {
    if n <= 1 {
        return crate::cluster_bound::MergeLog::new();
    }
    let mut nodes = crate::cluster_bound::NodeScratch::new(tiles.stride())?;
    nodes.activate_singletons(n, cancel)?;
    let mut merges = crate::cluster_bound::MergeLog::new()?;
    for step in 0..(n - 1) {
        stop_if_cancelled(cancel)?;
        let (left, right, distance) = lowest_active_pair_tiles(tiles, &mut nodes, cancel)?
            .expect("at least two active clusters remain");
        let new_id = n + step;
        let left_size = nodes.size(left, cancel)?;
        let right_size = nodes.size(right, cancel)?;
        let new_size = left_size + right_size;
        nodes.set_active(left, false, cancel)?;
        nodes.set_active(right, false, cancel)?;
        nodes.set_active(new_id, true, cancel)?;
        nodes.set_size(new_id, new_size, cancel)?;

        for other in 0..new_id {
            if other % crate::cluster_bound::CLUSTER_TILE_EDGE == 0 {
                stop_if_cancelled(cancel)?;
            }
            if !nodes.is_active(other, cancel)? {
                continue;
            }
            let merged_distance = (left_size as f64 * tiles.get(left, other, cancel)?
                + right_size as f64 * tiles.get(right, other, cancel)?)
                / new_size as f64;
            tiles.set(new_id, other, merged_distance, cancel)?;
            tiles.set(other, new_id, merged_distance, cancel)?;
        }
        merges.push(left, right, distance, new_size)?;
    }
    Ok(merges)
}

fn lowest_active_pair_tiles(
    tiles: &mut crate::cluster_bound::LinkageTiles,
    nodes: &mut crate::cluster_bound::NodeScratch,
    cancel: &dyn Fn() -> bool,
) -> Result<Option<(usize, usize, f64)>, DiarizationError> {
    let mut best: Option<(usize, usize, f64)> = None;
    let limit = nodes.nodes();
    for i in 0..limit {
        if i % crate::cluster_bound::CLUSTER_TILE_EDGE == 0 {
            stop_if_cancelled(cancel)?;
        }
        if !nodes.is_active(i, cancel)? {
            continue;
        }
        for j in (i + 1)..limit {
            if (j - i) % crate::cluster_bound::CLUSTER_TILE_EDGE == 0 {
                stop_if_cancelled(cancel)?;
            }
            if !nodes.is_active(j, cancel)? {
                continue;
            }
            let distance = tiles.get(i, j, cancel)?;
            let replace = match best {
                None => true,
                Some((best_i, best_j, best_distance)) => {
                    distance < best_distance
                        // Exact ties use the lowest (i, j) active-pair index.
                        // This can diverge from scipy's nn-chain order only on
                        // exact distance ties; scipy documents its own tie
                        // choice as implementation-defined.
                        || (distance == best_distance && (i, j) < (best_i, best_j))
                }
            };
            if replace {
                best = Some((i, j, distance));
            }
        }
    }
    Ok(best)
}

fn cut_tree_canonical_labels(
    n: usize,
    merges: &mut crate::cluster_bound::MergeLog,
    k: usize,
    cancel: &dyn Fn() -> bool,
) -> Result<crate::cluster_bound::LabelFile, DiarizationError> {
    if n == 0 {
        return crate::cluster_bound::LabelFile::zeros(0);
    }
    if k <= 1 {
        return crate::cluster_bound::LabelFile::zeros(n);
    }

    let mut lists = crate::cluster_bound::MemberLists::singletons(n, cancel)?;
    let taken = n - k;
    if taken > merges.len() {
        return Err(DiarizationError::MatrixStore {
            detail: "linkage ended before the requested cut".to_owned(),
        });
    }
    for step in 0..taken {
        stop_if_cancelled(cancel)?;
        let merge = merges.get(step)?;
        if merge.distance.is_nan() {
            return Err(DiarizationError::MatrixStore {
                detail: "linkage distance is not a number".to_owned(),
            });
        }
        lists.merge_sorted(merge.left, merge.right, n + step, cancel)?;
        if lists.count(n + step, cancel)? != merge.size {
            return Err(DiarizationError::MatrixStore {
                detail: "merge size does not match membership".to_owned(),
            });
        }
    }

    // Labels follow the ascending smallest member. This is deliberately not
    // scipy _hc_cut's numbering: those label values are merge-order artifacts,
    // while only the partition is well-defined. Each choice scans the remaining
    // clusters and keeps two member tiles, so k is not a resident vector.
    let mut labels = crate::cluster_bound::LabelFile::zeros(n)?;
    let max_nodes = n * 2 - 1;
    for label in 0..k {
        stop_if_cancelled(cancel)?;
        let mut best: Option<(u64, usize)> = None;
        for node in 0..max_nodes {
            if node % crate::cluster_bound::CLUSTER_TILE_EDGE == 0 {
                stop_if_cancelled(cancel)?;
            }
            if lists.count(node, cancel)? == 0 {
                continue;
            }
            let first = lists.first(node, cancel)?;
            let replace = match best {
                None => true,
                Some((best_first, best_node)) => {
                    first < best_first || (first == best_first && node < best_node)
                }
            };
            if replace {
                best = Some((first, node));
            }
        }
        let Some((_, node)) = best else {
            return Err(DiarizationError::MatrixStore {
                detail: "cut left fewer clusters than requested".to_owned(),
            });
        };
        lists.for_each(node, cancel, |member| {
            let index = usize::try_from(member)
                .map_err(|_| DiarizationError::ShapeOverflow { rows: n, cols: 1 })?;
            labels.set(index, label, cancel)
        })?;
        lists.clear(node, cancel)?;
    }
    Ok(labels)
}

fn silhouette_score_stored_rows(
    stored: &mut crate::cluster_bound::BoundRows,
    rows: usize,
    labels: &mut crate::cluster_bound::LabelFile,
    cancel: &dyn Fn() -> bool,
) -> Result<f64, DiarizationError> {
    if rows != labels.len() {
        return Ok(UNDEFINED_SILHOUETTE);
    }
    let unique = labels.unique(cancel)?;
    if unique.len() < 2 || rows <= unique.len() {
        return Ok(UNDEFINED_SILHOUETTE);
    }

    // Clipped cosine is max(1 - dot, 0), matching _silhouette in diarize.py.
    // It is not the unclipped distance stored for agglomerative clustering.
    let mut total = 0.0_f64;
    for idx in 0..rows {
        if idx % crate::cluster_bound::CLUSTER_TILE_EDGE == 0 {
            stop_if_cancelled(cancel)?;
        }
        let own_label = labels.get(idx, cancel)?;
        let own_count = labels.count_label(own_label, Some(idx), cancel)?;
        if own_count == 0 {
            // sklearn computes the singleton intra-cluster mean as 0/0, then
            // np.nan_to_num maps that sample's silhouette contribution to 0.0.
            // The singleton contribution is therefore independent of b.
            continue;
        }
        let mut sum = 0.0_f64;
        for other in 0..rows {
            if other % crate::cluster_bound::CLUSTER_TILE_EDGE == 0 {
                stop_if_cancelled(cancel)?;
            }
            if other != idx && labels.get(other, cancel)? == own_label {
                sum += stored.clipped(idx, other)?;
            }
        }
        let a = sum / own_count as f64;

        let mut b = f64::INFINITY;
        for label in &unique {
            if *label == own_label {
                continue;
            }
            let mut count = 0_usize;
            let mut other_sum = 0.0_f64;
            for other in 0..rows {
                if other % crate::cluster_bound::CLUSTER_TILE_EDGE == 0 {
                    stop_if_cancelled(cancel)?;
                }
                if labels.get(other, cancel)? == *label {
                    count += 1;
                    other_sum += stored.clipped(idx, other)?;
                }
            }
            if count > 0 {
                b = b.min(other_sum / count as f64);
            }
        }

        let denom = a.max(b);
        let sample_score = if denom > 0.0 { (b - a) / denom } else { 0.0 };
        total += sample_score;
    }
    Ok(total / rows as f64)
}

#[cfg(test)]
fn silhouette_score_from_distances(distances: &[f64], rows: usize, labels: &[usize]) -> f64 {
    if rows != labels.len() {
        return UNDEFINED_SILHOUETTE;
    }
    let unique = unique_labels(labels);
    if unique.len() < 2 || rows <= unique.len() {
        return UNDEFINED_SILHOUETTE;
    }

    let mut total = 0.0_f64;
    for idx in 0..rows {
        let own_label = labels[idx];
        let own_count = labels
            .iter()
            .enumerate()
            .filter(|(other, label)| *other != idx && **label == own_label)
            .count();
        if own_count == 0 {
            // sklearn computes the singleton intra-cluster mean as 0/0, then
            // np.nan_to_num maps that sample's silhouette contribution to 0.0.
            // The singleton contribution is therefore independent of b.
            continue;
        }
        let a = labels
            .iter()
            .enumerate()
            .filter(|(other, label)| *other != idx && **label == own_label)
            .map(|(other, _label)| distances[idx * rows + other])
            .sum::<f64>()
            / own_count as f64;

        let mut b = f64::INFINITY;
        for label in &unique {
            if *label == own_label {
                continue;
            }
            let mut count = 0_usize;
            let mut sum = 0.0_f64;
            for (other, other_label) in labels.iter().enumerate() {
                if *other_label == *label {
                    count += 1;
                    sum += distances[idx * rows + other];
                }
            }
            if count > 0 {
                b = b.min(sum / count as f64);
            }
        }

        let denom = a.max(b);
        let sample_score = if denom > 0.0 { (b - a) / denom } else { 0.0 };
        total += sample_score;
    }
    total / rows as f64
}

#[cfg(test)]
fn unique_labels(labels: &[usize]) -> Vec<usize> {
    let mut labels = labels.to_vec();
    labels.sort_unstable();
    labels.dedup();
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const STAGE_FIXTURE: &str = include_str!("../../../fixtures/speaker_stage_boundaries.json");
    const CLUSTER_SCORE_ABS_TOLERANCE: f64 = 1e-6;

    #[test]
    fn diarization_constants_match_stage_fixture_identity() {
        let fixture = fixture();
        let constants = &fixture["identity"]["source_constants"]["diarize"];

        assert_eq!(
            constants["SAMPLE_RATE"].as_u64().unwrap(),
            u64::from(SAMPLE_RATE)
        );
        assert_eq!(constants["WINDOW_S"].as_u64().unwrap(), u64::from(WINDOW_S));
        assert_eq!(constants["STRIDE_S"].as_u64().unwrap(), u64::from(STRIDE_S));
        assert_eq!(
            constants["FRAMES_PER_WINDOW"].as_u64().unwrap(),
            FRAMES_PER_WINDOW as u64
        );
        assert_eq!(
            constants["MIN_INTERVAL_S"].as_f64().unwrap(),
            MIN_INTERVAL_S
        );
        assert_eq!(
            constants["MIN_FRAME_CONFIDENCE"].as_f64().unwrap(),
            MIN_FRAME_CONFIDENCE
        );
        assert_eq!(constants["AHC_LINKAGE"].as_str().unwrap(), AHC_LINKAGE);
        assert_eq!(constants["AHC_METRIC"].as_str().unwrap(), AHC_METRIC);
        assert_eq!(constants["MAX_K"].as_u64().unwrap(), MAX_K as u64);
        assert_eq!(
            constants["SILHOUETTE_IMPROVEMENT"].as_f64().unwrap(),
            SILHOUETTE_IMPROVEMENT
        );
        let classes: Vec<u8> = constants["SINGLE_SPEAKER_CLASSES"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_u64().unwrap() as u8)
            .collect();
        assert_eq!(classes, SINGLE_SPEAKER_CLASSES);
    }

    #[test]
    fn interval_finding_keeps_thirty_frames_and_drops_twenty_nine() {
        // Fixture cannot supply this because it records derived intervals, not
        // the raw per-frame log-prob matrix needed to exercise the flush code.
        let kept = intervals_for_run(30, WINDOW_S as usize * SAMPLE_RATE as usize);
        let dropped = intervals_for_run(29, WINDOW_S as usize * SAMPLE_RATE as usize);

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].start_s, 0.0);
        assert_eq!(kept[0].end_s.to_bits(), 0x3fe0_4c7e_e9d5_67e0);
        assert_eq!(kept[0].local_class, 1);
        assert!(dropped.is_empty());
    }

    #[test]
    fn interval_finding_flushes_active_class_change_and_active_end() {
        // Fixture cannot supply this because its interval case has one active
        // class followed by silence, not one single-speaker class changing into
        // another or an active run ending at loop completion.
        let mut classes = vec![1_u8; 35];
        classes.extend(std::iter::repeat_n(2_u8, 35));
        let data = dominant_log_probs(&classes);
        let log_probs = FrameLogProbs::from_row_major(&data, 7).expect("shape");

        let intervals = find_intervals(log_probs, WINDOW_S as usize * SAMPLE_RATE as usize);

        assert_eq!(
            intervals
                .iter()
                .map(|interval| interval.local_class)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn interval_finding_clamps_every_flush_to_audio_duration() {
        // Fixture cannot supply this because its audio length is a full pyannote
        // window, so no flush end crosses the true audio duration.
        let classes = vec![1_u8; 100];
        let data = dominant_log_probs(&classes);
        let log_probs = FrameLogProbs::from_row_major(&data, 7).expect("shape");

        let intervals = find_intervals(log_probs, 10_000);

        assert_eq!(intervals.len(), 1);
        assert_eq!(intervals[0].end_s, 10_000.0 / SAMPLE_RATE as f64);
    }

    #[test]
    fn interval_finding_uses_raw_argmax_but_softmax_confidence() {
        // Fixture cannot supply this because it does not record confidence
        // arrays; this case would be wrongly accepted if raw logit magnitude
        // were used as confidence.
        let row = [0.59_f32, 0.60, 0.59, 0.59, 0.59, 0.59, 0.59];
        let mut data = Vec::new();
        for _ in 0..40 {
            data.extend(row);
        }
        let log_probs = FrameLogProbs::from_row_major(&data, 7).expect("shape");

        let intervals = find_intervals(log_probs, WINDOW_S as usize * SAMPLE_RATE as usize);

        assert!(intervals.is_empty());
        assert_eq!(argmax_raw(&row), 1);
        assert!(softmax_confidence_at(&row, 1) < MIN_FRAME_CONFIDENCE);
    }

    #[test]
    fn k_selection_replays_fixture_curves_and_counter_argmaxes() {
        let fixture = fixture();
        let divergence = curve_at(&fixture["k_selection_divergence"]["case"]);
        let base = curve_at(&fixture["clustering_input_perturbation"]["base"]);
        let perturbed = curve_at(&fixture["clustering_input_perturbation"]["perturbed"]);

        assert_eq!(select_k_from_silhouette_curve(&divergence), 2);
        assert_eq!(select_k_from_silhouette_curve(&base), 3);
        assert_eq!(select_k_from_silhouette_curve(&perturbed), 4);
        assert_eq!(plain_argmax_k(&divergence), 8);
        assert_eq!(plain_argmax_k(&base), 5);
        assert_eq!(plain_argmax_k(&perturbed), 5);

        // Precision is part of the behavior: k=3 clears by +1.5064e-4 for
        // clustering_input_perturbation.base and misses by -5.1214e-5 for
        // .perturbed. In f32 this decision could flip, which on real audio
        // means a different speaker count.
        assert_close(k3_margin(&base), 0.000_150_622_129_440_308_73, 1e-15);
        assert_close(k3_margin(&perturbed), -5.121_409_893_035_777_6e-05, 1e-15);
    }

    #[test]
    fn silhouette_tolerance_violation_reports_some_error() {
        // This starts from a fixture score moved deliberately beyond tolerance,
        // so it is a negative test of the comparison helper itself rather than
        // a fixture-backed assertion.
        let fixture = fixture();
        let curve = curve_at(&fixture["clustering_input_perturbation"]["base"]);
        let expected = curve[0].1;
        let actual = expected + CLUSTER_SCORE_ABS_TOLERANCE * 2.0;

        assert!(score_comparison_error(actual, expected, CLUSTER_SCORE_ABS_TOLERANCE).is_some());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn ahc_recovers_well_separated_known_partition_at_k_three() {
        // Fixture cannot supply this because the committed stage fixture has no
        // raw clustering rows, only derived labels and scores.
        let embeddings = [
            1.0_f32, 0.0, 0.0, 0.99, 0.01, 0.0, 0.0, 1.0, 0.0, 0.01, 0.99, 0.0, 0.0, 0.0, 1.0, 0.0,
            0.01, 0.99,
        ];

        let scratch = tempfile::tempdir().expect("cluster scratch");
        let result =
            cluster_embeddings(&embeddings, 6, 3, Some(3), scratch.path()).expect("cluster labels");

        assert_same_partition(&result.labels, &[0, 0, 1, 1, 2, 2]);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn ahc_hand_walked_average_linkage_merge_sequence() {
        // Fixture cannot supply this because the arithmetic oracle is a
        // hand-built distance matrix, not recoverable from committed rows.
        //
        // d01=0.1 and d23=0.2 merge first. Then d({0,1},2)=(0.8+0.7)/2=0.75,
        // d({0,1},3)=(0.6+1.0)/2=0.8, and after {2,3} merges the final
        // distance is (0.75+0.8)/2=0.775.
        let distances = square_distances(&[
            [0.0, 0.1, 0.8, 0.6],
            [0.1, 0.0, 0.7, 1.0],
            [0.8, 0.7, 0.0, 0.2],
            [0.6, 1.0, 0.2, 0.0],
        ]);

        let merges = average_linkage_tree_from_distances(&distances, 4, &|| false)
            .expect("uninterrupted linkage");

        assert_merge(&merges[0], 0, 1, 0.1, 2);
        assert_merge(&merges[1], 2, 3, 0.2, 2);
        assert_merge(&merges[2], 4, 5, 0.775, 4);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn ahc_exact_ties_use_lowest_active_pair_index() {
        // Fixture cannot supply this because the tie rule is intentionally
        // hand-built; production fixture rows are absent and real embeddings
        // should not depend on exact ties.
        let distances = square_distances(&[
            [0.0, 1.0, 1.0, 1.0],
            [1.0, 0.0, 1.0, 1.0],
            [1.0, 1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0, 0.0],
        ]);

        let merges = average_linkage_tree_from_distances(&distances, 4, &|| false)
            .expect("uninterrupted linkage");

        assert_merge(&merges[0], 0, 1, 1.0, 2);
        assert_merge(&merges[1], 2, 3, 1.0, 2);
        assert_merge(&merges[2], 4, 5, 1.0, 4);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn ahc_partition_nesting_across_k_two_through_five_is_property_based() {
        // Fixture cannot supply this because it records nested partitions but
        // not the raw rows. This is property-based rather than oracle-based:
        // each k+1 partition must refine the k partition, a universal property
        // of any correct agglomerative implementation, so it constrains the
        // whole ladder without needing a committed oracle. The fixture's own
        // recorded curves satisfy this property.
        let embeddings = [
            1.0_f32, 0.0, 0.0, 0.97, 0.03, 0.0, 0.0, 1.0, 0.0, 0.02, 0.98, 0.0, 0.0, 0.0, 1.0, 0.0,
            0.03, 0.97, 0.7, 0.7, 0.0, 0.7, 0.0, 0.7,
        ];

        let scratch = tempfile::tempdir().expect("cluster scratch");
        let mut previous = cluster_embeddings(&embeddings, 8, 3, Some(2), scratch.path())
            .expect("k=2")
            .labels;
        for k in 3..=5 {
            let current = cluster_embeddings(&embeddings, 8, 3, Some(k), scratch.path())
                .expect("labels")
                .labels;
            assert!(partition_refines(&current, &previous));
            previous = current;
        }
    }

    #[test]
    fn silhouette_hand_computable_value_matches_to_tolerance() {
        // Fixture cannot supply this because its silhouette scores depend on
        // missing raw clustering rows; this matrix has a closed-form mean.
        let distances = square_distances(&[
            [0.0, 0.2, 1.0, 1.0],
            [0.2, 0.0, 1.0, 1.0],
            [1.0, 1.0, 0.0, 0.2],
            [1.0, 1.0, 0.2, 0.0],
        ]);

        let score = silhouette_score_from_distances(&distances, 4, &[0, 0, 1, 1]);

        assert_close(score, 0.8, 1e-6);
    }

    #[test]
    fn silhouette_singleton_cluster_contributes_zero_to_mean() {
        // Fixture cannot supply this because its silhouette scores depend on
        // missing raw clustering rows. sklearn's silhouette_samples returns
        // [0.8, 0.8, 0.8, 0.8, 0.0] here: the singleton's intra-cluster mean is
        // 0/0 and np.nan_to_num makes that sample contribution 0.0. The existing
        // 0.8 two-cluster test cannot catch this because both clusters have two
        // or more members.
        let distances = square_distances(&[
            [0.0, 0.2, 0.2, 0.2, 1.0],
            [0.2, 0.0, 0.2, 0.2, 1.0],
            [0.2, 0.2, 0.0, 0.2, 1.0],
            [0.2, 0.2, 0.2, 0.0, 1.0],
            [1.0, 1.0, 1.0, 1.0, 0.0],
        ]);

        let score = silhouette_score_from_distances(&distances, 5, &[0, 0, 0, 0, 1]);

        assert_close(score, 0.64, 1e-6);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn zero_norm_embedding_row_passes_through_and_clusters_without_error() {
        // Fixture cannot supply this because it has no raw clustering rows and
        // no degenerate zero-norm embedding case.
        let embeddings = [0.0_f32, 0.0, 1.0, 0.0, 0.0, 1.0];

        let normalized = normalize_embedding_rows(&embeddings, 3, 2).expect("normalized");
        let scratch = tempfile::tempdir().expect("cluster scratch");
        let result =
            cluster_embeddings(&embeddings, 3, 2, Some(2), scratch.path()).expect("labels");

        assert_eq!(&normalized[0..2], &[0.0, 0.0]);
        assert_eq!(result.labels.len(), 3);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn cluster_embeddings_reports_selection_for_zero_rows() {
        // Fixture cannot supply this because it has only populated clustering
        // curves, not empty or single-row degenerate inputs.
        let scratch = tempfile::tempdir().expect("cluster scratch");
        let empty = cluster_embeddings(&[], 0, 2, None, scratch.path()).expect("empty labels");

        assert_eq!(empty.labels, Vec::<usize>::new());
        assert_eq!(empty.silhouette_k, None);
        assert_eq!(empty.effective_k, None);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn cluster_embeddings_reports_selection_for_one_row() {
        let scratch = tempfile::tempdir().expect("cluster scratch");
        let result =
            cluster_embeddings(&[1.0_f32, 0.0], 1, 2, None, scratch.path()).expect("single labels");

        assert_eq!(result.labels, vec![0]);
        assert_eq!(result.silhouette_k, Some(1));
        assert_eq!(result.effective_k, Some(1));
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn cluster_embeddings_reports_selection_for_two_rows() {
        let scratch = tempfile::tempdir().expect("cluster scratch");
        let result = cluster_embeddings(&[1.0_f32, 0.0, 0.0, 1.0], 2, 2, None, scratch.path())
            .expect("two-row labels");

        assert_eq!(result.labels, vec![0, 0]);
        assert_eq!(result.silhouette_k, Some(1));
        assert_eq!(result.effective_k, Some(1));
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn cluster_embeddings_reports_selection_for_three_rows() {
        let scratch = tempfile::tempdir().expect("cluster scratch");
        let result = cluster_embeddings(
            &[1.0_f32, 0.0, 0.0, 1.0, 0.8, 0.2],
            3,
            2,
            None,
            scratch.path(),
        )
        .expect("three-row labels");

        assert_eq!(result.labels.len(), 3);
        assert_eq!(result.silhouette_k, Some(2));
        assert_eq!(result.effective_k, Some(2));
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn cluster_embeddings_cancellable_matches_an_uninterrupted_call() {
        let never = || false;
        let scratch = tempfile::tempdir().expect("cluster scratch");
        let compare = |embeddings: &[f32], rows: usize| {
            let plain =
                cluster_embeddings(embeddings, rows, 2, None, scratch.path()).expect("labels");
            let cancellable =
                cluster_embeddings_cancellable(embeddings, rows, 2, None, scratch.path(), &never)
                    .expect("labels");
            assert_eq!(plain.labels, cancellable.labels);
            assert_eq!(plain.silhouette_k, cancellable.silhouette_k);
            assert_eq!(plain.effective_k, cancellable.effective_k);
        };
        compare(&[1.0_f32, 0.0, 0.0, 1.0, 0.8, 0.2], 3);
        compare(&[1.0_f32, 0.0, 0.0, 1.0], 2);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn cluster_embeddings_cancellable_stops_on_the_first_poll() {
        let scratch = tempfile::tempdir().expect("cluster scratch");
        let error = cluster_embeddings_cancellable(
            &[1.0_f32, 0.0, 0.0, 1.0, 0.8, 0.2],
            3,
            2,
            None,
            scratch.path(),
            &|| true,
        )
        .expect_err("first poll cancels");
        assert_eq!(error, DiarizationError::Cancelled);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn cluster_embeddings_cancellable_stops_on_a_later_poll() {
        let polls = std::cell::Cell::new(0_usize);
        let cancel = || {
            let seen = polls.get();
            polls.set(seen + 1);
            seen > 0
        };
        let scratch = tempfile::tempdir().expect("cluster scratch");
        let error = cluster_embeddings_cancellable(
            &[1.0_f32, 0.0, 0.0, 1.0, 0.8, 0.2],
            3,
            2,
            None,
            scratch.path(),
            &cancel,
        )
        .expect_err("later poll cancels");
        assert_eq!(error, DiarizationError::Cancelled);
        assert!(polls.get() >= 2, "polls={}", polls.get());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn cluster_embeddings_keeps_labels_inside_the_tiled_resident_cap() {
        let rows = 96;
        let cols = 2;
        let mut embeddings = Vec::with_capacity(rows * cols);
        for index in 0..rows {
            let angle = (index as f32) * 0.2;
            embeddings.push(angle.cos());
            embeddings.push(angle.sin());
        }
        let _ = crate::cluster_bound::take_cluster_tile_work_units();
        let scratch = tempfile::tempdir().expect("cluster scratch");
        let result =
            cluster_embeddings(&embeddings, rows, cols, Some(3), scratch.path()).expect("labels");
        let tile_units = crate::cluster_bound::take_cluster_tile_work_units();

        assert_eq!(result.labels.len(), rows);
        assert!(tile_units > 1, "tile work units={tile_units}");
        assert!(
            crate::cluster_bound::cluster_matrix_resident_bytes(cols)
                < rows * rows * std::mem::size_of::<f64>()
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn cluster_embeddings_cancellable_stops_inside_a_later_tile() {
        let rows = 96;
        let cols = 2;
        let mut embeddings = Vec::with_capacity(rows * cols);
        for index in 0..rows {
            let angle = (index as f32) * 0.2;
            embeddings.push(angle.cos());
            embeddings.push(angle.sin());
        }
        let polls = std::cell::Cell::new(0_usize);
        let cancel = || {
            let seen = polls.get();
            polls.set(seen + 1);
            // Opening check, then row normalization at 0, 32, and 64. The
            // following polls are linkage tiles.
            seen > 4
        };

        let scratch = tempfile::tempdir().expect("cluster scratch");
        let error = cluster_embeddings_cancellable(
            &embeddings,
            rows,
            cols,
            Some(3),
            scratch.path(),
            &cancel,
        )
        .expect_err("later tile cancels");

        assert_eq!(error, DiarizationError::Cancelled);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn cluster_embeddings_gives_one_label_to_the_same_speaker_across_units() {
        let scratch = tempfile::tempdir().expect("cluster scratch");
        let result = cluster_embeddings(
            &[1.0_f32, 0.0, 0.0, 1.0, 1.0, 0.0],
            3,
            2,
            None,
            scratch.path(),
        )
        .expect("same-speaker labels");

        assert_eq!(result.labels[0], result.labels[2]);
        assert_ne!(result.labels[0], result.labels[1]);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn cluster_embeddings_reports_requested_speaker_count_without_silhouette() {
        // Fixture cannot supply this because selected fixture k values are
        // already in range; this directly pins the n <= k clamp degenerate.
        assert_eq!(clamp_speaker_count(99, 3), 2);
        assert_eq!(clamp_speaker_count(0, 3), 1);
        assert_eq!(clamp_speaker_count(4, 0), 1);

        let embeddings = [1.0_f32, 0.0, 0.0, 1.0, 0.8, 0.2];
        let scratch = tempfile::tempdir().expect("cluster scratch");
        let result =
            cluster_embeddings(&embeddings, 3, 2, Some(99), scratch.path()).expect("labels");

        assert_eq!(unique_labels(&result.labels).len(), 2);
        assert_eq!(result.silhouette_k, None);
        assert_eq!(result.effective_k, Some(2));
    }

    #[test]
    fn sentence_assignment_label_base_is_one_indexed() {
        // Fixture cannot supply this exact assertion name; criterion 9 requires
        // pinning the one-indexed label base directly rather than indirectly.
        let intervals = [SpeakerInterval {
            start_s: 0.0,
            end_s: 1.0,
            local_class: 1,
        }];
        let sentences = [SentenceTiming {
            start_s: Some(0.1),
            end_s: Some(0.2),
        }];

        let labels = assign_sentences(&sentences, &intervals, &[0]).expect("labels");

        assert_eq!(labels, vec![Some(1)]);
    }

    #[test]
    fn sentence_assignment_matches_fixture_assigned_sentences() {
        let fixture = fixture();
        let interval_boundary = &fixture["interval_boundary"];
        let interval = speaker_interval_at(&interval_boundary["kept_at_30_frames"]["intervals"][0]);
        let expected = assigned_sentences_at(&interval_boundary["assigned_sentences"]);
        let sentences = [
            SentenceTiming {
                start_s: Some(interval.start_s),
                end_s: Some((interval.start_s + interval.end_s) / 2.0),
            },
            SentenceTiming {
                start_s: Some(interval.end_s + 1.0),
                end_s: Some(interval.end_s + 2.0),
            },
        ];

        let labels = assign_sentences(&sentences, &[interval], &[0]).expect("labels");

        assert_eq!(labels, expected);
    }

    #[test]
    fn sentence_assignment_equal_overlap_takes_first_interval_label() {
        // Fixture cannot supply this because its only assignment case has a
        // single interval. The tie rule is strict >, not >=, so equal overlap
        // leaves the earlier interval's label in place; zero overlap never
        // improves on the initial 0.0 and remains unlabelled.
        let intervals = [
            SpeakerInterval {
                start_s: 0.0,
                end_s: 1.0,
                local_class: 1,
            },
            SpeakerInterval {
                start_s: 1.0,
                end_s: 2.0,
                local_class: 2,
            },
        ];
        let sentences = [
            SentenceTiming {
                start_s: Some(0.5),
                end_s: Some(1.5),
            },
            SentenceTiming {
                start_s: Some(2.0),
                end_s: Some(2.5),
            },
        ];

        let labels = assign_sentences(&sentences, &intervals, &[4, 8]).expect("labels");

        assert_eq!(labels, vec![Some(5), None]);
    }

    #[test]
    fn sentence_assignment_invalid_timestamps_are_unlabelled() {
        // Fixture cannot supply this because its assigned-sentence case has only
        // valid numeric timestamps.
        let intervals = [SpeakerInterval {
            start_s: 0.0,
            end_s: 1.0,
            local_class: 1,
        }];
        let sentences = [
            SentenceTiming {
                start_s: None,
                end_s: Some(0.2),
            },
            SentenceTiming {
                start_s: Some(f64::NAN),
                end_s: Some(0.2),
            },
            SentenceTiming {
                start_s: Some(0.3),
                end_s: Some(0.2),
            },
            SentenceTiming {
                start_s: Some(0.1),
                end_s: Some(0.2),
            },
        ];

        let labels = assign_sentences(&sentences, &intervals, &[0]).expect("labels");

        assert_eq!(labels, vec![None, None, None, Some(1)]);
    }

    fn fixture() -> Value {
        serde_json::from_str(STAGE_FIXTURE).expect("stage fixture")
    }

    fn curve_at(case: &Value) -> Vec<(usize, f64)> {
        case["curve"]
            .as_array()
            .expect("curve")
            .iter()
            .map(|row| {
                (
                    row["k"].as_u64().expect("k") as usize,
                    row["silhouette"].as_f64().expect("silhouette"),
                )
            })
            .collect()
    }

    fn speaker_interval_at(value: &Value) -> SpeakerInterval {
        SpeakerInterval {
            start_s: value["start_s"].as_f64().expect("start_s"),
            end_s: value["end_s"].as_f64().expect("end_s"),
            local_class: value["local_class"].as_u64().expect("local_class") as u8,
        }
    }

    fn assigned_sentences_at(value: &Value) -> Vec<Option<usize>> {
        value
            .as_array()
            .expect("assigned_sentences")
            .iter()
            .map(|label| label.as_u64().map(|label| label as usize))
            .collect()
    }

    fn dominant_log_probs(classes: &[u8]) -> Vec<f32> {
        let mut out = Vec::with_capacity(classes.len() * 7);
        for class in classes {
            let mut row = [-10.0_f32; 7];
            row[usize::from(*class)] = 0.0;
            out.extend(row);
        }
        out
    }

    fn intervals_for_run(run_frames: usize, audio_len_samples: usize) -> Vec<SpeakerInterval> {
        let mut classes = vec![0_u8; FRAMES_PER_WINDOW];
        for class in classes.iter_mut().take(run_frames) {
            *class = 1;
        }
        let data = dominant_log_probs(&classes);
        let log_probs = FrameLogProbs::from_row_major(&data, 7).expect("shape");
        find_intervals(log_probs, audio_len_samples)
    }

    fn plain_argmax_k(curve: &[(usize, f64)]) -> usize {
        curve
            .iter()
            .max_by(|left, right| left.1.partial_cmp(&right.1).expect("finite scores"))
            .map(|(k, _score)| *k)
            .expect("non-empty curve")
    }

    fn k3_margin(curve: &[(usize, f64)]) -> f64 {
        let k2 = curve.iter().find(|(k, _score)| *k == 2).expect("k=2").1;
        let k3 = curve.iter().find(|(k, _score)| *k == 3).expect("k=3").1;
        k3 - (k2 + SILHOUETTE_IMPROVEMENT)
    }

    fn score_comparison_error(actual: f64, expected: f64, tolerance: f64) -> Option<String> {
        if (actual - expected).abs() > tolerance {
            Some(format!(
                "score drifted: actual={actual} expected={expected} tolerance={tolerance}"
            ))
        } else {
            None
        }
    }

    fn square_distances<const N: usize>(rows: &[[f64; N]; N]) -> Vec<f64> {
        rows.iter().flat_map(|row| row.iter().copied()).collect()
    }

    #[cfg(feature = "full-tests")]
    fn assert_merge(merge: &Merge, left: usize, right: usize, distance: f64, size: usize) {
        assert_eq!(merge.left, left);
        assert_eq!(merge.right, right);
        assert_close(merge.distance, distance, 1e-12);
        assert_eq!(merge.size, size);
    }

    fn assert_close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "actual={actual} expected={expected} tolerance={tolerance}"
        );
    }

    #[cfg(feature = "full-tests")]
    fn assert_same_partition(actual: &[usize], expected: &[usize]) {
        assert_eq!(actual.len(), expected.len());
        for left in 0..actual.len() {
            for right in 0..actual.len() {
                assert_eq!(
                    actual[left] == actual[right],
                    expected[left] == expected[right],
                    "partition mismatch at ({left}, {right}): actual={actual:?} expected={expected:?}"
                );
            }
        }
    }

    #[cfg(feature = "full-tests")]
    fn partition_refines(finer: &[usize], coarser: &[usize]) -> bool {
        if finer.len() != coarser.len() {
            return false;
        }
        for left in 0..finer.len() {
            for right in 0..finer.len() {
                if finer[left] == finer[right] && coarser[left] != coarser[right] {
                    return false;
                }
            }
        }
        true
    }
}
