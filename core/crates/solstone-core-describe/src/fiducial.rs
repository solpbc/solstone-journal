use image::{GrayImage, ImageBuffer, Luma, Rgb, RgbImage};
use imageproc::contours::{BorderType, find_contours};
use imageproc::contrast::adaptive_threshold;
use imageproc::drawing::draw_polygon_mut;
use imageproc::geometric_transformations::{Border, Interpolation, Projection, warp_into};
use imageproc::geometry::{approximate_polygon_dp, arc_length, min_area_rect};
use imageproc::point::Point;

use crate::{PreHashOutcome, PreHashRejectReason, PreHashTransform, RgbFrame};

pub const AREA_RELATIVE_TOLERANCE: f64 = 0.0009;
pub const MASK_SKIP_THRESHOLD: f64 = 0.8;
pub const MIN_MARKER_PERIMETER_RATE: f64 = 0.003;
pub const MAX_MARKER_PERIMETER_RATE: f64 = 8.0;

const CORNER_TAG_IDS: [u8; 4] = [6, 7, 2, 4];
const RECTIFIED_SIDE: u32 = 60;
const CELL_COUNT: u32 = 6;
// Contours follow the inside edge of the thresholded black border; shift each fitted corner
// 4/13 px outward per axis to recover the physical marker edge before sampling and masking.
const CORNER_EDGE_OFFSET: f32 = 4.0 / 13.0;

