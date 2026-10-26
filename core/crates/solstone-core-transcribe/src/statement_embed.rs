// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Statement-only WeSpeaker embedding over spool PCM.
//!
//! Pass one reads fixed PCM windows, writes pre-mean filterbank frames, and
//! keeps only the 80 bin sums. Pass two subtracts the statement mean per
//! prefix window and projects once. Neither pass holds the statement PCM or
//! the whole feature matrix.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use solstone_core_observe_audio::SAMPLE_RATE;
use solstone_core_speakers::{
    KaldiFrames, WESPEAKER_FRAME_LENGTH_SAMPLES, WESPEAKER_FRAME_SHIFT_SAMPLES, WESPEAKER_MEL_BINS,
    snipped_frame_count,
};
use solstone_core_speakers_onnx::{
    PlatformDescriptor, SpeakerOnnxError, WespeakerEmbedder, default_speaker_execution_providers,
};

use crate::TranscribeError;
use crate::embedded_spool::{DraftSpool, StatementAudioMeasure};
use crate::speakers::SpeakerAnalyzeError;

/// Frames of filterbank work read and written together.
const STATEMENT_FEATURE_FRAMES: usize = 32;

/// PCM samples one feature batch reads. A test walk uses the same ceiling.
pub(crate) const STATEMENT_PCM_CHUNK: usize =
    (STATEMENT_FEATURE_FRAMES - 1) * WESPEAKER_FRAME_SHIFT_SAMPLES + WESPEAKER_FRAME_LENGTH_SAMPLES;

/// Read the measured statement in non-overlapping chunks.
///
/// This is the full-tests stand-in walk. Production framing overlaps by design
/// and is not this function.
#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn visit_statement_pcm(
    spool: &mut DraftSpool,
    span_start: usize,
    span_end: usize,
    measured: &StatementAudioMeasure,
    cancel: &dyn Fn() -> bool,
) -> Result<(), std::io::Error> {
    let mut offset = 0_usize;
    while offset < measured.samples {
        let len = (measured.samples - offset).min(STATEMENT_PCM_CHUNK);
        let mut dest = vec![0.0_f32; len];
        spool.read_statement_slice(
            span_start,
            span_end,
            offset,
            &mut dest,
            measured.reduced,
            cancel,
        )?;
        offset += len;
    }
    Ok(())
}

