//! Decode and winnow screencast video frames for `journal describe`.

mod decode;
mod hash;
mod winnow;

pub use decode::{
    DescribeResult, IdentityTransform, PreHashOutcome, PreHashTransform, QualifiedFrame, RgbFrame,
    WinnowMetrics, process_video, process_video_with_transform,
};
pub use hash::{dhash, format_dhash};
pub use winnow::{HashedFrame, WinnowConfig, WinnowCounters, WinnowState, WinnowVerdict, winnow};
