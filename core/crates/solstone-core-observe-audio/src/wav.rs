// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::AudioError;

const WAV_HEADER_BYTES: usize = 44;
const PCM16_BYTES_PER_SAMPLE: usize = 2;

/// The exact byte length `audio_to_wav_bytes` will produce for `samples`.
///
/// Exposed so a caller can size a request against a server's byte budget before
/// building the payload, rather than guessing from a duration.
#[must_use]
pub fn wav_bytes_for_samples(samples: usize) -> usize {
    WAV_HEADER_BYTES.saturating_add(samples.saturating_mul(PCM16_BYTES_PER_SAMPLE))
}

/// Encodes mono audio as a canonical PCM-16 RIFF/WAVE payload.
pub fn audio_to_wav_bytes(audio: &[f32], sample_rate: u32) -> Result<Vec<u8>, AudioError> {
    if sample_rate == 0 {
        return Err(AudioError::InvalidWavSampleRate { sample_rate });
    }

    let data_bytes = audio
        .len()
        .checked_mul(PCM16_BYTES_PER_SAMPLE)
        .filter(|bytes| *bytes <= u32::MAX as usize)
        .ok_or(AudioError::WavDataTooLarge {
            samples: audio.len(),
        })?;
    let riff_size = 36_usize
        .checked_add(data_bytes)
        .filter(|size| *size <= u32::MAX as usize)
        .ok_or(AudioError::WavDataTooLarge {
            samples: audio.len(),
        })?;
    let byte_rate = sample_rate
        .checked_mul(PCM16_BYTES_PER_SAMPLE as u32)
        .ok_or(AudioError::InvalidWavSampleRate { sample_rate })?;

    let mut output = Vec::with_capacity(WAV_HEADER_BYTES + data_bytes);
    output.extend_from_slice(b"RIFF");
    output.extend_from_slice(&(riff_size as u32).to_le_bytes());
    output.extend_from_slice(b"WAVEfmt ");
    output.extend_from_slice(&16_u32.to_le_bytes());
    output.extend_from_slice(&1_u16.to_le_bytes());
    output.extend_from_slice(&1_u16.to_le_bytes());
    output.extend_from_slice(&sample_rate.to_le_bytes());
    output.extend_from_slice(&byte_rate.to_le_bytes());
    output.extend_from_slice(&2_u16.to_le_bytes());
    output.extend_from_slice(&16_u16.to_le_bytes());
    output.extend_from_slice(b"data");
    output.extend_from_slice(&(data_bytes as u32).to_le_bytes());

    for sample in audio {
        let pcm = (sample * 32_768.0).floor().clamp(-32_768.0, 32_767.0) as i16;
        output.extend_from_slice(&pcm.to_le_bytes());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::audio_to_wav_bytes;

    #[test]
    fn writes_canonical_empty_wav() {
        let wav = audio_to_wav_bytes(&[], 16_000).expect("encode WAV");
        assert_eq!(wav.len(), 44);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(&wav[40..44], &0_u32.to_le_bytes());
    }

    #[test]
    fn matches_pcm16_floor_and_clamp_boundaries() {
        let lsb = 1.0_f32 / 32_768.0;
        let input = [
            -1.0,
            1.0,
            -0.5 * lsb,
            0.5 * lsb,
            -1.5 * lsb,
            1.5 * lsb,
            -2.5 * lsb,
            2.5 * lsb,
            -32_767.5 * lsb,
            32_767.5 * lsb,
            -1.25,
            1.25,
        ];
        let wav = audio_to_wav_bytes(&input, 16_000).expect("encode WAV");
        let actual: Vec<i16> = wav[44..]
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
            .collect();
        assert_eq!(
            actual,
            [
                -32_768, 32_767, -1, 0, -2, 1, -3, 2, -32_768, 32_767, -32_768, 32_767
            ]
        );
    }
}
