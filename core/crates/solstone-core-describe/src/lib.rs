// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Decode and winnow screencast video frames for `journal describe`.

mod bound;
mod bounding;
pub mod categories;
mod decode;
pub mod detect;
pub mod extraction;
mod hash;
mod merge;
mod notify;
pub mod pipeline;
pub mod request;
pub(crate) mod screen_ocr;
pub mod selection;
pub mod session;
mod settings;
mod winnow;

pub use bound::DescribeWorkBound;
pub use decode::{
    DescribeResult, QualifiedFrame, WinnowMetrics, process_video, process_video_metadata,
    resize_for_vlm, resize_for_vlm_png,
};
pub use hash::format_dhash;
pub use screen_ocr::{MobileOcrGap, OcrFailure, OcrWorker, OcrWorkerFactory, RawOccurrence};
pub use settings::{DescribeSettings, read_config};
pub use winnow::{HashedFrame, WinnowConfig, WinnowCounters, WinnowState, WinnowVerdict, winnow};
