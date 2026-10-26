// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use ffmpeg_next as ffmpeg;

use crate::{AudioError, SAMPLE_RATE};

const OUTPUT_FORMAT: ffmpeg::format::Sample =
    ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed);
const FLUSH_OUTPUT_PADDING: i64 = 3;
const MAX_FLUSH_ITERATIONS: usize = 16;

/// Demuxers this call site reads; every other demuxer compiled into FFmpeg is refused.
const INPUT_FORMATS: &str = "flac,matroska,mov,mp3,ogg,wav";

/// Opens a local media file, restricted to [`INPUT_FORMATS`] and the `file` protocol.
fn open_input(path: &Path) -> Result<ffmpeg::format::context::Input, ffmpeg::Error> {
    let mut options = ffmpeg::Dictionary::new();
    options.set("protocol_whitelist", "file");
    options.set("format_whitelist", INPUT_FORMATS);
    ffmpeg::format::input_with_dictionary(path, options)
}

/// Decodes audio into packed mono f32 at [`SAMPLE_RATE`].
///
/// The `.m4a` suffix selects Python-compatible all-stream mixing. Every other
/// suffix decodes only the first audio stream and flushes its resampler.
pub fn decode_f32_mono(path: &Path) -> Result<Vec<f32>, AudioError> {
    decode_f32_mono_bounded(path, usize::MAX)
}

/// Decode with a bound on the total decoded samples, including all mixed M4A
/// tracks. Refuses before appending samples beyond the caller's memory budget.
pub fn decode_f32_mono_bounded(path: &Path, max_samples: usize) -> Result<Vec<f32>, AudioError> {
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
        decode_m4a(path, max_samples)
    } else {
        decode_non_m4a(path, max_samples)
    }
}

fn decode_non_m4a(path: &Path, max_samples: usize) -> Result<Vec<f32>, AudioError> {
    let mut input = open_input(path).map_err(|error| corrupt_input(path, error))?;
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
                    drain_non_m4a_decoder(
                        &mut decoder,
                        &mut resampler,
                        &mut audio,
                        path,
                        max_samples,
                    )?;
                }
            }
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(corrupt_input(path, error)),
        }
    }
    decoder
        .send_eof()
        .map_err(|error| corrupt_input(path, error))?;
    drain_non_m4a_decoder(&mut decoder, &mut resampler, &mut audio, path, max_samples)?;
    if let Some(resampler) = resampler.as_mut() {
        flush_non_m4a_resampler(resampler, &mut audio, path, max_samples)?;
    }

    if audio.is_empty() {
        Err(AudioError::NoDecodedAudio {
            path: path.to_path_buf(),
        })
    } else {
        Ok(audio)
    }
}