#[derive(Clone, Debug, PartialEq)]
pub struct ArucoMarker {
    pub id: u8,
    pub corners: [[f32; 2]; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArucoFrame {
    pub markers: Vec<ArucoMarker>,
    pub polygon: Option<[[f32; 2]; 4]>,
    pub masked: bool,
    /// `None` represents the Python result omitting the `extrapolated` key.
    pub extrapolated: Option<u8>,
}

#[derive(Default)]
pub struct ConveyFiducialMask;

impl PreHashTransform for ConveyFiducialMask {
    fn apply(&mut self, _frame_id: u64, _timestamp: f64, frame: &mut RgbFrame) -> PreHashOutcome {
        let Some(mut aruco) = detect_markers(frame) else {
            return PreHashOutcome::Apply { aruco: None };
        };
        if let Some(polygon) = aruco.polygon {
            let frame_area = f64::from(frame.width) * f64::from(frame.height);
            if exceeds_mask_threshold(polygon_area(&polygon), frame_area) {
                return PreHashOutcome::Reject(PreHashRejectReason::FiducialMask);
            }
            fill_polygon_black(frame, &polygon);
            aruco.masked = true;
        }
        PreHashOutcome::Apply { aruco: Some(aruco) }
    }
}

fn exceeds_mask_threshold(polygon_area: f64, frame_area: f64) -> bool {
    polygon_area / frame_area > MASK_SKIP_THRESHOLD
}

pub fn detect_markers(frame: &RgbFrame) -> Option<ArucoFrame> {
    let gray = grayscale(frame)?;
    // Adaptive thresholding adds locally dark regions; the fixed threshold retains uniformly black marker borders.
    let adaptive = adaptive_threshold(&gray, 11, 7);
    let mut binary = GrayImage::new(frame.width, frame.height);
    for (x, y, pixel) in binary.enumerate_pixels_mut() {
        let dark = gray.get_pixel(x, y)[0] < 128;
        let locally_dark = adaptive.get_pixel(x, y)[0] == 0;
        *pixel = Luma([u8::from(dark || locally_dark) * 255]);
    }

    let image_perimeter = 2.0 * (f64::from(frame.width) + f64::from(frame.height));
    let mut markers = Vec::new();
    for contour in find_contours::<i32>(&binary) {
        if contour.border_type != BorderType::Outer || contour.points.len() < 4 {
            continue;
        }
        let perimeter = arc_length(&contour.points, true);
        let rate = perimeter / image_perimeter;
        if !(MIN_MARKER_PERIMETER_RATE..=MAX_MARKER_PERIMETER_RATE).contains(&rate) {
            continue;
        }
        let curve = contour.points;
        let points = approximate_polygon_dp(&curve, perimeter * 0.03, true);
        if points.len() != 4 {
            continue;
        }
        let Some(points) = fit_quad(&curve, min_area_rect(&curve)) else {
            continue;
        };
        let Some(corners) = order_corners(points) else {
            continue;
        };
        let corners = refine_corners(corners);
        let Some((id, rotation)) = decode_marker(&gray, corners) else {
            continue;
        };
        let own_top_left = (4 - rotation) % 4;
        let corners = std::array::from_fn(|index| corners[(own_top_left + index) % 4]);
        markers.push(ArucoMarker { id, corners });
    }
    markers.sort_by_key(|marker| marker.id);
    markers.dedup_by_key(|marker| marker.id);
    (!markers.is_empty()).then(|| assemble_frame(markers))
}

fn grayscale(frame: &RgbFrame) -> Option<GrayImage> {
    let pixels = frame
        .pixels
        .chunks_exact(3)
        .map(|rgb| {
            let luma = u32::from(rgb[0]) * 19_595
                + u32::from(rgb[1]) * 38_470
                + u32::from(rgb[2]) * 7_471
                + 0x8000;
            (luma >> 16) as u8
        })
        .collect();
    ImageBuffer::from_vec(frame.width, frame.height, pixels)
}

fn order_corners(points: [[f32; 2]; 4]) -> Option<[[f32; 2]; 4]> {
    let mut ordered = points;
    ordered.sort_by(|left, right| (left[0] + left[1]).total_cmp(&(right[0] + right[1])));
    let top_left = ordered[0];
    let bottom_right = ordered[3];
    let (top_right, bottom_left) = if ordered[1][0] > ordered[2][0] {
        (ordered[1], ordered[2])
    } else {
        (ordered[2], ordered[1])
    };
    let corners = [top_left, top_right, bottom_right, bottom_left];
    (polygon_area(&corners) > 1.0).then_some(corners)
}

fn refine_corners(corners: [[f32; 2]; 4]) -> [[f32; 2]; 4] {
    let center = [
        corners.iter().map(|corner| corner[0]).sum::<f32>() / 4.0,
        corners.iter().map(|corner| corner[1]).sum::<f32>() / 4.0,
    ];
    corners.map(|corner| {
        [
            corner[0]
                + if corner[0] < center[0] {
                    -CORNER_EDGE_OFFSET
                } else {
                    CORNER_EDGE_OFFSET
                },
            corner[1]
                + if corner[1] < center[1] {
                    -CORNER_EDGE_OFFSET
                } else {
                    CORNER_EDGE_OFFSET
                },
        ]
    })
}

fn fit_quad(contour: &[Point<i32>], initial: [Point<i32>; 4]) -> Option<[[f32; 2]; 4]> {
    let initial = initial.map(|point| [point.x as f64, point.y as f64]);
    let lines: Vec<_> = initial
        .iter()
        .zip(initial.iter().cycle().skip(1))
        .take(4)
        .map(|(start, end)| fit_line(contour, *start, *end))
        .collect::<Option<_>>()?;
    Some([
        intersection(lines[3], lines[0])?,
        intersection(lines[0], lines[1])?,
        intersection(lines[1], lines[2])?,
        intersection(lines[2], lines[3])?,
    ])
}

fn fit_line(contour: &[Point<i32>], start: [f64; 2], end: [f64; 2]) -> Option<[f64; 3]> {
    let direction = [end[0] - start[0], end[1] - start[1]];
    let length = direction[0].hypot(direction[1]);
    if length == 0.0 {
        return None;
    }
    let selected: Vec<_> = contour
        .iter()
        .filter_map(|point| {
            let point = [f64::from(point.x), f64::from(point.y)];
            let relative = [point[0] - start[0], point[1] - start[1]];
            let along = (relative[0] * direction[0] + relative[1] * direction[1]) / length;
            let distance = (relative[0] * direction[1] - relative[1] * direction[0]).abs() / length;
            ((-2.0..=length + 2.0).contains(&along) && distance <= 1.5).then_some(point)
        })
        .collect();
    if selected.len() < 2 {
        return None;
    }
    let center = [
        selected.iter().map(|point| point[0]).sum::<f64>() / selected.len() as f64,
        selected.iter().map(|point| point[1]).sum::<f64>() / selected.len() as f64,
    ];
    let (xx, yy, xy) = selected
        .iter()
        .fold((0.0, 0.0, 0.0), |(xx, yy, xy), point| {
            let x = point[0] - center[0];
            let y = point[1] - center[1];
            (xx + x * x, yy + y * y, xy + x * y)
        });
    let angle = 0.5 * (2.0 * xy).atan2(xx - yy);
    let normal = [-angle.sin(), angle.cos()];
    Some([
        normal[0],
        normal[1],
        -normal[0] * center[0] - normal[1] * center[1],
    ])
}

fn intersection(left: [f64; 3], right: [f64; 3]) -> Option<[f32; 2]> {
    let determinant = left[0] * right[1] - right[0] * left[1];
    (determinant.abs() > 1e-6).then(|| {
        [
            ((left[1] * right[2] - right[1] * left[2]) / determinant) as f32,
            ((right[0] * left[2] - left[0] * right[2]) / determinant) as f32,
        ]
    })
}

fn decode_marker(gray: &GrayImage, corners: [[f32; 2]; 4]) -> Option<(u8, usize)> {
    let projection = Projection::from_control_points(
        corners.map(|corner| (corner[0], corner[1])),
        [
            (0.0, 0.0),
            ((RECTIFIED_SIDE - 1) as f32, 0.0),
            ((RECTIFIED_SIDE - 1) as f32, (RECTIFIED_SIDE - 1) as f32),
            (0.0, (RECTIFIED_SIDE - 1) as f32),
        ],
    )?;
    let mut rectified = GrayImage::new(RECTIFIED_SIDE, RECTIFIED_SIDE);
    warp_into(
        gray,
        projection,
        Interpolation::Nearest,
        Border::Constant(Luma([255])),
        &mut rectified,
    );
    let bits = sample_bits(&rectified)?;
    let mut best: Option<(u32, u8, usize)> = None;
    for rotation in 0..4 {
        let rotated = rotate_clockwise(bits, rotation);
        for &(id, code) in &DICTIONARY {
            let distance = hamming(rotated, code);
            if distance <= 1 && best.is_none_or(|current| distance < current.0) {
                best = Some((distance, id, rotation));
            }
        }
    }
    best.map(|(_, id, rotation)| (id, rotation))
}

fn sample_bits(image: &GrayImage) -> Option<[[bool; 4]; 4]> {
    let cell = RECTIFIED_SIDE / CELL_COUNT;
    let black = |row: u32, column: u32| {
        let x = column * cell + cell / 2;
        let y = row * cell + cell / 2;
        image.get_pixel(x, y)[0] < 128
    };
    for edge in 0..CELL_COUNT {
        if !black(0, edge)
            || !black(CELL_COUNT - 1, edge)
            || !black(edge, 0)
            || !black(edge, CELL_COUNT - 1)
        {
            return None;
        }
    }
    Some(std::array::from_fn(|row| {
        std::array::from_fn(|column| black(row as u32 + 1, column as u32 + 1))
    }))
}

fn rotate_clockwise(bits: [[bool; 4]; 4], turns: usize) -> [[bool; 4]; 4] {
    (0..turns).fold(bits, |current, _| {
        std::array::from_fn(|row| std::array::from_fn(|column| current[3 - column][row]))
    })
}

fn hamming(left: [[bool; 4]; 4], right: [[bool; 4]; 4]) -> u32 {
    left.iter()
        .flatten()
        .zip(right.iter().flatten())
        .filter(|(left, right)| **left != !**right)
        .count() as u32
}

const DICTIONARY: [(u8, [[bool; 4]; 4]); 4] = [
    (
        2,
        [
            [false, false, true, true],
            [false, false, true, true],
            [false, false, true, false],
            [true, true, false, true],
        ],
    ),
    (
        4,
        [
            [false, true, false, true],
            [false, true, false, false],
            [true, false, false, true],
            [true, true, true, false],
        ],
    ),
    (
        6,
        [
            [true, false, false, true],
            [true, true, true, false],
            [false, false, true, false],
            [true, true, true, false],
        ],
    ),
    (
        7,
        [
            [true, true, false, false],
            [false, true, false, false],
            [true, true, true, true],
            [false, false, true, false],
        ],
    ),
];

fn assemble_frame(markers: Vec<ArucoMarker>) -> ArucoFrame {
    let by_id = |id| {
        markers
            .iter()
            .find(|marker| marker.id == id)
            .map(|marker| marker.corners)
    };
    let found: Vec<_> = CORNER_TAG_IDS
        .into_iter()
        .filter(|&id| by_id(id).is_some())
        .collect();
    let (polygon, extrapolated) = if found.len() == 4 {
        (
            Some([
                by_id(6).unwrap()[0],
                by_id(7).unwrap()[1],
                by_id(2).unwrap()[2],
                by_id(4).unwrap()[3],
            ]),
            None,
        )
    } else if found.len() == 3 {
        let missing = CORNER_TAG_IDS
            .into_iter()
            .find(|id| !found.contains(id))
            .unwrap();
        let point = match missing {
            6 => subtract_add(
                by_id(7).unwrap()[1],
                by_id(4).unwrap()[3],
                by_id(2).unwrap()[2],
            ),
            7 => subtract_add(
                by_id(6).unwrap()[0],
                by_id(2).unwrap()[2],
                by_id(4).unwrap()[3],
            ),
            2 => subtract_add(
                by_id(7).unwrap()[1],
                by_id(4).unwrap()[3],
                by_id(6).unwrap()[0],
            ),
            4 => subtract_add(
                by_id(6).unwrap()[0],
                by_id(2).unwrap()[2],
                by_id(7).unwrap()[1],
            ),
            _ => unreachable!(),
        };
        let point_for = |id| {
            if id == missing {
                point
            } else {
                by_id(id).unwrap()[match id {
                    6 => 0,
                    7 => 1,
                    2 => 2,
                    4 => 3,
                    _ => unreachable!(),
                }]
            }
        };
        (
            Some([point_for(6), point_for(7), point_for(2), point_for(4)]),
            Some(missing),
        )
    } else {
        (None, None)
    };
    ArucoFrame {
        markers,
        polygon,
        masked: false,
        extrapolated,
    }
}

fn subtract_add(left: [f32; 2], right: [f32; 2], subtract: [f32; 2]) -> [f32; 2] {
    [
        left[0] + right[0] - subtract[0],
        left[1] + right[1] - subtract[1],
    ]
}

pub fn polygon_area(points: &[[f32; 2]]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    0.5 * points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(left, right)| {
            f64::from(left[0]) * f64::from(right[1]) - f64::from(right[0]) * f64::from(left[1])
        })
        .sum::<f64>()
        .abs()
}

fn fill_polygon_black(frame: &mut RgbFrame, polygon: &[[f32; 2]; 4]) {
    let mut image =
        RgbImage::from_raw(frame.width, frame.height, std::mem::take(&mut frame.pixels))
            .expect("RgbFrame pixel buffer is validated on construction");
    let polygon = polygon.map(|point| Point::new(point[0] as i32, point[1] as i32));
    draw_polygon_mut(&mut image, &polygon, Rgb([0, 0, 0]));
    frame.pixels = image.into_raw();
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::path::Path;

    use serde::Deserialize;

    use super::{
        AREA_RELATIVE_TOLERANCE, ArucoMarker, ConveyFiducialMask, MAX_MARKER_PERIMETER_RATE,
        MIN_MARKER_PERIMETER_RATE, assemble_frame, detect_markers, exceeds_mask_threshold,
        polygon_area,
    };
    use crate::{PreHashOutcome, PreHashTransform, RgbFrame};

    const ORACLE_CORNER_TOLERANCE_PX: f32 = 2.0;

    #[derive(Deserialize)]
    struct Fixture {
        cases: Vec<FixtureCase>,
    }

    #[derive(Deserialize)]
    struct FixtureCase {
        detected: bool,
        file: String,
        marker_ids: Option<Vec<u8>>,
        markers: Option<Vec<FixtureMarker>>,
        polygon: Option<[[f32; 2]; 4]>,
        polygon_area: Option<f64>,
        extrapolated: Option<u8>,
        #[serde(default)]
        skips_frame: Option<bool>,
    }

    #[derive(Deserialize)]
    struct FixtureMarker {
        id: u8,
        corners: [[f32; 2]; 4],
    }

    fn fixture() -> Fixture {
        serde_json::from_str(include_str!("../../../fixtures/describe_fiducials.json"))
            .expect("valid fiducial oracle")
    }

    fn raw_fixture_case(file: &str) -> serde_json::Value {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/describe_fiducials.json"))
                .expect("valid fiducial oracle");
        fixture["cases"]
            .as_array()
            .expect("fixture cases")
            .iter()
            .find(|case| case["file"] == file)
            .cloned()
            .expect("fixture case")
    }

    fn frame(file: &str) -> RgbFrame {
        let image = image::open(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/describe_fiducials")
                .join(file),
        )
        .expect("fixture image")
        .into_rgb8();
        RgbFrame::new(image.width(), image.height(), image.into_raw()).expect("rgb frame")
    }

    fn close(actual: [f32; 2], expected: [f32; 2], label: &str) {
        assert!(
            (actual[0] - expected[0]).abs() <= ORACLE_CORNER_TOLERANCE_PX,
            "{label} x: expected {}, got {}",
            expected[0],
            actual[0]
        );
        assert!(
            (actual[1] - expected[1]).abs() <= ORACLE_CORNER_TOLERANCE_PX,
            "{label} y: expected {}, got {}",
            expected[1],
            actual[1]
        );
    }

    #[test]
    fn fixture_detection_matches_ids_corners_polygons_and_skip_verdicts() {
        for case in fixture().cases {
            let mut image = frame(&case.file);
            let actual = detect_markers(&image);
            assert_eq!(actual.is_some(), case.detected, "{} detected", case.file);
            let Some(actual) = actual else {
                assert!(
                    case.skips_frame.is_none(),
                    "{} has no skip verdict",
                    case.file
                );
                continue;
            };
            assert_eq!(
                actual
                    .markers
                    .iter()
                    .map(|marker| marker.id)
                    .collect::<Vec<_>>(),
                case.marker_ids.unwrap(),
                "{} ids",
                case.file
            );
            for (actual, expected) in actual.markers.iter().zip(case.markers.unwrap()) {
                assert_eq!(actual.id, expected.id, "{} marker", case.file);
                let marker_id = expected.id;
                for (index, (actual, expected)) in
                    actual.corners.iter().zip(expected.corners).enumerate()
                {
                    close(
                        *actual,
                        expected,
                        &format!("{} marker {marker_id} corner {index}", case.file),
                    );
                }
            }
            assert_eq!(
                actual.extrapolated, case.extrapolated,
                "{} extrapolated",
                case.file
            );
            match (actual.polygon, case.polygon) {
                (Some(actual), Some(expected)) => {
                    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                        close(*actual, expected, &format!("{} polygon {index}", case.file));
                    }
                    let bound =
                        AREA_RELATIVE_TOLERANCE * f64::from(image.width) * f64::from(image.height);
                    assert!(
                        (polygon_area(&actual) - case.polygon_area.unwrap()).abs() <= bound,
                        "{} area: expected {}, got {}, bound {bound}",
                        case.file,
                        case.polygon_area.unwrap(),
                        polygon_area(&actual)
                    );
                }
                (None, None) => {}
                _ => panic!("{} polygon presence", case.file),
            }
            let mut mask = ConveyFiducialMask;
            let skipped = matches!(mask.apply(1, 0.0, &mut image), PreHashOutcome::Reject(_));
            assert_eq!(Some(skipped), case.skips_frame, "{} skip", case.file);
        }
    }

