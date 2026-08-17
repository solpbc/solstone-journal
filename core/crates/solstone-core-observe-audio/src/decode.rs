// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use ffmpeg_next as ffmpeg;

use crate::{AudioError, SAMPLE_RATE};

const OUTPUT_FORMAT: ffmpeg::format::Sample =
    ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed);
const FLUSH_OUTPUT_PADDING: i64 = 3;
const MAX_FLUSH_ITERATIONS: usize = 16;

/// Decodes audio into packed mono f32 at [`SAMPLE_RATE`].
///
/// The `.m4a` suffix selects Python-compatible all-stream mixing. Every other
/// suffix decodes only the first audio stream and flushes its resampler.
pub fn decode_f32_mono(path: &Path) -> Result<Vec<f32>, AudioError> {
    let metadata = fs::metadata(path).map_err(|source| AudioError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() == 0 {
        return Err(AudioError::EmptyInput {
            path: path.to_path_buf(),
        });
    }
    ffmpeg::init().map_err(|error| ffmpeg_error(path, error))?;

    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("m4a"))
    {
        decode_m4a(path)
    } else {
        decode_non_m4a(path)
    }
}

fn decode_non_m4a(path: &Path) -> Result<Vec<f32>, AudioError> {
    let mut input = ffmpeg::format::input(path).map_err(|error| corrupt_input(path, error))?;
    let stream_index = first_audio_stream_index(&input, path)?;
    let parameters = input
        .stream(stream_index)
        .expect("stream index came from this input")
        .parameters();
    let context = ffmpeg::codec::context::Context::from_parameters(parameters)
        .map_err(|error| corrupt_input(path, error))?;
    let mut decoder = context
        .decoder()
        .audio()
        .map_err(|error| corrupt_input(path, error))?;
    let mut resampler = None;
    let mut audio = Vec::new();

    loop {
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => {
                if packet.stream() == stream_index {
                    decoder
                        .send_packet(&packet)
                        .map_err(|error| corrupt_input(path, error))?;
                    drain_non_m4a_decoder(&mut decoder, &mut resampler, &mut audio, path)?;
                }
            }
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(corrupt_input(path, error)),
        }
    }
    decoder
        .send_eof()
        .map_err(|error| corrupt_input(path, error))?;
    drain_non_m4a_decoder(&mut decoder, &mut resampler, &mut audio, path)?;
    if let Some(resampler) = resampler.as_mut() {
        flush_non_m4a_resampler(resampler, &mut audio, path)?;
    }

    if audio.is_empty() {
        Err(AudioError::NoDecodedAudio {
            path: path.to_path_buf(),
        })
    } else {
        Ok(audio)
    }
}

fn decode_m4a(path: &Path) -> Result<Vec<f32>, AudioError> {
    let input = ffmpeg::format::input(path).map_err(|error| corrupt_input(path, error))?;
    let stream_indices: Vec<usize> = input
        .streams()
        .filter(|stream| stream.parameters().medium() == ffmpeg::media::Type::Audio)
        .map(|stream| stream.index())
        .collect();
    drop(input);
    if stream_indices.is_empty() {
        return Err(AudioError::NoAudioStream {
            path: path.to_path_buf(),
        });
    }

    let mut streams = Vec::new();
    for stream_index in stream_indices {
        // Python reopens the M4A container for every audio stream. It also
        // deliberately does not flush this per-stream resampler.
        let mut input = ffmpeg::format::input(path).map_err(|error| corrupt_input(path, error))?;
        let parameters = input
            .stream(stream_index)
            .expect("stream index came from this input")
            .parameters();
        let context = ffmpeg::codec::context::Context::from_parameters(parameters)
            .map_err(|error| corrupt_input(path, error))?;
        let mut decoder = context
            .decoder()
            .audio()
            .map_err(|error| corrupt_input(path, error))?;
        let mut resampler = None;
        let mut audio = Vec::new();

        loop {
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut input) {
                Ok(()) => {
                    if packet.stream() == stream_index {
                        let packet = m4a_payload_packet(&packet);
                        decoder
                            .send_packet(&packet)
                            .map_err(|error| corrupt_input(path, error))?;
                        drain_m4a_decoder(&mut decoder, &mut resampler, &mut audio, path)?;
                    }
                }
                Err(ffmpeg::Error::Eof) => break,
                Err(error) => return Err(corrupt_input(path, error)),
            }
        }
        decoder
            .send_eof()
            .map_err(|error| corrupt_input(path, error))?;
        drain_m4a_decoder(&mut decoder, &mut resampler, &mut audio, path)?;
        if !audio.is_empty() {
            streams.push(audio);
        }
    }

    match streams.len() {
        0 => Err(AudioError::NoDecodedAudio {
            path: path.to_path_buf(),
        }),
        1 => Ok(streams.pop().expect("one stream exists")),
        _ => Ok(mix_m4a_streams(streams)),
    }
}

