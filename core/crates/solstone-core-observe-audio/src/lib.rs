// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic audio-front-end primitives used by journal transcription.

use std::io;
use std::path::PathBuf;

pub mod decode;
pub mod nonspeech;
pub mod reduce;
pub mod sidecar;
pub mod wav;

pub use decode::decode_f32_mono;
pub use nonspeech::{
    DEFAULT_LOUD_RMS_THRESHOLD, DEFAULT_LOUD_WINDOW_SECONDS, MIN_NONSPEECH_SEGMENT_SECONDS,
    compute_loud_speech_windows, compute_nonspeech_rms, get_nonspeech_segments,
};
pub use reduce::{AudioReduction, SpeechInterval, SpeechSegment, VadResult, reduce_audio};
pub use sidecar::write_f32le_exclusive;
pub use wav::audio_to_wav_bytes;
pub use wav::wav_bytes_for_samples;

pub const SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no audio stream in {path}")]
    NoAudioStream { path: PathBuf },
    #[error("no audio samples decoded from {path}")]
    NoDecodedAudio { path: PathBuf },
    #[error("audio input is empty: {path}")]
    EmptyInput { path: PathBuf },
    #[error("corrupt audio input {path}: {detail}")]
    CorruptInput { path: PathBuf, detail: String },
    #[error("I/O error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("FFmpeg error for {path}: {detail}")]
    Ffmpeg { path: PathBuf, detail: String },
    #[error(
        "resampler flush did not converge for {path} after {iterations} iterations \
         ({flushed_samples} samples flushed; {remaining_samples} still delayed)"
    )]
    ResamplerFlushDidNotConverge {
        path: PathBuf,
        iterations: usize,
        flushed_samples: usize,
        remaining_samples: i64,
    },
    #[error("cannot create f32le sidecar {path}: {source}")]
    SidecarCreate {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot write f32le sidecar {path}: {source}")]
    SidecarWrite {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot sync f32le sidecar {path}: {source}")]
    SidecarSync {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("WAV data is too large for a RIFF u32 field: {samples} samples")]
    WavDataTooLarge { samples: usize },
    #[error("invalid WAV sample rate: {sample_rate}")]
    InvalidWavSampleRate { sample_rate: u32 },
}