    #[test]
    fn tolerances_and_detector_constants_are_scope_pinned() {
        const { assert!(AREA_RELATIVE_TOLERANCE < 0.001) };
        const { assert!(AREA_RELATIVE_TOLERANCE * (1280.0 * 720.0) < 4459.0) };
        assert_eq!(MIN_MARKER_PERIMETER_RATE, 0.003);
        assert_eq!(MAX_MARKER_PERIMETER_RATE, 8.0);
    }

    #[test]
    fn mask_gate_is_strictly_greater_than_threshold() {
        let exactly_at_threshold = std::hint::black_box(0.8_f64);
        let just_over = std::hint::black_box(0.800_000_1_f64);
        assert!(!exceeds_mask_threshold(exactly_at_threshold, 1.0));
        assert!(exceeds_mask_threshold(just_over, 1.0));
    }

    #[test]
    fn mask_fill_blacks_inside_without_touching_outside() {
        let mut image = frame("coverage_far_under.png");
        let original = image.pixels.clone();
        let mut transform = ConveyFiducialMask;
        let PreHashOutcome::Apply { aruco: Some(aruco) } = transform.apply(1, 0.0, &mut image)
        else {
            panic!("sub-threshold fixture is applied");
        };
        assert!(aruco.masked);
        let polygon = aruco.polygon.expect("four tags form polygon");
        for y in 0..image.height {
            for x in 0..image.width {
                let offset = ((y * image.width + x) * 3) as usize;
                match pixel_region(x, y, &polygon) {
                    PixelRegion::Inside => {
                        assert_eq!(&image.pixels[offset..offset + 3], [0, 0, 0], "{x},{y}");
                    }
                    PixelRegion::Outside => {
                        assert_eq!(
                            &image.pixels[offset..offset + 3],
                            &original[offset..offset + 3],
                            "{x},{y}"
                        );
                    }
                    PixelRegion::Boundary => {}
                }
            }
        }
    }