fn first_audio_stream_index(
    input: &ffmpeg::format::context::Input,
    path: &Path,
) -> Result<usize, AudioError> {
    input
        .streams()
        .find(|stream| stream.parameters().medium() == ffmpeg::media::Type::Audio)
        .map(|stream| stream.index())
        .ok_or_else(|| AudioError::NoAudioStream {
            path: path.to_path_buf(),
        })
}

fn make_resampler(
    frame: &ffmpeg::frame::Audio,
    path: &Path,
) -> Result<ffmpeg::software::resampling::Context, AudioError> {
    ffmpeg::software::resampling::Context::get(
        frame.format(),
        frame.channel_layout(),
        frame.rate(),
        OUTPUT_FORMAT,
        ffmpeg::ChannelLayout::MONO,
        SAMPLE_RATE,
    )
    .map_err(|error| ffmpeg_error(path, error))
}

fn drain_non_m4a_decoder(
    decoder: &mut ffmpeg::decoder::Audio,
    resampler: &mut Option<ffmpeg::software::resampling::Context>,
    audio: &mut Vec<f32>,
    path: &Path,
) -> Result<(), AudioError> {
    loop {
        let mut decoded = ffmpeg::frame::Audio::empty();
        match decoder.receive_frame(&mut decoded) {
            Ok(()) => {
                normalize_channel_layout(&mut decoded);
                if resampler.is_none() {
                    *resampler = Some(make_resampler(&decoded, path)?);
                }
                let mut converted = ffmpeg::frame::Audio::empty();
                resampler
                    .as_mut()
                    .expect("resampler initialized from decoded frame")
                    .run(&decoded, &mut converted)
                    .map_err(|error| ffmpeg_error(path, error))?;
                audio.extend_from_slice(converted.plane::<f32>(0));
            }
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(corrupt_input(path, error)),
        }
    }
    Ok(())
}

fn drain_m4a_decoder(
    decoder: &mut ffmpeg::decoder::Audio,
    resampler: &mut Option<ffmpeg::software::resampling::Context>,
    audio: &mut Vec<f32>,
    path: &Path,
) -> Result<(), AudioError> {
    loop {
        let mut decoded = ffmpeg::frame::Audio::empty();
        match decoder.receive_frame(&mut decoded) {
            Ok(()) => {
                normalize_channel_layout(&mut decoded);
                if resampler.is_none() {
                    *resampler = Some(make_resampler(&decoded, path)?);
                }
                let mut converted = ffmpeg::frame::Audio::empty();
                resampler
                    .as_mut()
                    .expect("resampler initialized from decoded frame")
                    .run(&decoded, &mut converted)
                    .map_err(|error| ffmpeg_error(path, error))?;
                audio.extend_from_slice(converted.plane::<f32>(0));
            }
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(corrupt_input(path, error)),
        }
    }
    Ok(())
}