fn decode_m4a(path: &Path, max_samples: usize) -> Result<Vec<f32>, AudioError> {
    let input = open_input(path).map_err(|error| corrupt_input(path, error))?;
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
    let mut remaining_samples = max_samples;
    for stream_index in stream_indices {
        // Python reopens the M4A container for every audio stream. It also
        // deliberately does not flush this per-stream resampler.
        let mut input = open_input(path).map_err(|error| corrupt_input(path, error))?;
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
                        drain_m4a_decoder(
                            &mut decoder,
                            &mut resampler,
                            &mut audio,
                            path,
                            remaining_samples,
                        )?;
                    }
                }
                Err(ffmpeg::Error::Eof) => break,
                Err(error) => return Err(corrupt_input(path, error)),
            }
        }
        decoder
            .send_eof()
            .map_err(|error| corrupt_input(path, error))?;
        drain_m4a_decoder(
            &mut decoder,
            &mut resampler,
            &mut audio,
            path,
            remaining_samples,
        )?;
        if !audio.is_empty() {
            remaining_samples -= audio.len();
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
    max_samples: usize,
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
                let samples = converted.plane::<f32>(0);
                if samples.len() > max_samples.saturating_sub(audio.len()) {
                    return Err(AudioError::DecodedSampleLimit {
                        path: path.to_path_buf(),
                        max_samples,
                    });
                }
                audio.extend_from_slice(samples);
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
    max_samples: usize,
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
                if decoded.rate() < SAMPLE_RATE {
                    // An empty output frame is allocated to the input frame's
                    // sample count by ffmpeg-next. Upsampling needs room for
                    // more output, including samples buffered by swresample.
                    let context = resampler
                        .as_ref()
                        .expect("resampler initialized from decoded frame");
                    let pending = context
                        .delay()
                        .map_or(0, |delay| delay.output.max(0) as usize);
                    let capacity = decoded
                        .samples()
                        .saturating_mul(SAMPLE_RATE as usize)
                        .div_ceil(decoded.rate() as usize)
                        .saturating_add(pending)
                        .saturating_add(FLUSH_OUTPUT_PADDING as usize);
                    converted = ffmpeg::frame::Audio::new(
                        OUTPUT_FORMAT,
                        capacity,
                        ffmpeg::ChannelLayout::MONO,
                    );
                }
                resampler
                    .as_mut()
                    .expect("resampler initialized from decoded frame")
                    .run(&decoded, &mut converted)
                    .map_err(|error| ffmpeg_error(path, error))?;
                let samples = converted.plane::<f32>(0);
                if samples.len() > max_samples.saturating_sub(audio.len()) {
                    return Err(AudioError::DecodedSampleLimit {
                        path: path.to_path_buf(),
                        max_samples,
                    });
                }
                audio.extend_from_slice(samples);
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
    max_samples: usize,
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
        let samples = converted.plane::<f32>(0);
        if samples.len() > max_samples.saturating_sub(audio.len()) {
            return Err(AudioError::DecodedSampleLimit {
                path: path.to_path_buf(),
                max_samples,
            });
        }
        audio.extend_from_slice(samples);
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

pub(crate) fn is_transient_ffmpeg_errno(errno: i32) -> bool {
    matches!(
        errno,
        ffmpeg::error::EIO | ffmpeg::error::EINTR | ffmpeg::error::ENOMEM | ffmpeg::error::EBUSY
    )
}

fn extract_ffmpeg_errno(error: &ffmpeg::Error) -> Option<i32> {
    match error {
        ffmpeg::Error::Other { errno } => Some(*errno),
        _ => None,
    }
}

fn corrupt_input(path: &Path, error: ffmpeg::Error) -> AudioError {
    AudioError::CorruptInput {
        path: path.to_path_buf(),
        detail: error.to_string(),
        errno: extract_ffmpeg_errno(&error),
    }
}

fn ffmpeg_error(path: &Path, error: ffmpeg::Error) -> AudioError {
    AudioError::Ffmpeg {
        path: path.to_path_buf(),
        detail: error.to_string(),
        errno: extract_ffmpeg_errno(&error),
    }
}

/// Samples already decoded and not yet returned, plus whether that stream has ended.
struct PcmCursor {
    pending: VecDeque<f32>,
    ended: bool,
}

fn mix_ready(states: &[(usize, bool)]) -> usize {
    let open = states.iter().filter(|(_, ended)| !ended);
    if let Some(shortest) = open.map(|(len, _)| *len).min() {
        shortest
    } else {
        states.iter().map(|(len, _)| *len).max().unwrap_or(0)
    }
}

#[cfg(test)]
fn cursor_states(cursors: &[PcmCursor]) -> Vec<(usize, bool)> {
    cursors
        .iter()
        .map(|cursor| (cursor.pending.len(), cursor.ended))
        .collect()
}

fn mix_emit(cursors: &mut [PcmCursor], count: usize, divisor: f32, out: &mut Vec<f32>) {
    for _ in 0..count {
        let mut sum = 0.0;
        for cursor in cursors.iter_mut() {
            if let Some(sample) = cursor.pending.pop_front() {
                sum += sample;
            }
        }
        out.push(sum / divisor);
    }
}

/// Mix constructed slices with the same sum, stream-count divisor, and longest-length
/// padding as [`mix_m4a_streams`], feeding `feed` new samples per stream at a time.
#[cfg(test)]
fn mix_m4a_incremental(streams: &[Vec<f32>], feed: usize) -> Vec<f32> {
    let mut sources = streams
        .iter()
        .map(|stream| stream.as_slice())
        .collect::<Vec<_>>();
    let mut cursors = streams
        .iter()
        .map(|_| PcmCursor {
            pending: VecDeque::new(),
            ended: false,
        })
        .collect::<Vec<_>>();
    let divisor = streams.len() as f32;
    let mut mixed = Vec::new();
    loop {
        let ready = mix_ready(&cursor_states(&cursors));
        if ready == 0 {
            let mut progressed = false;
            for (cursor, source) in cursors.iter_mut().zip(sources.iter_mut()) {
                if cursor.ended {
                    continue;
                }
                let take = feed.max(1).min(source.len());
                cursor.pending.extend(source.iter().take(take).copied());
                *source = &source[take..];
                if source.is_empty() {
                    cursor.ended = true;
                }
                progressed = true;
            }
            if !progressed || mix_ready(&cursor_states(&cursors)) == 0 {
                break;
            }
            continue;
        }
        mix_emit(&mut cursors, ready, divisor, &mut mixed);
    }
    mixed
}

struct NonM4aDecoder {
    input: ffmpeg::format::context::Input,
    stream_index: usize,
    decoder: ffmpeg::decoder::Audio,
    resampler: Option<ffmpeg::software::resampling::Context>,
    cursor: PcmCursor,
    packets_done: bool,
}

struct M4aStream {
    input: ffmpeg::format::context::Input,
    stream_index: usize,
    decoder: ffmpeg::decoder::Audio,
    resampler: Option<ffmpeg::software::resampling::Context>,
    cursor: PcmCursor,
}

enum DecoderKind {
    NonM4a(NonM4aDecoder),
    /// `None` until the first pull has dropped streams that produced no samples.
    M4a {
        streams: Vec<M4aStream>,
        probed: bool,
    },
}

/// Pull decoded mono f32 at [`SAMPLE_RATE`] in bounded pieces.
///
/// Concatenating every [`UnitDecoder::pull`] equals [`decode_f32_mono`]. Desktop
/// decode does not use this type. M4A mixes with [`mix_m4a_streams`]'s formula
/// without retaining every stream's PCM. Opus stays on the ogg and matroska demuxers.
pub struct UnitDecoder {
    path: PathBuf,
    kind: DecoderKind,
}

impl UnitDecoder {
    /// Open one local file. An empty file is [`AudioError::EmptyInput`].
    pub fn open(path: &Path) -> Result<Self, AudioError> {
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
        let kind = if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("m4a"))
        {
            DecoderKind::M4a {
                streams: open_m4a_streams(path)?,
                probed: false,
            }
        } else {
            DecoderKind::NonM4a(open_non_m4a(path)?)
        };
        Ok(Self {
            path: path.to_path_buf(),
            kind,
        })
    }

    /// At most `count` new samples. `None` once flushed output is exhausted.
    /// A shorter `Some` is the final piece. `count == 0` yields an empty piece.
    pub fn pull(&mut self, count: usize) -> Result<Option<Vec<f32>>, AudioError> {
        if count == 0 {
            return Ok(Some(Vec::new()));
        }
        match &mut self.kind {
            DecoderKind::NonM4a(decoder) => pull_non_m4a(decoder, &self.path, count),
            DecoderKind::M4a { streams, probed } => pull_m4a(streams, probed, &self.path, count),
        }
    }
}

