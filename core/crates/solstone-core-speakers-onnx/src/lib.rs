// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(feature = "runtime")]
mod pyannote;
#[cfg(feature = "runtime")]
mod session;
#[cfg(feature = "runtime")]
mod wespeaker;

use std::error::Error;
use std::fmt;

use solstone_core_speakers::{PYANNOTE_SAMPLE_RATE_HZ, PYANNOTE_WINDOW_S, WESPEAKER_MEL_BINS};

#[cfg(feature = "runtime")]
pub use pyannote::PyannoteSegmenter;
#[cfg(feature = "runtime")]
pub use wespeaker::{SpeakerEmbedding, WespeakerEmbedder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformFamily {
    Apple,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformDescriptor {
    pub family: PlatformFamily,
}

impl PlatformDescriptor {
    pub fn current() -> Self {
        Self {
            family: if cfg!(target_vendor = "apple") {
                PlatformFamily::Apple
            } else {
                PlatformFamily::Other
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakerExecutionProvider {
    CoreMl,
    Cpu,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeakerOnnxError {
    EmptyProviderPlan,
    ProviderUnavailable {
        provider: &'static str,
    },
    InvalidFeatureMatrix {
        frames: usize,
        bins: usize,
    },
    InvalidAudioWindow {
        expected_samples: usize,
        actual_samples: usize,
    },
    InvalidModelIo {
        detail: String,
    },
    MissingOutput {
        name: String,
    },
    Ort {
        message: String,
    },
}

impl fmt::Display for SpeakerOnnxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyProviderPlan => formatter.write_str("speaker ONNX provider plan is empty"),
            Self::ProviderUnavailable { provider } => {
                write!(
                    formatter,
                    "speaker ONNX provider is unavailable: {provider}"
                )
            }
            Self::InvalidFeatureMatrix { frames, bins } => write!(
                formatter,
                "speaker ONNX features must have at least one frame and {WESPEAKER_MEL_BINS} bins, got frames={frames} bins={bins}"
            ),
            Self::InvalidAudioWindow {
                expected_samples,
                actual_samples,
            } => write!(
                formatter,
                "pyannote ONNX audio window must have {expected_samples} samples ({PYANNOTE_WINDOW_S}s at {PYANNOTE_SAMPLE_RATE_HZ} Hz), got {actual_samples}"
            ),
            Self::InvalidModelIo { detail } => {
                write!(formatter, "speaker ONNX model IO mismatch: {detail}")
            }
            Self::MissingOutput { name } => {
                write!(formatter, "speaker ONNX output was not returned: {name}")
            }
            Self::Ort { message } => write!(formatter, "speaker ONNX Runtime error: {message}"),
        }
    }
}

impl Error for SpeakerOnnxError {}

#[cfg(feature = "runtime")]
impl<R> From<ort::Error<R>> for SpeakerOnnxError {
    fn from(error: ort::Error<R>) -> Self {
        Self::Ort {
            message: error.to_string(),
        }
    }
}

pub fn default_speaker_execution_providers(
    platform: PlatformDescriptor,
) -> Vec<SpeakerExecutionProvider> {
    match platform.family {
        PlatformFamily::Apple => vec![
            SpeakerExecutionProvider::CoreMl,
            SpeakerExecutionProvider::Cpu,
        ],
        PlatformFamily::Other => vec![SpeakerExecutionProvider::Cpu],
    }
}

#[cfg(any(feature = "runtime", test))]
fn validate_pyannote_audio_window(audio_window: &[f32]) -> Result<(), SpeakerOnnxError> {
    let expected_samples = PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize;
    if audio_window.len() != expected_samples {
        return Err(SpeakerOnnxError::InvalidAudioWindow {
            expected_samples,
            actual_samples: audio_window.len(),
        });
    }
    Ok(())
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::*;

    #[test]
    fn provider_plan_selects_coreml_then_cpu_for_synthetic_apple() {
        let plan = default_speaker_execution_providers(PlatformDescriptor {
            family: PlatformFamily::Apple,
        });

        assert_eq!(
            plan,
            vec![
                SpeakerExecutionProvider::CoreMl,
                SpeakerExecutionProvider::Cpu
            ]
        );
    }

    #[test]
    fn provider_plan_selects_cpu_for_non_apple() {
        let plan = default_speaker_execution_providers(PlatformDescriptor {
            family: PlatformFamily::Other,
        });

        assert_eq!(plan, vec![SpeakerExecutionProvider::Cpu]);
    }

    #[test]
    fn pyannote_rejects_wrong_length_window() {
        let error = validate_pyannote_audio_window(&[0.0; 42]).unwrap_err();

        assert_eq!(
            error,
            SpeakerOnnxError::InvalidAudioWindow {
                expected_samples: PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize,
                actual_samples: 42,
            }
        );
    }
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) mod test_support {
    use serde_json::Value;

    pub(crate) const FIXTURE: &str = include_str!("../../../fixtures/speaker_filterbank.json");

    pub(crate) fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
    }

    pub(crate) fn fixture() -> Value {
        serde_json::from_str(FIXTURE).expect("fixture JSON")
    }

    pub(crate) fn decode_waveform(fixture: &Value) -> Vec<f32> {
        let encoded = fixture["waveform"]["samples_f32_le_base64"]
            .as_str()
            .expect("waveform base64");
        let bytes = decode_base64(encoded);
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("f32 bytes")))
            .collect()
    }

    pub(crate) fn decode_base64(input: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len() / 4 * 3);
        let mut quartet = [0_u8; 4];
        let mut len = 0;
        for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
            quartet[len] = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' => 64,
                _ => panic!("invalid base64 byte: {byte}"),
            };
            len += 1;
            if len == 4 {
                out.push((quartet[0] << 2) | (quartet[1] >> 4));
                if quartet[2] != 64 {
                    out.push((quartet[1] << 4) | (quartet[2] >> 2));
                }
                if quartet[3] != 64 {
                    out.push((quartet[2] << 6) | quartet[3]);
                }
                len = 0;
            }
        }
        assert_eq!(len, 0);
        out
    }
}