    #[test]
    fn no_tags_oracle_omits_skip_verdict() {
        let raw = raw_fixture_case("no_tags.png");
        assert!(
            !raw.as_object()
                .expect("fixture case object")
                .contains_key("skips_frame")
        );
    }

    #[test]
    fn polygon_area_handles_non_convex_and_short_polygons() {
        assert_eq!(
            polygon_area(&[[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [2.0, 2.0], [0.0, 4.0]]),
            12.0
        );
        assert_eq!(polygon_area(&[[0.0, 0.0], [1.0, 1.0]]), 0.0);
    }

    #[test]
    fn non_corner_marker_does_not_contribute_to_three_tag_extrapolation() {
        let marker = |id, corners| ArucoMarker { id, corners };
        let aruco = assemble_frame(vec![
            marker(6, [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]),
            marker(7, [[9.0, 0.0], [10.0, 0.0], [10.0, 1.0], [9.0, 1.0]]),
            marker(2, [[9.0, 9.0], [10.0, 9.0], [10.0, 10.0], [9.0, 10.0]]),
            marker(42, [[4.0, 4.0], [5.0, 4.0], [5.0, 5.0], [4.0, 5.0]]),
        ]);
        assert_eq!(
            aruco
                .markers
                .iter()
                .map(|marker| marker.id)
                .collect::<Vec<_>>(),
            [6, 7, 2, 42]
        );
        assert_eq!(aruco.extrapolated, Some(4));
        assert_eq!(aruco.polygon.unwrap()[3], [0.0, 10.0]);
    }

    #[derive(Clone, Copy)]
    enum PixelRegion {
        Inside,
        Outside,
        Boundary,
    }

    fn pixel_region(x: u32, y: u32, polygon: &[[f32; 2]; 4]) -> PixelRegion {
        let point = [x as f32 + 0.5, y as f32 + 0.5];
        let boundary_distance = polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .take(4)
            .map(|(start, end)| distance_to_segment(point, *start, *end))
            .fold(f32::INFINITY, f32::min);
        if boundary_distance <= std::f32::consts::FRAC_1_SQRT_2 {
            PixelRegion::Boundary
        } else if point_in_polygon(point, polygon) {
            PixelRegion::Inside
        } else {
            PixelRegion::Outside
        }
    }

    fn point_in_polygon(point: [f32; 2], polygon: &[[f32; 2]; 4]) -> bool {
        polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .take(4)
            .fold(false, |inside, (start, end)| {
                let crosses = (start[1] > point[1]) != (end[1] > point[1]);
                let intersection =
                    (end[0] - start[0]) * (point[1] - start[1]) / (end[1] - start[1]) + start[0];
                if crosses && point[0] < intersection {
                    !inside
                } else {
                    inside
                }
            })
    }

    fn distance_to_segment(point: [f32; 2], start: [f32; 2], end: [f32; 2]) -> f32 {
        let direction = [end[0] - start[0], end[1] - start[1]];
        let length_squared = direction[0].powi(2) + direction[1].powi(2);
        let progress = ((point[0] - start[0]) * direction[0]
            + (point[1] - start[1]) * direction[1])
            / length_squared;
        let progress = progress.clamp(0.0, 1.0);
        let closest = [
            start[0] + progress * direction[0],
            start[1] + progress * direction[1],
        ];
        (point[0] - closest[0]).hypot(point[1] - closest[1])
    }
}
