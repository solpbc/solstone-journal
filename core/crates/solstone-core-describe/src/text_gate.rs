// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The text gate keeps a frame the image hash calls unchanged when its text has changed.
//!
//! The hash reads a 9x8 thumbnail, so a new chat message, a new inbox row or a scrolled
//! column of same-styled text rarely crosses its threshold. On a frame the winnow drops
//! as below threshold, at least [`MIN_SECONDS`] after the last kept frame and the last
//! check, and only when pixels moved since then, the gate reads the frame with the
//! screen-text helper and compares its lines with the last kept frame's. Enough new
//! lines or words keep the frame.
//!
//! The text exists only for that comparison. Nothing the gate reads is written
//! anywhere; a kept frame reaches the screen-text record through the normal path.

use std::collections::HashSet;

use crate::WinnowVerdict;
use crate::decode::{RgbFrame, encode_png};
use crate::screen_ocr::{OcrWorker, OcrWorkerFactory, RawOccurrence};

/// Seconds after the last kept frame, and between checks, before the gate reads a frame.
const MIN_SECONDS: f64 = 5.0;
/// A frame with this many new lines is kept.
const MIN_NEW_LINES: usize = 4;
/// A frame with this many words on new lines is kept.
const MIN_NEW_WORDS: usize = 15;
/// Reads per video. Static screens cost nothing; this bounds a screen that never settles.
const MAX_CHECKS: usize = 20;
/// Motion is judged on a thumbnail this many times smaller in each dimension.
const MOTION_SCALE: usize = 8;
/// A thumbnail pixel moved when its brightness changed by more than this.
const MOTION_LEVEL: u8 = 12;
/// The frame moved when more than this fraction of its thumbnail pixels moved.
const MOTION_FRACTION: f64 = 0.0005;
/// Lines below this recognition confidence are not compared.
const MIN_CONFIDENCE: f32 = 0.85;
/// Two lines whose character similarity reaches this percentage are the same line.
const SAME_LINE_RATIO: f64 = 88.0;

/// Decides, frame by frame, whether a frame the hash dropped should be kept.
pub(crate) struct TextGate<'a> {
    factory: &'a mut dyn OcrWorkerFactory,
    reader: Reader,
    motion: Motion,
    moved: bool,
    anchor: Option<Anchor>,
    last_check: Option<f64>,
    checks: usize,
}

enum Reader {
    NotStarted,
    Ready(Box<dyn OcrWorker>),
    Off,
}

struct Anchor {
    timestamp: f64,
    /// The kept frame's pixels until a check first needs its text.
    pixels: Option<RgbFrame>,
    lines: Option<Lines>,
}

impl<'a> TextGate<'a> {
    pub(crate) fn new(factory: &'a mut dyn OcrWorkerFactory) -> Self {
        Self {
            factory,
            reader: Reader::NotStarted,
            motion: Motion::default(),
            moved: false,
            anchor: None,
            last_check: None,
            checks: 0,
        }
    }

    /// Sees every decoded frame with its hash verdict. `true` keeps a below-threshold frame.
    pub(crate) fn frame(
        &mut self,
        timestamp: f64,
        frame: &RgbFrame,
        verdict: WinnowVerdict,
    ) -> bool {
        let moved = self.motion.moved(frame);
        match verdict {
            WinnowVerdict::Kept | WinnowVerdict::SceneCut | WinnowVerdict::TextChanged => {
                self.anchor = Some(Anchor {
                    timestamp,
                    pixels: Some(frame.clone()),
                    lines: None,
                });
                self.moved = false;
                return false;
            }
            WinnowVerdict::StrideDropped => {
                self.moved |= moved;
                return false;
            }
            WinnowVerdict::BelowThreshold => self.moved |= moved,
        }
        let Some(anchor_timestamp) = self.anchor.as_ref().map(|anchor| anchor.timestamp) else {
            return false;
        };
        if timestamp - anchor_timestamp < MIN_SECONDS
            || self
                .last_check
                .is_some_and(|last| timestamp - last < MIN_SECONDS)
            || !self.moved
            || self.checks >= MAX_CHECKS
            || matches!(self.reader, Reader::Off)
        {
            return false;
        }
        self.last_check = Some(timestamp);
        self.moved = false;
        self.checks += 1;
        let Some(current) = self.read(frame) else {
            return false;
        };
        let Some(anchor) = self.anchor_lines() else {
            return false;
        };
        if !anchor.has_new_text(&current) {
            return false;
        }
        self.anchor = Some(Anchor {
            timestamp,
            pixels: None,
            lines: Some(current),
        });
        true
    }

