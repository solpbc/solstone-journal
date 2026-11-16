// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// A frame after PTS and pre-hash filtering, ready for winnowing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HashedFrame {
    pub timestamp: f64,
    pub hash: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WinnowConfig {
    pub dhash_threshold: u32,
    pub scene_cut_threshold: u32,
    pub min_stride_seconds: f64,
    /// The hash of the same screen's last kept frame in the previous segment. The first
    /// frame is compared with it instead of being kept outright, so a screen that has not
    /// changed across the boundary is not described again. `None` keeps the first frame.
    pub start_reference: Option<u64>,
}

impl Default for WinnowConfig {
    fn default() -> Self {
        Self {
            dhash_threshold: 8,
            scene_cut_threshold: 25,
            min_stride_seconds: 5.0,
            start_reference: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WinnowVerdict {
    BelowThreshold,
    SceneCut,
    StrideDropped,
    Kept,
    /// Below the hash threshold, but the text gate saw new lines on screen.
    TextChanged,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WinnowCounters {
    pub dhash_qualified: usize,
    pub scene_cut: usize,
    pub stride_dropped: usize,
    pub kept: usize,
    pub text_changed: usize,
}

/// Streaming form of [`winnow`], sharing the same last-kept reference rules.
#[derive(Clone, Debug)]
pub struct WinnowState {
    config: WinnowConfig,
    last_kept: Option<HashedFrame>,
    counters: WinnowCounters,
}

impl WinnowState {
    pub fn new(config: WinnowConfig) -> Self {
        // The previous segment's frame was kept long before this video began, so the
        // stride floor never holds back this video's first change.
        let last_kept = config.start_reference.map(|hash| HashedFrame {
            timestamp: f64::NEG_INFINITY,
            hash,
        });
        Self {
            config,
            last_kept,
            counters: WinnowCounters::default(),
        }
    }

    /// Whether a later frame is compared with something: a kept frame, or the start
    /// reference before any frame was kept.
    pub fn has_reference(&self) -> bool {
        self.last_kept.is_some()
    }

    pub fn decide(&mut self, frame: HashedFrame) -> WinnowVerdict {
        let Some(last_kept) = self.last_kept else {
            self.last_kept = Some(frame);
            self.counters.dhash_qualified += 1;
            self.counters.kept += 1;
            return WinnowVerdict::Kept;
        };

        let distance = (last_kept.hash ^ frame.hash).count_ones();
        if distance < self.config.dhash_threshold {
            return WinnowVerdict::BelowThreshold;
        }
        if distance >= self.config.scene_cut_threshold {
            self.last_kept = Some(frame);
            self.counters.dhash_qualified += 1;
            self.counters.scene_cut += 1;
            self.counters.kept += 1;
            return WinnowVerdict::SceneCut;
        }
        self.counters.dhash_qualified += 1;
        if frame.timestamp - last_kept.timestamp < self.config.min_stride_seconds {
            self.counters.stride_dropped += 1;
            return WinnowVerdict::StrideDropped;
        }

        self.last_kept = Some(frame);
        self.counters.kept += 1;
        WinnowVerdict::Kept
    }

    /// Keep a frame [`Self::decide`] called [`WinnowVerdict::BelowThreshold`]. It becomes the
    /// reference, so later hash comparisons start from it.
    pub fn promote(&mut self, frame: HashedFrame) -> WinnowVerdict {
        self.last_kept = Some(frame);
        self.counters.kept += 1;
        self.counters.text_changed += 1;
        WinnowVerdict::TextChanged
    }

    pub fn counters(&self) -> WinnowCounters {
        self.counters
    }
}

/// Winnow a complete sequence without any video-decoding dependency.
pub fn winnow(
    frames: &[HashedFrame],
    config: &WinnowConfig,
) -> (Vec<WinnowVerdict>, WinnowCounters) {
    let mut state = WinnowState::new(*config);
    let verdicts = frames
        .iter()
        .copied()
        .map(|frame| state.decide(frame))
        .collect();
    (verdicts, state.counters())
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::{HashedFrame, WinnowConfig, WinnowState, WinnowVerdict, winnow};

    fn config() -> WinnowConfig {
        WinnowConfig {
            dhash_threshold: 3,
            scene_cut_threshold: 8,
            min_stride_seconds: 5.0,
            start_reference: None,
        }
    }

    #[test]
    fn static_frames_do_not_gain_a_heartbeat_after_hours() {
        let mut state = WinnowState::new(WinnowConfig::default());
        for second in 0..10_800 {
            let verdict = state.decide(HashedFrame {
                timestamp: f64::from(second),
                hash: 0x0123_4567_89ab_cdef,
            });
            assert_eq!(
                verdict,
                if second == 0 {
                    WinnowVerdict::Kept
                } else {
                    WinnowVerdict::BelowThreshold
                }
            );
        }
        assert_eq!(state.counters().kept, 1);
    }

    #[test]
    fn emits_each_verdict() {
        let frames = [
            HashedFrame {
                timestamp: 0.0,
                hash: 0,
            },
            HashedFrame {
                timestamp: 1.0,
                hash: 0b1,
            },
            HashedFrame {
                timestamp: 1.0,
                hash: 0b111,
            },
            HashedFrame {
                timestamp: 2.0,
                hash: 0b1_1111_1111,
            },
        ];
        let (verdicts, counters) = winnow(&frames, &config());
        assert_eq!(
            verdicts,
            [
                WinnowVerdict::Kept,
                WinnowVerdict::BelowThreshold,
                WinnowVerdict::StrideDropped,
                WinnowVerdict::SceneCut,
            ]
        );
        assert_eq!(counters.dhash_qualified, 3);
        assert_eq!(counters.scene_cut, 1);
        assert_eq!(counters.stride_dropped, 1);
        assert_eq!(counters.kept, 2);
    }

    #[test]
    fn stride_floor_is_strict() {
        let frames = [
            HashedFrame {
                timestamp: 0.0,
                hash: 0,
            },
            HashedFrame {
                timestamp: 4.999,
                hash: 0b111,
            },
            HashedFrame {
                timestamp: 5.0,
                hash: 0b111,
            },
        ];
        let (verdicts, _) = winnow(&frames, &config());
        assert_eq!(verdicts[1], WinnowVerdict::StrideDropped);
        assert_eq!(verdicts[2], WinnowVerdict::Kept);
    }

    #[test]
    fn compares_against_last_kept_frame_after_below_threshold() {
        let frames = [
            HashedFrame {
                timestamp: 0.0,
                hash: 0,
            },
            HashedFrame {
                timestamp: 6.0,
                hash: 0b11,
            },
            HashedFrame {
                timestamp: 6.0,
                hash: 0b111,
            },
        ];
        let (verdicts, _) = winnow(&frames, &config());
        assert_eq!(
            verdicts,
            [
                WinnowVerdict::Kept,
                WinnowVerdict::BelowThreshold,
                WinnowVerdict::Kept
            ]
        );
    }

    #[test]
    fn a_promoted_frame_becomes_the_reference() {
        let mut state = WinnowState::new(config());
        let frame = |timestamp, hash| HashedFrame { timestamp, hash };
        assert_eq!(state.decide(frame(0.0, 0)), WinnowVerdict::Kept);
        assert_eq!(
            state.decide(frame(6.0, 0b11)),
            WinnowVerdict::BelowThreshold
        );
        assert_eq!(state.promote(frame(6.0, 0b11)), WinnowVerdict::TextChanged);
        // Three bits from the first frame, one from the promoted one.
        assert_eq!(
            state.decide(frame(12.0, 0b111)),
            WinnowVerdict::BelowThreshold
        );
        let counters = state.counters();
        assert_eq!(
            (
                counters.kept,
                counters.text_changed,
                counters.dhash_qualified
            ),
            (2, 1, 1)
        );
    }

    #[test]
    fn stride_dropped_frame_does_not_advance_reference() {
        let frames = [
            HashedFrame {
                timestamp: 0.0,
                hash: 0,
            },
            HashedFrame {
                timestamp: 1.0,
                hash: 0b111,
            },
            HashedFrame {
                timestamp: 6.0,
                hash: 0b11_1111,
            },
        ];
        let (verdicts, _) = winnow(&frames, &config());
        assert_eq!(
            verdicts,
            [
                WinnowVerdict::Kept,
                WinnowVerdict::StrideDropped,
                WinnowVerdict::Kept
            ]
        );
    }
    #[test]
    fn a_screen_unchanged_since_the_previous_segment_is_not_kept_again() {
        let carried = WinnowConfig {
            start_reference: Some(0),
            ..config()
        };
        let frame = |timestamp, hash| HashedFrame { timestamp, hash };
        // Two bits from the previous segment's last kept frame: nothing new to describe.
        let (verdicts, counters) = winnow(&[frame(0.0, 0b11), frame(9.0, 0b1)], &carried);
        assert_eq!(
            verdicts,
            [WinnowVerdict::BelowThreshold, WinnowVerdict::BelowThreshold]
        );
        assert_eq!(counters.kept, 0);
        // A screen that changed across the boundary is kept as its first frame always was.
        let changed = 0b1_1111_1111;
        let (verdicts, _) = winnow(&[frame(0.0, changed), frame(1.0, changed)], &carried);
        assert_eq!(
            verdicts,
            [WinnowVerdict::SceneCut, WinnowVerdict::BelowThreshold]
        );
    }

    #[test]
    fn the_stride_floor_does_not_reach_back_into_the_previous_segment() {
        let carried = WinnowConfig {
            start_reference: Some(0),
            ..config()
        };
        let frame = HashedFrame {
            timestamp: 0.0,
            hash: 0b111,
        };
        let mut state = WinnowState::new(carried);
        assert!(state.has_reference());
        assert_eq!(state.decide(frame), WinnowVerdict::Kept);
        assert_eq!(state.counters().stride_dropped, 0);
    }
}
