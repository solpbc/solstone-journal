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

pub use decode::{UnitDecoder, decode_f32_mono, decode_f32_mono_bounded};
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
    #[error("decoded sample budget {max_samples} exceeded for {path}")]
    DecodedSampleLimit { path: PathBuf, max_samples: usize },
    #[error("no audio stream in {path}")]
    NoAudioStream { path: PathBuf },
    #[error("no audio samples decoded from {path}")]
    NoDecodedAudio { path: PathBuf },
    #[error("audio input is empty: {path}")]
    EmptyInput { path: PathBuf },
    #[error("corrupt audio input {path}: {detail}")]
    CorruptInput {
        path: PathBuf,
        detail: String,
        errno: Option<i32>,
    },
    #[error("I/O error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("FFmpeg error for {path}: {detail}")]
    Ffmpeg {
        path: PathBuf,
        detail: String,
        errno: Option<i32>,
    },
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

impl From<&AudioError> for solstone_core_cli_copy::MessageDetail {
    fn from(error: &AudioError) -> Self {
        match error {
            AudioError::DecodedSampleLimit { path, max_samples } => Self::message(
                "journal.cli.transcribe.error.audio.budget",
                [("limit", &(max_samples)), ("path", &(path.display()))],
            ),
            AudioError::NoAudioStream { path } => Self::message(
                "journal.cli.transcribe.error.audio.stream",
                [("path", &(path.display()))],
            ),
            AudioError::NoDecodedAudio { path } => Self::message(
                "journal.cli.transcribe.error.audio.samples",
                [("path", &(path.display()))],
            ),
            AudioError::EmptyInput { path } => Self::message(
                "journal.cli.transcribe.error.audio.empty",
                [("path", &(path.display()))],
            ),
            AudioError::CorruptInput { path, detail, .. } => Self::message(
                "journal.cli.transcribe.error.audio.corrupt",
                [("path", &(path.display())), ("detail", &(detail))],
            ),
            AudioError::Io { path, source } => Self::message(
                "journal.cli.transcribe.error.audio.io",
                [("path", &(path.display())), ("error", &(source))],
            ),
            AudioError::Ffmpeg { path, detail, .. } => Self::message(
                "journal.cli.transcribe.error.audio.ffmpeg",
                [("path", &(path.display())), ("detail", &(detail))],
            ),
            AudioError::SidecarCreate { path, source } => Self::message(
                "journal.cli.transcribe.error.audio.sidecar.create",
                [("path", &(path.display())), ("error", &(source))],
            ),
            AudioError::SidecarWrite { path, source } => Self::message(
                "journal.cli.transcribe.error.audio.sidecar.write",
                [("path", &(path.display())), ("error", &(source))],
            ),
            AudioError::SidecarSync { path, source } => Self::message(
                "journal.cli.transcribe.error.audio.sidecar.sync",
                [("path", &(path.display())), ("error", &(source))],
            ),
            AudioError::InvalidWavSampleRate { sample_rate } => Self::message(
                "journal.cli.transcribe.error.audio.sample.rate",
                [("rate", &(sample_rate))],
            ),
            AudioError::WavDataTooLarge { samples } => Self::cardinal(
                "journal.cli.transcribe.error.audio.wav.size",
                [("samples", *samples as i64)],
                error.to_string(),
                [],
            ),
            AudioError::ResamplerFlushDidNotConverge {
                path,
                iterations,
                flushed_samples,
                remaining_samples,
            } => Self::cardinal(
                "journal.cli.transcribe.error.audio.flush",
                [
                    ("iterations", *iterations as i64),
                    ("flushed", *flushed_samples as i64),
                ],
                error.to_string(),
                [("path", &path.display()), ("remaining", remaining_samples)],
            ),
        }
    }
}

impl AudioError {
    pub fn ffmpeg_errno(&self) -> Option<i32> {
        match self {
            Self::CorruptInput { errno, .. } | Self::Ffmpeg { errno, .. } => *errno,
            _ => None,
        }
    }

    pub fn is_transient_decode(&self) -> bool {
        self.ffmpeg_errno()
            .is_some_and(decode::is_transient_ffmpeg_errno)
    }
}