fn open_non_m4a(path: &Path) -> Result<NonM4aDecoder, AudioError> {
    let input = open_input(path).map_err(|error| corrupt_input(path, error))?;
    let stream_index = first_audio_stream_index(&input, path)?;
    let parameters = input
        .stream(stream_index)
        .expect("stream index came from this input")
        .parameters();
    let context = ffmpeg::codec::context::Context::from_parameters(parameters)
        .map_err(|error| corrupt_input(path, error))?;
    let decoder = context
        .decoder()
        .audio()
        .map_err(|error| corrupt_input(path, error))?;
    Ok(NonM4aDecoder {
        input,
        stream_index,
        decoder,
        resampler: None,
        cursor: PcmCursor {
            pending: VecDeque::new(),
            ended: false,
        },
        packets_done: false,
    })
}

fn open_m4a_streams(path: &Path) -> Result<Vec<M4aStream>, AudioError> {
    let input = open_input(path).map_err(|error| corrupt_input(path, error))?;
    let stream_indices = input
        .streams()
        .filter(|stream| stream.parameters().medium() == ffmpeg::media::Type::Audio)
        .map(|stream| stream.index())
        .collect::<Vec<_>>();
    drop(input);
    if stream_indices.is_empty() {
        return Err(AudioError::NoAudioStream {
            path: path.to_path_buf(),
        });
    }
    let mut streams = Vec::with_capacity(stream_indices.len());
    for stream_index in stream_indices {
        let input = open_input(path).map_err(|error| corrupt_input(path, error))?;
        let parameters = input
            .stream(stream_index)
            .expect("stream index came from this input")
            .parameters();
        let context = ffmpeg::codec::context::Context::from_parameters(parameters)
            .map_err(|error| corrupt_input(path, error))?;
        let decoder = context
            .decoder()
            .audio()
            .map_err(|error| corrupt_input(path, error))?;
        streams.push(M4aStream {
            input,
            stream_index,
            decoder,
            resampler: None,
            cursor: PcmCursor {
                pending: VecDeque::new(),
                ended: false,
            },
        });
    }
    Ok(streams)
}