/// Embed one admitted statement from spool PCM with the same WeSpeaker tail.
pub(crate) fn embed_statement(
    audio_path: &Path,
    models: &Path,
    spool: &mut DraftSpool,
    span_start: usize,
    span_end: usize,
    measured: &StatementAudioMeasure,
    cancel: &dyn Fn() -> bool,
) -> Result<(f64, Vec<f32>), TranscribeError> {
    let frames = snipped_frame_count(measured.samples);
    if frames == 0 || frames.div_ceil(8) < 2 {
        return Err(onnx_error(
            audio_path,
            SpeakerOnnxError::InvalidFeatureMatrix {
                frames,
                bins: WESPEAKER_MEL_BINS,
            },
        ));
    }
    let feature_path = spool.directory().join("statement-features.f32le");
    let mut features =
        create_private(&feature_path).map_err(|error| io_error(audio_path, error))?;
    let draft = FeatureDraft {
        path: feature_path.clone(),
    };
    let mut sums = [0.0_f64; WESPEAKER_MEL_BINS];
    let kaldi = KaldiFrames::new();
    let mut frame = 0_usize;
    while frame < frames {
        if cancel() {
            return Err(TranscribeError::Cancelled);
        }
        let batch = (frames - frame).min(STATEMENT_FEATURE_FRAMES);
        let start = frame * WESPEAKER_FRAME_SHIFT_SAMPLES;
        let count = (batch - 1) * WESPEAKER_FRAME_SHIFT_SAMPLES + WESPEAKER_FRAME_LENGTH_SAMPLES;
        let mut pcm = Vec::with_capacity(STATEMENT_PCM_CHUNK);
        pcm.resize(count, 0.0);
        spool
            .read_statement_slice(
                span_start,
                span_end,
                start,
                &mut pcm,
                measured.reduced,
                cancel,
            )
            .map_err(|error| io_error(audio_path, error))?;
        for (index, sample) in pcm.iter().enumerate() {
            if !sample.is_finite() {
                return Err(non_finite(audio_path, start + index));
            }
        }
        if cancel() {
            return Err(TranscribeError::Cancelled);
        }
        let mut raw = Vec::with_capacity(batch * WESPEAKER_MEL_BINS);
        for local in 0..batch {
            let at = local * WESPEAKER_FRAME_SHIFT_SAMPLES;
            let energies = kaldi
                .log_energies(&pcm[at..at + WESPEAKER_FRAME_LENGTH_SAMPLES])
                .map_err(|error| feature_error(audio_path, error))?;
            for (bin, value) in energies.iter().enumerate() {
                sums[bin] += f64::from(*value);
            }
            raw.extend_from_slice(&energies);
        }
        if cancel() {
            return Err(TranscribeError::Cancelled);
        }
        write_f32s(&mut features, &raw).map_err(|error| io_error(audio_path, error))?;
        frame += batch;
    }
    let framed = (frames - 1) * WESPEAKER_FRAME_SHIFT_SAMPLES + WESPEAKER_FRAME_LENGTH_SAMPLES;
    if framed < measured.samples {
        if cancel() {
            return Err(TranscribeError::Cancelled);
        }
        let tail_len = measured.samples - framed;
        let mut tail = vec![0.0_f32; tail_len];
        spool
            .read_statement_slice(
                span_start,
                span_end,
                framed,
                &mut tail,
                measured.reduced,
                cancel,
            )
            .map_err(|error| io_error(audio_path, error))?;
        for (index, sample) in tail.iter().enumerate() {
            if !sample.is_finite() {
                return Err(non_finite(audio_path, framed + index));
            }
        }
    }
    features
        .flush()
        .map_err(|error| io_error(audio_path, error))?;
    if cancel() {
        return Err(TranscribeError::Cancelled);
    }
    features
        .sync_all()
        .map_err(|error| io_error(audio_path, error))?;
    drop(features);
    if cancel() {
        return Err(TranscribeError::Cancelled);
    }
    let model = crate::model_assets::verify_digest(
        "wespeaker-resnet34-256.onnx",
        models.join("wespeaker-resnet34-256.onnx"),
    )
    .map_err(|error| {
        TranscribeError::SpeakerAnalysis(SpeakerAnalyzeError::new(
            audio_path,
            "model",
            error.to_string(),
            None,
        ))
    })?;
    let providers = default_speaker_execution_providers(PlatformDescriptor::current());
    let mut embedder = WespeakerEmbedder::open(&model, &providers)
        .map_err(|error| onnx_error(audio_path, error))?;
    let mean = sums.map(|sum| (sum / frames as f64) as f32);
    let mut reader =
        CenteredFeatures::open(&draft.path, frames).map_err(|error| io_error(audio_path, error))?;
    let embedding = embedder
        .embed_polled(frames, WESPEAKER_MEL_BINS, cancel, |start, end, dest| {
            reader.read_centered(start, end, &mean, dest)
        })
        .map_err(|error| onnx_error(audio_path, error))?;
    drop(reader);
    drop(draft);
    let duration = measured.samples as f64 / f64::from(SAMPLE_RATE);
    Ok((duration, embedding.values().to_vec()))
}

struct FeatureDraft {
    path: PathBuf,
}

impl Drop for FeatureDraft {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

struct CenteredFeatures {
    file: File,
    frames: usize,
}

impl CenteredFeatures {
    fn open(path: &Path, frames: usize) -> Result<Self, std::io::Error> {
        Ok(Self {
            file: File::open(path)?,
            frames,
        })
    }

    fn read_centered(
        &mut self,
        start: usize,
        end: usize,
        mean: &[f32; WESPEAKER_MEL_BINS],
        dest: &mut [f32],
    ) -> Result<(), SpeakerOnnxError> {
        if end < start || end > self.frames {
            return Err(SpeakerOnnxError::InvalidFeatureMatrix {
                frames: self.frames,
                bins: WESPEAKER_MEL_BINS,
            });
        }
        let width = (end - start) * WESPEAKER_MEL_BINS;
        if dest.len() != width {
            return Err(SpeakerOnnxError::InvalidFeatureMatrix {
                frames: self.frames,
                bins: WESPEAKER_MEL_BINS,
            });
        }
        self.file
            .seek(SeekFrom::Start(
                (start * WESPEAKER_MEL_BINS * std::mem::size_of::<f32>()) as u64,
            ))
            .map_err(|error| SpeakerOnnxError::InvalidModelIo {
                detail: error.to_string(),
            })?;
        let mut bytes = vec![0_u8; width * std::mem::size_of::<f32>()];
        self.file
            .read_exact(&mut bytes)
            .map_err(|error| SpeakerOnnxError::InvalidModelIo {
                detail: error.to_string(),
            })?;
        for (index, chunk) in bytes.chunks_exact(4).enumerate() {
            let value = f32::from_le_bytes(chunk.try_into().expect("4 bytes"));
            dest[index] = value - mean[index % WESPEAKER_MEL_BINS];
        }
        Ok(())
    }
}

fn create_private(path: &Path) -> Result<File, std::io::Error> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

fn write_f32s(file: &mut File, samples: &[f32]) -> Result<(), std::io::Error> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    file.write_all(&bytes)
}

