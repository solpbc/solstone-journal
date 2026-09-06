// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Parity: solstone/observe/processing_record.py:23 (documented only, not mechanically enforced).
pub const SCHEMA: &str = "solstone.processing.v1";
// Parity: solstone/observe/processing_record.py:24 (documented only, not mechanically enforced).
pub const FAILED_ATTEMPT_BOUND: i64 = 3;

// Parity: solstone/observe/processing_record.py:31.
pub const STATE_ANALYZED: &str = "analyzed";
// Parity: solstone/observe/processing_record.py:32.
pub const STATE_EMPTY: &str = "empty";
// Parity: solstone/observe/processing_record.py:33.
pub const STATE_FAILED: &str = "failed";

// Parity: solstone/observe/processing_record.py:37.
pub const REASON_OK: &str = "ok";
// Parity: solstone/observe/processing_record.py:38.
pub const REASON_NO_DECODABLE_FRAMES: &str = "no_decodable_frames";
// Parity: solstone/observe/processing_record.py:39.
pub const REASON_NO_DECODABLE_AUDIO: &str = "no_decodable_audio";
/// Audio decoded successfully, but VAD found no usable speech.
pub const REASON_NO_SPEECH: &str = "no_speech";
/// Speech was submitted to STT, which returned no transcript statements.
pub const REASON_NO_TRANSCRIPT: &str = "no_transcript";
// Parity: solstone/observe/processing_record.py:40.
pub const REASON_CORRUPT_INPUT: &str = "corrupt_input";
// Parity: solstone/observe/processing_record.py:41.
pub const REASON_ANALYSIS_FAILED: &str = "analysis_failed";

// Parity: solstone/observe/processing_record.py:44.
pub const HANDLER_DESCRIBE: &str = "describe";
// Parity: solstone/observe/processing_record.py:45.
pub const HANDLER_TRANSCRIBE: &str = "transcribe";
// Rust-native: no Python parity counterpart.
pub const HANDLER_DEPICT: &str = "depict";

/// The JSONL row key proving screen (`describe`) analysis rows exist.
pub const SCREEN_ANALYSIS_ROW_KEY: &str = "timestamp";

/// The JSONL row key proving audio (`transcribe`) transcript rows exist.
pub const AUDIO_TRANSCRIPT_ROW_KEY: &str = "start";

/// The JSONL row key proving still-image (`depict`) analysis rows exist.
pub const IMAGE_ANALYSIS_ROW_KEY: &str = "text";

/// How much of a sidecar may be read to find its metadata header.
///
/// ⛔ A bound, not a buffer size. A sidecar whose first line exceeds this has no
/// readable record, which holds the raw rather than releasing it.
pub const MAX_FIRST_ROW_BYTES: usize = 64 * 1024;