fn pull_non_m4a(
    decoder: &mut NonM4aDecoder,
    path: &Path,
    count: usize,
) -> Result<Option<Vec<f32>>, AudioError> {
    while decoder.cursor.pending.len() < count && !decoder.cursor.ended {
        if decoder.packets_done {
            break;
        }
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut decoder.input) {
            Ok(()) => {
                if packet.stream() == decoder.stream_index {
                    decoder
                        .decoder
                        .send_packet(&packet)
                        .map_err(|error| corrupt_input(path, error))?;
                    let mut chunk = Vec::new();
                    drain_non_m4a_decoder(
                        &mut decoder.decoder,
                        &mut decoder.resampler,
                        &mut chunk,
                        path,
                        usize::MAX,
                    )?;
                    decoder.cursor.pending.extend(chunk);
                }
            }
            Err(ffmpeg::Error::Eof) => {
                decoder.packets_done = true;
                decoder
                    .decoder
                    .send_eof()
                    .map_err(|error| corrupt_input(path, error))?;
                let mut chunk = Vec::new();
                drain_non_m4a_decoder(
                    &mut decoder.decoder,
                    &mut decoder.resampler,
                    &mut chunk,
                    path,
                    usize::MAX,
                )?;
                decoder.cursor.pending.extend(chunk);
                if let Some(resampler) = decoder.resampler.as_mut() {
                    let mut flushed = Vec::new();
                    flush_non_m4a_resampler(resampler, &mut flushed, path, usize::MAX)?;
                    decoder.cursor.pending.extend(flushed);
                }
                decoder.cursor.ended = true;
            }
            Err(error) => return Err(corrupt_input(path, error)),
        }
    }
    take_pending(&mut decoder.cursor, count)
}

