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
}

impl Default for WinnowConfig {
    fn default() -> Self {
        Self {
            dhash_threshold: 8,
            scene_cut_threshold: 25,
            min_stride_seconds: 5.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WinnowVerdict {
    BelowThreshold,
    SceneCut,
    StrideDropped,
    Kept,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WinnowCounters {
    pub dhash_qualified: usize,
    pub scene_cut: usize,
    pub stride_dropped: usize,
    pub kept: usize,
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
        Self {
            config,
            last_kept: None,
            counters: WinnowCounters::default(),
        }
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
    use super::{HashedFrame, WinnowConfig, WinnowVerdict, winnow};

    fn config() -> WinnowConfig {
        WinnowConfig {
            dhash_threshold: 3,
            scene_cut_threshold: 8,
            min_stride_seconds: 5.0,
        }
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
    fn omitting_a_masked_frame_keeps_the_last_kept_reference() {
        let anchor = HashedFrame {
            timestamp: 0.0,
            hash: 0,
        };
        let masked = HashedFrame {
            timestamp: 6.0,
            hash: 0b1111_1111,
        };
        let later = HashedFrame {
            timestamp: 7.0,
            hash: 0b1111_1111,
        };
        let (without_masked, _) = winnow(&[anchor, later], &config());
        let (with_masked, _) = winnow(&[anchor, masked, later], &config());
        assert_eq!(without_masked[1], WinnowVerdict::SceneCut);
        assert_eq!(with_masked[2], WinnowVerdict::BelowThreshold);
        assert_ne!(without_masked[1], with_masked[2]);
    }
}
