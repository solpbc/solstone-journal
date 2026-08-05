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
// Parity: solstone/observe/processing_record.py:40.
pub const REASON_CORRUPT_INPUT: &str = "corrupt_input";
// Parity: solstone/observe/processing_record.py:41.
pub const REASON_ANALYSIS_FAILED: &str = "analysis_failed";

// Parity: solstone/observe/processing_record.py:44.
pub const HANDLER_DESCRIBE: &str = "describe";
// Parity: solstone/observe/processing_record.py:45.
pub const HANDLER_TRANSCRIBE: &str = "transcribe";