fn pull_m4a(
    streams: &mut Vec<M4aStream>,
    probed: &mut bool,
    path: &Path,
    count: usize,
) -> Result<Option<Vec<f32>>, AudioError> {
    if !*probed {
        let mut keep = Vec::new();
        for mut stream in streams.drain(..) {
            fill_m4a(&mut stream, path, 1)?;
            if !stream.cursor.pending.is_empty() {
                keep.push(stream);
            }
        }
        *streams = keep;
        *probed = true;
    }
    if streams.is_empty() {
        return Ok(None);
    }
    if streams.len() == 1 {
        fill_m4a(&mut streams[0], path, count)?;
        return take_pending(&mut streams[0].cursor, count);
    }
    let divisor = streams.len() as f32;
    let mut mixed = Vec::new();
    while mixed.len() < count {
        let ready = mix_ready(&m4a_states(streams));
        if ready == 0 {
            let mut open = false;
            for stream in streams.iter_mut() {
                if stream.cursor.ended {
                    continue;
                }
                open = true;
                let target = stream.cursor.pending.len().saturating_add(1);
                fill_m4a(stream, path, target)?;
            }
            if !open || mix_ready(&m4a_states(streams)) == 0 {
                break;
            }
            continue;
        }
        let emit = ready.min(count - mixed.len());
        let mut view = streams
            .iter_mut()
            .map(|stream| PcmCursor {
                pending: std::mem::take(&mut stream.cursor.pending),
                ended: stream.cursor.ended,
            })
            .collect::<Vec<_>>();
        mix_emit(&mut view, emit, divisor, &mut mixed);
        for (stream, cursor) in streams.iter_mut().zip(view) {
            stream.cursor.pending = cursor.pending;
        }
    }
    if mixed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(mixed))
    }
}

fn m4a_states(streams: &[M4aStream]) -> Vec<(usize, bool)> {
    streams
        .iter()
        .map(|stream| (stream.cursor.pending.len(), stream.cursor.ended))
        .collect()
}

fn fill_m4a(stream: &mut M4aStream, path: &Path, min_pending: usize) -> Result<(), AudioError> {
    while stream.cursor.pending.len() < min_pending && !stream.cursor.ended {
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut stream.input) {
            Ok(()) => {
                if packet.stream() == stream.stream_index {
                    let packet = m4a_payload_packet(&packet);
                    stream
                        .decoder
                        .send_packet(&packet)
                        .map_err(|error| corrupt_input(path, error))?;
                    let mut chunk = Vec::new();
                    drain_m4a_decoder(
                        &mut stream.decoder,
                        &mut stream.resampler,
                        &mut chunk,
                        path,
                        usize::MAX,
                    )?;
                    stream.cursor.pending.extend(chunk);
                }
            }
            Err(ffmpeg::Error::Eof) => {
                stream
                    .decoder
                    .send_eof()
                    .map_err(|error| corrupt_input(path, error))?;
                let mut chunk = Vec::new();
                drain_m4a_decoder(
                    &mut stream.decoder,
                    &mut stream.resampler,
                    &mut chunk,
                    path,
                    usize::MAX,
                )?;
                stream.cursor.pending.extend(chunk);
                stream.cursor.ended = true;
            }
            Err(error) => return Err(corrupt_input(path, error)),
        }
    }
    Ok(())
}

