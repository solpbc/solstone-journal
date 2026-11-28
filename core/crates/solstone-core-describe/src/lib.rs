// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Decode and winnow screencast video frames for `journal describe`.

mod bound;
mod bounding;
mod carry;
pub mod categories;
mod decode;
pub mod detect;
pub mod extraction;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod faces;
mod hash;
mod merge;
mod notify;
pub mod phone;
pub mod pipeline;
mod publication;
pub mod request;
pub(crate) mod screen_ocr;
pub mod selection;
pub mod session;
mod settings;
mod text_gate;
mod winnow;

pub use bound::DescribeWorkBound;
pub use decode::{
    DescribeResult, QualifiedFrame, WinnowMetrics, process_video, process_video_metadata,
    resize_for_vlm, resize_for_vlm_png,
};
pub use hash::format_dhash;
pub use screen_ocr::{
    MobileOcrGap, OcrFailure, OcrWorker, OcrWorkerFactory, RawOccurrence, installed_ocr_worker,
    read_still_ocr_text,
};
pub use settings::{DescribeSettings, read_config, read_config_in};

use solstone_core_cli_copy::manual_log_locale as terminal_locale;
pub use winnow::{HashedFrame, WinnowConfig, WinnowCounters, WinnowState, WinnowVerdict, winnow};
