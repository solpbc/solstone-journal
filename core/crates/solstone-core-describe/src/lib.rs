//! Decode and winnow screencast video frames for `journal describe`.

mod decode;
mod fiducial;
mod hash;
mod winnow;

pub use decode::{
    DescribeResult, IdentityTransform, PreHashOutcome, PreHashRejectReason, PreHashTransform,
    QualifiedFrame, RgbFrame, WinnowMetrics, process_video, process_video_with_transform,
};
pub use fiducial::{
    AREA_RELATIVE_TOLERANCE, ArucoFrame, ArucoMarker, ConveyFiducialMask, MASK_SKIP_THRESHOLD,
    MAX_MARKER_PERIMETER_RATE, MIN_MARKER_PERIMETER_RATE,
};
pub use hash::{dhash, format_dhash};
pub use winnow::{HashedFrame, WinnowConfig, WinnowCounters, WinnowState, WinnowVerdict, winnow};