fn take_pending(cursor: &mut PcmCursor, count: usize) -> Result<Option<Vec<f32>>, AudioError> {
    if cursor.pending.is_empty() {
        return Ok(None);
    }
    let take = count.min(cursor.pending.len());
    Ok(Some(cursor.pending.drain(..take).collect()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use ffmpeg_next as ffmpeg;

    use super::{corrupt_input, ffmpeg_error};
    use crate::AudioError;

    #[test]
    fn ac1_allowlisted_errno_is_transient_and_others_are_permanent() {
        let dummy = Path::new("test.wav");
        let allowlisted = [
            ffmpeg::error::EIO,
            ffmpeg::error::EINTR,
            ffmpeg::error::ENOMEM,
            ffmpeg::error::EBUSY,
        ];
        for errno in allowlisted {
            let error = AudioError::CorruptInput {
                path: dummy.to_path_buf(),
                detail: "error".to_owned(),
                errno: Some(errno),
            };
            assert!(error.is_transient_decode());
            assert_eq!(error.ffmpeg_errno(), Some(errno));
        }

        let non_allowlisted_errnos = [
            ffmpeg::error::EINVAL,
            ffmpeg::error::ENOENT,
            ffmpeg::error::EPERM,
            ffmpeg::error::EAGAIN,
        ];
        for errno in non_allowlisted_errnos {
            let error = AudioError::CorruptInput {
                path: dummy.to_path_buf(),
                detail: "error".to_owned(),
                errno: Some(errno),
            };
            assert!(!error.is_transient_decode());
            assert_eq!(error.ffmpeg_errno(), Some(errno));
        }

        let non_other = AudioError::CorruptInput {
            path: dummy.to_path_buf(),
            detail: "error".to_owned(),
            errno: None,
        };
        assert!(!non_other.is_transient_decode());
        assert_eq!(non_other.ffmpeg_errno(), None);

        let other_variant = AudioError::EmptyInput {
            path: dummy.to_path_buf(),
        };
        assert!(!other_variant.is_transient_decode());
        assert_eq!(other_variant.ffmpeg_errno(), None);
    }

    #[test]
    fn ac2_detail_string_does_not_affect_transient_classification() {
        let dummy = Path::new("test.wav");
        let error = AudioError::CorruptInput {
            path: dummy.to_path_buf(),
            detail: "codec not found: permanent fatal error".to_owned(),
            errno: Some(ffmpeg::error::EIO),
        };
        assert!(error.is_transient_decode());

        let error_no_errno = AudioError::CorruptInput {
            path: dummy.to_path_buf(),
            detail: "Input/output error: transient I/O failure".to_owned(),
            errno: None,
        };
        assert!(!error_no_errno.is_transient_decode());
    }

    #[test]
    fn ac3_ffmpeg_error_and_corrupt_input_constructors_classify_typed_ffmpeg_errors() {
        let dummy = Path::new("test.wav");
        let transient_ff = ffmpeg::Error::Other {
            errno: ffmpeg::error::EIO,
        };
        let corrupt = corrupt_input(dummy, transient_ff);
        assert!(corrupt.is_transient_decode());
        assert_eq!(corrupt.ffmpeg_errno(), Some(ffmpeg::error::EIO));

        let ff_err = ffmpeg_error(dummy, transient_ff);
        assert!(ff_err.is_transient_decode());
        assert_eq!(ff_err.ffmpeg_errno(), Some(ffmpeg::error::EIO));

        let permanent_ff = ffmpeg::Error::InvalidData;
        let corrupt_perm = corrupt_input(dummy, permanent_ff);
        assert!(!corrupt_perm.is_transient_decode());
        assert_eq!(corrupt_perm.ffmpeg_errno(), None);

        let ff_err_perm = ffmpeg_error(dummy, permanent_ff);
        assert!(!ff_err_perm.is_transient_decode());
        assert_eq!(ff_err_perm.ffmpeg_errno(), None);
    }

    #[test]
    fn incremental_m4a_mix_matches_whole_stream_mix() {
        let streams = vec![
            vec![1.0, 2.0, 3.0, 4.0],
            vec![1.0, 1.0],
            vec![0.5, 0.5, 0.5, 0.5, 0.5],
        ];
        let expected = super::mix_m4a_streams(streams.clone());
        assert_eq!(super::mix_m4a_incremental(&streams, 1), expected);
        assert_eq!(super::mix_m4a_incremental(&streams, 3), expected);
        assert_eq!(
            super::mix_m4a_incremental(&streams, streams.iter().map(Vec::len).max().unwrap()),
            expected
        );
    }
}
