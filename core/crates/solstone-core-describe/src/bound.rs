// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Caller-owned limits for one in-process describe of one file.

use std::sync::atomic::{AtomicBool, Ordering};

/// How many images and native contexts one describe may hold.
///
/// `max_png_slots` is the generate window: the same `jobs * 2` depth the
/// pipeline already submits, and at least one slot.
#[derive(Debug)]
pub struct DescribeWorkBound {
    pub max_png_slots: usize,
    pub detector_contexts: usize,
    pub ocr_contexts: usize,
    cancel: AtomicBool,
}

impl DescribeWorkBound {
    pub fn for_jobs(jobs: usize) -> Self {
        Self {
            max_png_slots: jobs.saturating_mul(2).max(1),
            detector_contexts: 1,
            ocr_contexts: 1,
            cancel: AtomicBool::new(false),
        }
    }

    /// The caller stores `true` here to cancel. Describe only reads it.
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::DescribeWorkBound;

    #[test]
    fn jobs_set_the_png_window_and_the_caller_owns_cancel() {
        let one = DescribeWorkBound::for_jobs(1);
        assert_eq!(one.max_png_slots, 2);
        assert_eq!(one.detector_contexts, 1);
        assert_eq!(one.ocr_contexts, 1);
        assert!(!one.is_cancelled());
        one.cancel_flag().store(true, Ordering::Release);
        assert!(one.is_cancelled());

        let four = DescribeWorkBound::for_jobs(4);
        assert_eq!(four.max_png_slots, 8);
        assert_eq!(four.detector_contexts, 1);
        assert_eq!(four.ocr_contexts, 1);
        four.cancel_flag().store(true, Ordering::Release);
        assert!(four.is_cancelled());
    }
}