    fn anchor_lines(&mut self) -> Option<&Lines> {
        let pixels = self.anchor.as_mut()?.pixels.take();
        if let Some(pixels) = pixels {
            let lines = self.read(&pixels)?;
            self.anchor.as_mut()?.lines = Some(lines);
        }
        self.anchor.as_ref()?.lines.as_ref()
    }

    /// Any failure turns the gate off for the rest of the video; what it kept stays kept.
    fn read(&mut self, frame: &RgbFrame) -> Option<Lines> {
        if matches!(self.reader, Reader::NotStarted) {
            self.reader = match self.factory.create() {
                Ok(Some(worker)) => Reader::Ready(worker),
                Ok(None) | Err(_) => Reader::Off,
            };
        }
        let Reader::Ready(worker) = &mut self.reader else {
            return None;
        };
        let lines = encode_png(frame)
            .and_then(|png| worker.recognize(&png).ok())
            .map(|occurrences| Lines::from_occurrences(&occurrences));
        if lines.is_none() {
            self.reader = Reader::Off;
        }
        lines
    }
}

/// Brightness thumbnails of consecutive frames, compared pixel by pixel.
#[derive(Default)]
struct Motion {
    previous: Option<(usize, usize, Vec<u8>)>,
}

impl Motion {
    fn moved(&mut self, frame: &RgbFrame) -> bool {
        let current = thumbnail(frame);
        let moved = match &self.previous {
            _ if current.2.is_empty() => false,
            Some((width, height, previous)) if (*width, *height) == (current.0, current.1) => {
                let changed = previous
                    .iter()
                    .zip(&current.2)
                    .filter(|(before, after)| before.abs_diff(**after) > MOTION_LEVEL)
                    .count();
                changed as f64 > MOTION_FRACTION * previous.len() as f64
            }
            // A new frame size is a new screen.
            Some(_) => true,
            None => false,
        };
        self.previous = Some(current);
        moved
    }
}

fn thumbnail(frame: &RgbFrame) -> (usize, usize, Vec<u8>) {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let (small_width, small_height) = (width / MOTION_SCALE, height / MOTION_SCALE);
    if small_width == 0 || small_height == 0 {
        return (0, 0, Vec::new());
    }
    let mut sums = vec![0_u32; small_width * small_height];
    for (y, row) in frame
        .pixels
        .chunks_exact(width * 3)
        .take(small_height * MOTION_SCALE)
        .enumerate()
    {
        let sums = &mut sums[(y / MOTION_SCALE) * small_width..][..small_width];
        for (x, rgb) in row
            .chunks_exact(3)
            .take(small_width * MOTION_SCALE)
            .enumerate()
        {
            sums[x / MOTION_SCALE] += luma(rgb);
        }
    }
    let cell = (MOTION_SCALE * MOTION_SCALE) as u32;
    let pixels = sums.into_iter().map(|sum| (sum / cell) as u8).collect();
    (small_width, small_height, pixels)
}

fn luma(rgb: &[u8]) -> u32 {
    (u32::from(rgb[0]) * 19_595 + u32::from(rgb[1]) * 38_470 + u32::from(rgb[2]) * 7_471 + 0x8000)
        >> 16
}

/// The lines of one frame worth comparing.
#[derive(Debug)]
struct Lines {
    lines: Vec<String>,
}

impl Lines {
    fn from_occurrences(occurrences: &[RawOccurrence]) -> Self {
        Self {
            lines: occurrences
                .iter()
                .filter(|occurrence| occurrence.confidence.unwrap_or(0.0) >= MIN_CONFIDENCE)
                .filter_map(|occurrence| comparable(&occurrence.text))
                .collect(),
        }
    }

