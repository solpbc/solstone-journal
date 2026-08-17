// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use hdbscan::{DistanceMetric, Hdbscan, HdbscanError, HdbscanHyperParams, NnAlgorithm};

const UNIT_NORM_TOLERANCE: f64 = 1.0e-3;

#[derive(Debug, Clone, PartialEq)]
pub enum DiscoveryClusteringError {
    InvalidMinClusterSize {
        actual: usize,
    },
    InvalidMinSamples {
        actual: usize,
    },
    ZeroColumns,
    ShapeOverflow {
        rows: usize,
        cols: usize,
    },
    ShapeMismatch {
        rows: usize,
        cols: usize,
        len: usize,
    },
    MinSamplesExceedsRows {
        min_samples: usize,
        rows: usize,
    },
    NonFiniteCoordinate {
        row: usize,
        col: usize,
    },
    NonUnitEmbeddingRow {
        row: usize,
        norm: f64,
    },
    HdbscanEmptyDataset,
    HdbscanWrongDimension {
        detail: String,
    },
    HdbscanNonFiniteCoordinate {
        detail: String,
    },
    HdbscanOutputLength {
        expected: usize,
        actual: usize,
    },
    HdbscanInvalidLabel {
        label: i32,
    },
}

impl fmt::Display for DiscoveryClusteringError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMinClusterSize { actual } => write!(
                formatter,
                "invalid min_cluster_size: expected at least 2, got {actual}"
            ),
            Self::InvalidMinSamples { actual } => write!(
                formatter,
                "invalid min_samples: expected at least 1, got {actual}"
            ),
            Self::ZeroColumns => {
                write!(formatter, "row-major matrix must have at least one column")
            }
            Self::ShapeOverflow { rows, cols } => {
                write!(
                    formatter,
                    "row-major matrix shape overflow: rows={rows} cols={cols}"
                )
            }
            Self::ShapeMismatch { rows, cols, len } => write!(
                formatter,
                "row-major matrix length mismatch: rows={rows} cols={cols} len={len}"
            ),
            Self::MinSamplesExceedsRows { min_samples, rows } => write!(
                formatter,
                "invalid min_samples: min_samples={min_samples} exceeds rows={rows}"
            ),
            Self::NonFiniteCoordinate { row, col } => {
                write!(
                    formatter,
                    "matrix value at row={row} col={col} is not finite"
                )
            }
            Self::NonUnitEmbeddingRow { row, norm } => write!(
                formatter,
                "embedding row {row} is not unit length: norm={norm}"
            ),
            Self::HdbscanEmptyDataset => write!(formatter, "hdbscan rejected an empty dataset"),
            Self::HdbscanWrongDimension { detail } => {
                write!(formatter, "hdbscan rejected matrix dimensions: {detail}")
            }
            Self::HdbscanNonFiniteCoordinate { detail } => {
                write!(
                    formatter,
                    "hdbscan rejected a non-finite coordinate: {detail}"
                )
            }
            Self::HdbscanOutputLength { expected, actual } => write!(
                formatter,
                "hdbscan returned {actual} labels for {expected} rows"
            ),
            Self::HdbscanInvalidLabel { label } => {
                write!(formatter, "hdbscan returned invalid label {label}")
            }
        }
    }
}

impl Error for DiscoveryClusteringError {}

