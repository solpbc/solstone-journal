// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use solstone_core_speakers::discovery::cluster_embeddings;

fn partition(labels: &[Option<usize>]) -> BTreeSet<BTreeSet<usize>> {
    let mut by_label: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (index, label) in labels.iter().enumerate() {
        if let Some(label) = label {
            by_label.entry(*label).or_default().insert(index);
        }
    }
    by_label.into_values().collect()
}

fn expected_discovery_partition(rows: usize) -> BTreeSet<BTreeSet<usize>> {
    let mut expected = BTreeSet::new();
    expected.insert((0..95).collect());
    let mut start = 95;
    while start < 9_995 {
        expected.insert((start..start + 60).collect());
        start += 60;
    }
    expected.insert((9_995..rows).collect());
    expected
}

#[test]
fn ten_thousand_by_256_discovery_clustering_recovers_the_seeded_partition() {
    let rows = 10_000;
    let cols = 256;
    let (matrix, expected_clusters) =
        seeded_unit_normalized_discovery_matrix(rows, cols, 0x5eed_5eed_cafe_babe);

    let started = Instant::now();
    let labels = cluster_embeddings(&matrix, rows, cols, 5, 3).expect("benchmark clusters");
    let elapsed = started.elapsed();
    let noise_count = labels.iter().filter(|label| label.is_none()).count();
    let cluster_count: BTreeSet<usize> = labels.iter().filter_map(|label| *label).collect();

    println!(
        "discovery_hdbscan_benchmark rows={rows} cols={cols} profile={} elapsed_ms={} clusters={} noise={noise_count}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        elapsed.as_millis(),
        cluster_count.len()
    );

    assert_eq!(labels.len(), rows);
    assert_eq!(cluster_count.len(), 167);
    assert_eq!(expected_clusters, 167);
    assert_eq!(noise_count, 0);
    assert_eq!(partition(&labels), expected_discovery_partition(rows));
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