    /// Whether `current` holds enough lines this frame does not.
    fn has_new_text(&self, current: &Lines) -> bool {
        let known = self
            .lines
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let known_chars = self
            .lines
            .iter()
            .map(|line| line.chars().collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let (mut lines, mut words) = (0, 0);
        for line in &current.lines {
            if known.contains(line.as_str()) {
                continue;
            }
            let chars = line.chars().collect::<Vec<_>>();
            let seen = self
                .lines
                .iter()
                .zip(&known_chars)
                .any(|(other, other_chars)| {
                    other.contains(line.as_str())
                        || line.contains(other.as_str())
                        || same_line(&chars, other_chars)
                });
            if !seen {
                lines += 1;
                words += line.split(' ').count();
            }
        }
        lines >= MIN_NEW_LINES || words >= MIN_NEW_WORDS
    }
}

/// Lowercased words without punctuation, kept when long enough to mean something and not
/// a clock.
fn comparable(text: &str) -> Option<String> {
    let cleaned = text
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>();
    let line = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let length = line.chars().count();
    let words = line.split(' ').count();
    let long_enough = (words >= 3 && length >= 10) || length >= 14;
    (long_enough && !is_clock(&line)).then_some(line)
}

/// `12 45`, `12 45 30`, `12 45 pm`, `12 45pm`, or a bare number.
fn is_clock(line: &str) -> bool {
    let digits = |part: &str, sizes: &[usize]| {
        sizes.contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit())
    };
    let base = line
        .strip_suffix("am")
        .or_else(|| line.strip_suffix("pm"))
        .map_or(line, |rest| rest.strip_suffix(' ').unwrap_or(rest));
    let parts = base.split(' ').collect::<Vec<_>>();
    match parts.as_slice() {
        [number] => {
            base.len() == line.len() && !number.is_empty() && digits(number, &[number.len()])
        }
        [hour, minute] => digits(hour, &[1, 2]) && digits(minute, &[2]),
        [hour, minute, second] => {
            digits(hour, &[1, 2]) && digits(minute, &[2]) && digits(second, &[2])
        }
        _ => false,
    }
}

/// Character similarity, `2 * LCS / (len a + len b)`, at or above [`SAME_LINE_RATIO`].
fn same_line(a: &[char], b: &[char]) -> bool {
    let total = (a.len() + b.len()) as f64;
    if total == 0.0 || 200.0 * a.len().min(b.len()) as f64 / total < SAME_LINE_RATIO {
        return false;
    }
    200.0 * common_subsequence(a, b) as f64 / total >= SAME_LINE_RATIO
}

