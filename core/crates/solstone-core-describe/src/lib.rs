// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Decode and winnow screencast video frames for `journal describe`.

mod bounding;
pub mod categories;
mod decode;
pub mod detect;
pub mod extraction;
mod fiducial;
mod hash;
mod merge;
mod notify;
pub mod pipeline;
pub mod request;
pub mod selection;
pub mod session;
mod winnow;

pub use decode::{
    DescribeResult, IdentityTransform, PreHashOutcome, PreHashRejectReason, PreHashTransform,
    QualifiedFrame, RgbFrame, WinnowMetrics, process_video, process_video_metadata,
    process_video_with_transform, process_video_with_transform_metadata, resize_for_vlm,
    resize_for_vlm_png,
};
pub use fiducial::{
    AREA_RELATIVE_TOLERANCE, ArucoFrame, ArucoMarker, ConveyFiducialMask, MASK_SKIP_THRESHOLD,
    MAX_MARKER_PERIMETER_RATE, MIN_MARKER_PERIMETER_RATE,
};
pub use hash::{dhash, format_dhash};
pub use winnow::{HashedFrame, WinnowConfig, WinnowCounters, WinnowState, WinnowVerdict, winnow};