/// Cluster unknown speaker embeddings with the verified `hdbscan` EOM kernel.
///
/// The caller supplies all sklearn-tuned parameters explicitly. Noise is returned
/// as `None`; consumers that need sklearn's JSON-facing `-1` sentinel should map
/// it at that boundary.
///
/// Degenerate inputs are named deliberately: invalid HDBSCAN parameters error,
/// zero rows returns an empty result for direct callers, too few rows returns all
/// noise to preserve the Python discovery early-out, excessive `min_samples`
/// mirrors sklearn's fit-time error, and non-finite coordinates error instead
/// of sklearn's non-finite-row remapping.
pub fn cluster_embeddings(
    embeddings: &[f32],
    rows: usize,
    cols: usize,
    min_cluster_size: usize,
    min_samples: usize,
) -> Result<Vec<Option<usize>>, DiscoveryClusteringError> {
    if min_cluster_size < 2 {
        // sklearn rejects this before fit proceeds
        // (.venv/lib/python3.13/site-packages/sklearn/cluster/_hdbscan/hdbscan.py:640-642).
        return Err(DiscoveryClusteringError::InvalidMinClusterSize {
            actual: min_cluster_size,
        });
    }
    if min_samples == 0 {
        // sklearn rejects zero min_samples alongside the min_cluster_size guard
        // (.venv/lib/python3.13/site-packages/sklearn/cluster/_hdbscan/hdbscan.py:642).
        return Err(DiscoveryClusteringError::InvalidMinSamples {
            actual: min_samples,
        });
    }
    if cols == 0 {
        // A zero-width Euclidean space would make every row indistinguishable,
        // so direct callers get a named shape error instead of accidental labels.
        return Err(DiscoveryClusteringError::ZeroColumns);
    }
    validate_shape(embeddings, rows, cols)?;
    if rows == 0 {
        // Python has no direct kernel call for rows==0. The caller normally
        // short-circuits before clustering, so this protects direct callers and
        // mirrors diarization.rs:238-248.
        return Ok(Vec::new());
    }
    if rows < min_cluster_size {
        // Keep solstone/apps/speakers/discovery.py:306-308 reachable: too few
        // points for a legal cluster is not an error, and this happens before
        // min_samples is compared with rows.
        return Ok(vec![None; rows]);
    }
    if min_samples > rows {
        // The hdbscan crate validates shape and infinities
        // (~/.cargo/registry/src/.../hdbscan-0.12.0/src/validation.rs:16-47)
        // but does not reject min_samples > rows before k-NN indexing.
        return Err(DiscoveryClusteringError::MinSamplesExceedsRows { min_samples, rows });
    }
    validate_finite(embeddings, rows, cols)?;
    validate_unit_rows(embeddings, rows, cols)?;

    let data = matrix_as_f64_rows(embeddings, rows, cols);
    let hyper_params = HdbscanHyperParams::builder()
        .min_cluster_size(min_cluster_size)
        .min_samples(min_samples)
        .max_cluster_size(usize::MAX)
        .allow_single_cluster(false)
        .epsilon(0.0)
        .dist_metric(DistanceMetric::Euclidean)
        // The crate exposes the nearest-neighbour choice. Pin KD-tree instead
        // of Auto so rows <= 250 do not take the crate's brute-force branch
        // (~/.cargo/registry/src/.../hdbscan-0.12.0/src/core_distances.rs:1-18),
        // while sklearn resolves this production metric to KD-tree
        // (.venv/lib/python3.13/site-packages/sklearn/cluster/_hdbscan/hdbscan.py:848-857).
        .nn_algorithm(NnAlgorithm::KdTree)
        .build();
    let clusterer = Hdbscan::new(&data, hyper_params);
    let raw_labels = clusterer.cluster().map_err(map_hdbscan_error)?;
    option_labels_from_hdbscan(raw_labels, rows)
}

fn validate_shape(
    embeddings: &[f32],
    rows: usize,
    cols: usize,
) -> Result<(), DiscoveryClusteringError> {
    let expected = rows
        .checked_mul(cols)
        .ok_or(DiscoveryClusteringError::ShapeOverflow { rows, cols })?;
    if embeddings.len() != expected {
        return Err(DiscoveryClusteringError::ShapeMismatch {
            rows,
            cols,
            len: embeddings.len(),
        });
    }
    Ok(())
}

fn validate_finite(
    embeddings: &[f32],
    rows: usize,
    cols: usize,
) -> Result<(), DiscoveryClusteringError> {
    for row in 0..rows {
        for col in 0..cols {
            if !embeddings[row * cols + col].is_finite() {
                // Deliberate divergence: sklearn remaps non-finite rows before
                // fitting (.venv/lib/python3.13/site-packages/sklearn/cluster/_hdbscan/hdbscan.py:747-772).
                // The Rust kernel names the bad coordinate instead. This also
                // prevents the hdbscan crate's NaN path from reaching
                // partial_cmp(...).expect("Invalid float")
                // (~/.cargo/registry/src/.../hdbscan-0.12.0/src/core_distances/serial.rs:76-78).
                return Err(DiscoveryClusteringError::NonFiniteCoordinate { row, col });
            }
        }
    }
    Ok(())
}