fn common_subsequence(a: &[char], b: &[char]) -> usize {
    let mut previous = vec![0_usize; b.len() + 1];
    let mut current = vec![0_usize; b.len() + 1];
    for left in a {
        for (index, right) in b.iter().enumerate() {
            current[index + 1] = if left == right {
                previous[index] + 1
            } else {
                previous[index + 1].max(current[index])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use super::{Lines, MAX_CHECKS, TextGate, comparable, is_clock};
    use crate::WinnowVerdict::{BelowThreshold, Kept};
    use crate::decode::RgbFrame;
    use crate::screen_ocr::{OcrFailure, OcrWorker, OcrWorkerFactory, RawOccurrence};

    /// Answers each read with the next page of lines, and counts reads.
    #[derive(Clone, Default)]
    struct Pages(Rc<RefCell<PageState>>);

    #[derive(Default)]
    struct PageState {
        pages: VecDeque<Vec<&'static str>>,
        reads: usize,
    }

    impl Pages {
        fn new(pages: &[&[&'static str]]) -> Self {
            Self(Rc::new(RefCell::new(PageState {
                pages: pages.iter().map(|page| page.to_vec()).collect(),
                reads: 0,
            })))
        }

        fn reads(&self) -> usize {
            self.0.borrow().reads
        }
    }

    impl OcrWorker for Pages {
        fn recognize(&mut self, _png: &[u8]) -> Result<Vec<RawOccurrence>, OcrFailure> {
            let mut state = self.0.borrow_mut();
            state.reads += 1;
            let page = state.pages.pop_front().ok_or(OcrFailure::Runtime)?;
            Ok(page
                .into_iter()
                .enumerate()
                .map(|(order, text)| RawOccurrence {
                    line_order: order as u32,
                    text: text.to_owned(),
                    confidence: Some(0.99),
                    geometry: Vec::new(),
                    recurrence: 0,
                })
                .collect())
        }
    }

    impl OcrWorkerFactory for Pages {
        fn recipe_id(&mut self) -> String {
            "pages".to_owned()
        }

        fn create(&mut self) -> Result<Option<Box<dyn OcrWorker>>, OcrFailure> {
            Ok(Some(Box::new(self.clone())))
        }
    }

    fn screen(shade: u8) -> RgbFrame {
        RgbFrame::new(64, 64, vec![shade; 64 * 64 * 3]).expect("frame")
    }

    const INBOX: [&str; 3] = [
        "Weekly planning notes from the design review",
        "Your build is ready to test on your devices",
        "Receipt for your order of paper towels",
    ];
    const NEW_ROWS: [&str; 4] = [
        "Can you look at the draft before Friday",
        "The meeting moved to the small conference room",
        "Reminder: library books are due on Tuesday",
        "Thanks again for dinner last night, it was lovely",
    ];

    #[test]
    fn new_rows_under_the_hash_threshold_keep_a_frame_and_a_still_screen_reads_nothing() {
        let next: Vec<&str> = INBOX.iter().chain(&NEW_ROWS).copied().collect();
        let mut pages = Pages::new(&[&next, &INBOX]);
        let reads = pages.clone();
        let mut gate = TextGate::new(&mut pages);
        assert!(!gate.frame(0.0, &screen(10), Kept));
        // A still screen is never read, however long it stays.
        for second in 1..600 {
            assert!(!gate.frame(f64::from(second), &screen(10), BelowThreshold));
        }
        assert_eq!(reads.reads(), 0);
        // Something moved; the next eligible frame is read and compared with the kept one.
        assert!(gate.frame(600.0, &screen(200), BelowThreshold));
        assert_eq!(reads.reads(), 2);
    }

    #[test]
    fn the_same_lines_split_or_misread_are_not_new() {
        let anchor = Lines {
            lines: [
                "the quarterly report is attached for your review today",
                "please send the signed copy back",
            ]
            .map(|line| comparable(line).expect("comparable"))
            .to_vec(),
        };
        let current = Lines {
            lines: [
                "the quarterly report is attached",
                "for your review today",
                "please send the signed c0py back",
                "the quarterly report is attached for your review today",
            ]
            .iter()
            .filter_map(|line| comparable(line))
            .collect(),
        };
        assert!(!anchor.has_new_text(&current));
        let fresh = Lines {
            lines: NEW_ROWS
                .iter()
                .filter_map(|line| comparable(line))
                .collect(),
        };
        assert!(anchor.has_new_text(&fresh));
    }

    #[test]
    fn checks_stop_at_the_cap_on_a_screen_that_never_settles() {
        let pages = vec![&INBOX[..]; 2 * MAX_CHECKS];
        let mut pages = Pages::new(&pages);
        let reads = pages.clone();
        let mut gate = TextGate::new(&mut pages);
        assert!(!gate.frame(0.0, &screen(0), Kept));
        for second in 1..3_600 {
            let shade = if second % 2 == 0 { 0 } else { 255 };
            assert!(!gate.frame(f64::from(second), &screen(shade), BelowThreshold));
        }
        // One read of the kept frame, then one per check.
        assert_eq!(reads.reads(), MAX_CHECKS + 1);
    }

    #[test]
    fn clocks_and_short_fragments_are_not_compared() {
        for clock in ["10 45", "9 05 pm", "10 45 30", "10 45am", "2026"] {
            assert!(is_clock(clock), "{clock}");
        }
        assert!(!is_clock("10 45 meeting with the team"));
        assert_eq!(comparable("10:45 PM"), None);
        assert_eq!(comparable("File  Edit"), None);
        assert_eq!(
            comparable("Re: The Plan, v2!").as_deref(),
            Some("re the plan v2")
        );
    }
}
