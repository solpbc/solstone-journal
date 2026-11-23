// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Resolution-8 H3 cells for low-resolution (~1 km) agent location sharing.
//!
//! A cell's reported coordinates come strictly from the cell centre — never
//! from the input latitude or longitude — so any two points inside the same
//! cell produce identical output.

use h3o::{LatLng, Resolution};
use serde::Serialize;

/// One H3 resolution-8 cell (~0.74 km², edge ~461 m) and its centre point.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApproxCell {
    pub h3: String,
    pub cell_lat: f64,
    pub cell_lon: f64,
}

/// Map `(latitude, longitude)` to its H3 resolution-8 cell and cell centre.
/// Returns `None` for non-finite or out-of-range coordinates.
#[must_use]
pub fn res8_cell(latitude: f64, longitude: f64) -> Option<ApproxCell> {
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !(-90.0..=90.0).contains(&latitude)
        || !(-180.0..=180.0).contains(&longitude)
    {
        return None;
    }
    let coord = LatLng::new(latitude, longitude).ok()?;
    let cell = coord.to_cell(Resolution::Eight);
    let center = LatLng::from(cell);
    Some(ApproxCell {
        h3: cell.to_string(),
        cell_lat: round5(center.lat()),
        cell_lon: round5(center.lng()),
    })
}

fn round5(value: f64) -> f64 {
    (value * 100_000.0).round() / 100_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn res8_cell_matches_known_h3_indices_and_hides_input_offsets() {
        // Known H3 res-8 indices verified against h3-js 4.5.0.
        let london = res8_cell(51.515, -0.0815).expect("london cell");
        assert_eq!(london.h3, "88194ad301fffff");
        assert!((london.cell_lat - 51.514).abs() < 0.01);
        assert!((london.cell_lon - (-0.081)).abs() < 0.01);

        let stratford = res8_cell(52.1939, -1.708).expect("stratford cell");
        assert_eq!(stratford.h3, "88195c2545fffff");

        let bell = res8_cell(51.5131, -0.1000).expect("bell cell");
        assert_eq!(bell.h3, "88194ad329fffff");

        let westminster = res8_cell(51.4999, -0.1254).expect("westminster cell");
        assert_eq!(westminster.h3, "88194ad14dfffff");

        let slough = res8_cell(51.592475, -0.61325).expect("slough cell");
        assert_eq!(slough.h3, "88195d322bfffff");

        // Two nearby points in the same cell produce byte-identical cell_lat/cell_lon.
        let a = res8_cell(51.51498, -0.08151).expect("point a");
        let b = res8_cell(51.51505, -0.08145).expect("point b");
        assert_eq!(a, b);
    }

    #[test]
    fn res8_cell_rejects_non_finite_and_out_of_range_inputs() {
        assert_eq!(res8_cell(f64::NAN, 0.0), None);
        assert_eq!(res8_cell(0.0, f64::INFINITY), None);
        assert_eq!(res8_cell(91.0, 0.0), None);
        assert_eq!(res8_cell(0.0, -181.0), None);
    }
}