fn validate_unit_rows(
    embeddings: &[f32],
    rows: usize,
    cols: usize,
) -> Result<(), DiscoveryClusteringError> {
    for row in 0..rows {
        let mut norm_squared = 0.0;
        for col in 0..cols {
            let value = f64::from(embeddings[row * cols + col]);
            norm_squared += value * value;
        }
        let norm = norm_squared.sqrt();
        if (norm - 1.0).abs() > UNIT_NORM_TOLERANCE {
            // solstone/think/entities/voiceprints.py:31 normalize_embedding,
            // reached through solstone/apps/speakers/discovery.py:271, is the
            // production normalization point. This assertion catches callers
            // that silently stop normalizing; in-kernel normalization would be
            // idempotent and invisible to tests on already-normalized inputs.
            return Err(DiscoveryClusteringError::NonUnitEmbeddingRow { row, norm });
        }
    }
    Ok(())
}

fn matrix_as_f64_rows(embeddings: &[f32], rows: usize, cols: usize) -> Vec<Vec<f64>> {
    let mut out = Vec::with_capacity(rows);
    for row in 0..rows {
        let start = row * cols;
        // sklearn casts HDBSCAN fit input to float64 before clustering
        // (.venv/lib/python3.13/site-packages/sklearn/cluster/_hdbscan/hdbscan.py:739-745).
        // The hdbscan crate API takes &[Vec<T>], so this necessarily allocates
        // one Vec per row before invoking the dependency.
        out.push(
            embeddings[start..start + cols]
                .iter()
                .map(|value| f64::from(*value))
                .collect(),
        );
    }
    out
}

fn map_hdbscan_error(error: HdbscanError) -> DiscoveryClusteringError {
    match error {
        HdbscanError::EmptyDataset => DiscoveryClusteringError::HdbscanEmptyDataset,
        HdbscanError::WrongDimension(detail) => {
            DiscoveryClusteringError::HdbscanWrongDimension { detail }
        }
        HdbscanError::NonFiniteCoordinate(detail) => {
            DiscoveryClusteringError::HdbscanNonFiniteCoordinate { detail }
        }
    }
}