// FFmpeg 9 honors AAC terminal skip/discard padding that PyAV leaves visible,
// so rebuild packets as payload plus timing to retain oracle-visible tails.
// This drops all side data, not only discard padding; scope is the tested corpus.
// Future side-data-bearing M4A must filter only the relevant side-data type.
fn m4a_payload_packet(packet: &ffmpeg::Packet) -> ffmpeg::Packet {
    let mut payload_only = ffmpeg::Packet::copy(packet.data().unwrap_or_default());
    payload_only.set_pts(packet.pts());
    payload_only.set_dts(packet.dts());
    payload_only.set_duration(packet.duration());
    payload_only.set_time_base(packet.time_base());
    payload_only
}

fn normalize_channel_layout(frame: &mut ffmpeg::frame::Audio) {
    if frame.channel_layout().is_empty() {
        frame.set_channel_layout(ffmpeg::ChannelLayout::default(i32::from(frame.channels())));
    }
}

fn flush_non_m4a_resampler(
    resampler: &mut ffmpeg::software::resampling::Context,
    audio: &mut Vec<f32>,
    path: &Path,
) -> Result<(), AudioError> {
    let flush_sample_limit = audio
        .len()
        .saturating_mul(2)
        .max(usize::try_from(SAMPLE_RATE).expect("u32 always fits usize"));
    let mut flushed_samples = 0;
    let mut iterations = 0;

    while let Some(delay) = resampler.delay() {
        if iterations == MAX_FLUSH_ITERATIONS {
            return Err(AudioError::ResamplerFlushDidNotConverge {
                path: path.to_path_buf(),
                iterations,
                flushed_samples,
                remaining_samples: delay.output,
            });
        }
        let capacity =
            usize::try_from(delay.output.saturating_add(FLUSH_OUTPUT_PADDING)).map_err(|_| {
                AudioError::ResamplerFlushDidNotConverge {
                    path: path.to_path_buf(),
                    iterations,
                    flushed_samples,
                    remaining_samples: delay.output,
                }
            })?;
        let mut converted =
            ffmpeg::frame::Audio::new(OUTPUT_FORMAT, capacity, ffmpeg::ChannelLayout::MONO);
        let remaining = resampler
            .flush(&mut converted)
            .map_err(|error| ffmpeg_error(path, error))?;
        let produced = converted.samples();
        let next_flushed_samples = flushed_samples.saturating_add(produced);
        let remaining_samples = remaining.map_or(0, |remaining| remaining.output);
        iterations += 1;
        if next_flushed_samples > flush_sample_limit {
            return Err(AudioError::ResamplerFlushDidNotConverge {
                path: path.to_path_buf(),
                iterations,
                flushed_samples: next_flushed_samples,
                remaining_samples,
            });
        }
        audio.extend_from_slice(converted.plane::<f32>(0));
        flushed_samples = next_flushed_samples;
        // `swr_get_delay` can retain filter delay after every remaining output
        // sample has been drained. A zero-producing flush is the authoritative
        // terminal condition; otherwise FFmpeg 9 reports the same delay forever.
        if remaining.is_none() || produced == 0 {
            break;
        }
    }
    Ok(())
}

fn mix_m4a_streams(streams: Vec<Vec<f32>>) -> Vec<f32> {
    let longest = streams.iter().map(Vec::len).max().expect("streams exist");
    let mut mixed = vec![0.0; longest];
    for stream in &streams {
        for (destination, source) in mixed.iter_mut().zip(stream) {
            *destination += source;
        }
    }
    let count = streams.len() as f32;
    mixed.iter_mut().for_each(|sample| *sample /= count);
    mixed
}

fn corrupt_input(path: &Path, error: ffmpeg::Error) -> AudioError {
    AudioError::CorruptInput {
        path: path.to_path_buf(),
        detail: error.to_string(),
    }
}

fn ffmpeg_error(path: &Path, error: ffmpeg::Error) -> AudioError {
    AudioError::Ffmpeg {
        path: path.to_path_buf(),
        detail: error.to_string(),
    }
}