fn io_error(audio: &Path, error: std::io::Error) -> TranscribeError {
    if error.kind() == std::io::ErrorKind::Interrupted {
        TranscribeError::Cancelled
    } else {
        TranscribeError::TranscriptPayload {
            path: Some(audio.to_path_buf()),
            detail: error.to_string(),
        }
    }
}

fn non_finite(audio: &Path, index: usize) -> TranscribeError {
    TranscribeError::SpeakerAnalysis(SpeakerAnalyzeError::new(
        audio,
        "statement",
        format!("audio sample at index {index} is not finite"),
        None,
    ))
}

fn feature_error(
    audio: &Path,
    error: solstone_core_speakers::SpeakerFeatureError,
) -> TranscribeError {
    TranscribeError::SpeakerAnalysis(SpeakerAnalyzeError::new(
        audio,
        "statement",
        error.to_string(),
        None,
    ))
}

fn onnx_error(audio: &Path, error: SpeakerOnnxError) -> TranscribeError {
    match error {
        SpeakerOnnxError::Cancelled => TranscribeError::Cancelled,
        other => TranscribeError::SpeakerAnalysis(SpeakerAnalyzeError::new(
            audio,
            "statement",
            other.to_string(),
            None,
        )),
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::embedded_spool::{
        reset_statement_reads, statement_max_read, statement_visited, statement_whole_reads,
    };

    fn tone(samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|index| ((index % 40) as f32 / 40.0) - 0.5)
            .collect()
    }

    #[test]
    fn cancel_during_statement_features_does_not_open_the_embedding() {
        let scratch = tempfile::tempdir().expect("scratch");
        let mut spool = DraftSpool::open(scratch.path()).expect("spool");
        let audio = tone(8_000);
        spool.store_unit_pcm(0, &audio, None).expect("pcm");
        let measured = spool
            .measure_statement_audio(0, audio.len(), &|| false)
            .expect("measure");
        reset_statement_reads();
        let error = embed_statement(
            Path::new("statement.wav"),
            Path::new("missing-models"),
            &mut spool,
            0,
            audio.len(),
            &measured,
            &|| true,
        )
        .expect_err("stop");
        assert!(matches!(error, TranscribeError::Cancelled), "{error}");
        assert_eq!(statement_whole_reads(), 0);
        assert!(!spool.directory().join("statement-features.f32le").exists());
    }

    #[test]
    fn statement_embedding_reads_bounded_pcm_and_returns_256_floats() {
        let scratch = tempfile::tempdir().expect("scratch");
        let mut spool = DraftSpool::open(scratch.path()).expect("spool");
        let audio = tone(8_000);
        spool.store_unit_pcm(0, &audio, None).expect("pcm");
        let measured = spool
            .measure_statement_audio(0, audio.len(), &|| false)
            .expect("measure");
        assert!(!measured.reduced);
        assert_eq!(measured.samples, audio.len());
        reset_statement_reads();
        let models = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../core/models/assets");
        let (duration, values) = embed_statement(
            Path::new("statement.wav"),
            &models,
            &mut spool,
            0,
            audio.len(),
            &measured,
            &|| false,
        )
        .expect("embedding");
        assert_eq!(
            values.len(),
            solstone_core_speakers::WESPEAKER_EMBEDDING_SIZE
        );
        assert!(values.iter().all(|value| value.is_finite()));
        assert!(duration > 0.0);
        assert_eq!(statement_whole_reads(), 0);
        assert!(statement_max_read() > 0);
        assert!(statement_max_read() <= STATEMENT_PCM_CHUNK);
        assert!(statement_visited() >= measured.samples);
        assert!(!spool.directory().join("statement-features.f32le").exists());
    }
}