fn option_labels_from_hdbscan(
    labels: Vec<i32>,
    rows: usize,
) -> Result<Vec<Option<usize>>, DiscoveryClusteringError> {
    if labels.len() != rows {
        return Err(DiscoveryClusteringError::HdbscanOutputLength {
            expected: rows,
            actual: labels.len(),
        });
    }

    let mut members_by_raw_label: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    let mut out = vec![None; rows];
    for (row, label) in labels.into_iter().enumerate() {
        match label {
            -1 => {}
            0..=i32::MAX => {
                members_by_raw_label.entry(label).or_default().push(row);
            }
            _ => return Err(DiscoveryClusteringError::HdbscanInvalidLabel { label }),
        }
    }

    let mut clusters: Vec<Vec<usize>> = members_by_raw_label.into_values().collect();
    clusters.sort_by_key(|members| members[0]);
    for (label, members) in clusters.into_iter().enumerate() {
        for row in members {
            out[row] = Some(label);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn empty_matrix_returns_empty_labels() {
        let labels = cluster_embeddings(&[], 0, 2, 5, 3).expect("empty matrix clusters");

        assert!(labels.is_empty());
    }

    #[test]
    fn fewer_rows_than_min_cluster_size_returns_noise_before_min_samples_row_check() {
        let matrix = vec![0.0_f32, 0.0, 1.0, 1.0, 2.0, 2.0];

        let labels =
            cluster_embeddings(&matrix, 3, 2, 5, 99).expect("too few rows is not an error");

        assert_eq!(labels, vec![None, None, None]);
    }

    #[test]
    fn all_noise_result_is_representable() {
        let matrix = unit_rows_2d(&[(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0), (1.0, 1.0)]);

        let labels = cluster_embeddings(&matrix, 5, 2, 5, 2).expect("matrix clusters");

        assert_eq!(labels, vec![None, None, None, None, None]);
    }

    #[test]
    fn well_separated_clusters_are_recovered_as_partition() {
        let matrix = unit_rows_2d(&[
            (1.0, 0.0),
            (1.0, 0.03),
            (1.0, -0.03),
            (-1.0, 0.0),
            (-1.0, 0.03),
            (-1.0, -0.03),
        ]);

        let labels = cluster_embeddings(&matrix, 6, 2, 3, 2).expect("matrix clusters");

        assert_eq!(
            partition(&labels),
            partition_from_sets(&[&[0, 1, 2], &[3, 4, 5]])
        );
    }

    #[test]
    fn min_cluster_size_boundary_excludes_root_inherited_exact_size_group() {
        let matrix = unit_rows_2d(&[
            (1.0, 0.0),
            (1.0, 0.03),
            (1.0, -0.03),
            (-1.0, 0.0),
            (-1.0, 0.03),
        ]);

        let labels = cluster_embeddings(&matrix, 5, 2, 3, 2).expect("matrix clusters");

        assert_eq!(labels, vec![None, None, None, None, None]);
    }

    #[test]
    fn non_unit_embedding_rows_are_rejected_instead_of_normalized() {
        let matrix = vec![
            0.9_f32, 0.0, 1.0, 0.0, 1.1, 0.0, 9.0, 0.0, 10.0, 0.0, 11.0, 0.0,
        ];

        let error = cluster_embeddings(&matrix, 6, 2, 3, 2).unwrap_err();

        let DiscoveryClusteringError::NonUnitEmbeddingRow { row, norm } = error else {
            panic!("expected NonUnitEmbeddingRow");
        };
        assert_eq!(row, 0);
        assert!((norm - 0.9).abs() < 1.0e-6);
    }

    #[test]
    fn many_well_separated_256_dim_groups_recover_expected_cluster_count() {
        let rows = 2_000;
        let cols = 256;
        let (matrix, expected_clusters) = seeded_unit_normalized_discovery_matrix(rows, cols, 11);

        let labels = cluster_embeddings(&matrix, rows, cols, 5, 3).expect("matrix clusters");

        assert_eq!(partition(&labels).len(), expected_clusters);
    }

    #[test]
    fn invalid_min_cluster_size_is_error() {
        assert_eq!(
            cluster_embeddings(&[], 0, 2, 1, 1),
            Err(DiscoveryClusteringError::InvalidMinClusterSize { actual: 1 })
        );
    }

    #[test]
    fn invalid_min_samples_is_error() {
        assert_eq!(
            cluster_embeddings(&[], 0, 2, 2, 0),
            Err(DiscoveryClusteringError::InvalidMinSamples { actual: 0 })
        );
    }

    #[test]
    fn zero_columns_is_error() {
        assert_eq!(
            cluster_embeddings(&[], 0, 0, 2, 1),
            Err(DiscoveryClusteringError::ZeroColumns)
        );
    }

    #[test]
    fn min_samples_greater_than_rows_is_error() {
        let matrix = unit_rows_2d(&[(1.0, 0.0), (-1.0, 0.0)]);

        assert_eq!(
            cluster_embeddings(&matrix, 2, 2, 2, 3),
            Err(DiscoveryClusteringError::MinSamplesExceedsRows {
                min_samples: 3,
                rows: 2
            })
        );
    }

    #[test]
    fn non_finite_coordinate_is_error() {
        let matrix = vec![1.0_f32, 0.0, f32::NAN, 1.0];

        assert_eq!(
            cluster_embeddings(&matrix, 2, 2, 2, 1),
            Err(DiscoveryClusteringError::NonFiniteCoordinate { row: 1, col: 0 })
        );
    }

    #[test]
    fn shape_overflow_is_error() {
        assert_eq!(
            cluster_embeddings(&[], usize::MAX, 2, 2, 1),
            Err(DiscoveryClusteringError::ShapeOverflow {
                rows: usize::MAX,
                cols: 2
            })
        );
    }

    #[test]
    fn slice_length_mismatch_is_error() {
        let matrix = vec![0.0_f32, 0.0, 1.0];

        assert_eq!(
            cluster_embeddings(&matrix, 2, 2, 2, 1),
            Err(DiscoveryClusteringError::ShapeMismatch {
                rows: 2,
                cols: 2,
                len: 3
            })
        );
    }

    fn unit_rows_2d(points: &[(f32, f32)]) -> Vec<f32> {
        let mut out = Vec::with_capacity(points.len() * 2);
        for (x, y) in points {
            let norm = (x * x + y * y).sqrt();
            out.push(*x / norm);
            out.push(*y / norm);
        }
        out
    }

    fn partition(labels: &[Option<usize>]) -> BTreeSet<BTreeSet<usize>> {
        let mut by_label: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for (index, label) in labels.iter().enumerate() {
            if let Some(label) = label {
                by_label.entry(*label).or_default().insert(index);
            }
        }
        by_label.into_values().collect()
    }

    fn partition_from_sets(clusters: &[&[usize]]) -> BTreeSet<BTreeSet<usize>> {
        clusters
            .iter()
            .map(|cluster| cluster.iter().copied().collect())
            .collect()
    }

    fn seeded_unit_normalized_discovery_matrix(
        rows: usize,
        cols: usize,
        seed: u64,
    ) -> (Vec<f32>, usize) {
        let mut rng = Lcg::new(seed);
        let marginal = 5;
        let group_count = 3.max((rows - marginal) / 60);
        let mut sizes = vec![(rows - marginal) / group_count; group_count];
        sizes[0] += (rows - marginal) - sizes.iter().sum::<usize>();
        sizes.push(marginal);

        let mut centers = Vec::with_capacity(sizes.len());
        for _ in 0..sizes.len() {
            let mut center = Vec::with_capacity(cols);
            for _ in 0..cols {
                center.push(rng.next_standard_normal());
            }
            normalize_f64_row(&mut center);
            centers.push(center);
        }

        let mut matrix = Vec::with_capacity(rows * cols);
        for (group_index, size) in sizes.iter().enumerate() {
            let total_spread = if group_index == sizes.len() - 1 {
                0.45
            } else {
                0.12
            };
            let per_dimension_spread = total_spread / (cols as f64).sqrt();
            for _ in 0..*size {
                let mut row = Vec::with_capacity(cols);
                for center_value in &centers[group_index] {
                    row.push(*center_value + rng.next_standard_normal() * per_dimension_spread);
                }
                normalize_f64_row(&mut row);
                matrix.extend(row.into_iter().map(|value| value as f32));
            }
        }

        (matrix, sizes.len())
    }

    fn normalize_f64_row(row: &mut [f64]) {
        let norm = row.iter().map(|value| value * value).sum::<f64>().sqrt();
        for value in row {
            *value /= norm;
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct Lcg {
        state: u64,
        spare_normal: Option<f64>,
    }

    impl Lcg {
        fn new(seed: u64) -> Self {
            Self {
                state: seed,
                spare_normal: None,
            }
        }

        fn next_unit_f64(&mut self) -> f64 {
            self.state = self
                .state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let bits = self.state >> 11;
            bits as f64 / (1_u64 << 53) as f64
        }

        fn next_standard_normal(&mut self) -> f64 {
            if let Some(value) = self.spare_normal.take() {
                return value;
            }
            let u1 = self.next_unit_f64().max(f64::MIN_POSITIVE);
            let u2 = self.next_unit_f64();
            let radius = (-2.0 * u1.ln()).sqrt();
            let angle = std::f64::consts::TAU * u2;
            self.spare_normal = Some(radius * angle.sin());
            radius * angle.cos()
        }
    }
}
