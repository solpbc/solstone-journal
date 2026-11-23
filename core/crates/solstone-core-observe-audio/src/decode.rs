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

/// Maximum file size scanned when checking whether a container open failure is
/// a header-only ISO-BMFF stub with zero media payload bytes.
const MAX_HEADER_STUB_BYTES: u64 = 64 * 1024;

/// Opens a local media file, restricted to [`INPUT_FORMATS`] and the `file` protocol.
fn open_input(path: &Path) -> Result<ffmpeg::format::context::Input, ffmpeg::Error> {
    let mut options = ffmpeg::Dictionary::new();
    options.set("protocol_whitelist", "file");
    options.set("format_whitelist", INPUT_FORMATS);
    ffmpeg::format::input_with_dictionary(path, options)
}

/// Opens a local audio container, classifying header-only ISO-BMFF stubs that
/// hold zero media payload bytes as [`AudioError::EmptyInput`] rather than
/// [`AudioError::CorruptInput`].
fn open_audio_input(path: &Path) -> Result<ffmpeg::format::context::Input, AudioError> {
    open_input(path).map_err(|error| {
        let mapped = corrupt_input(path, error);
        if !mapped.is_transient_decode() && is_header_only_isobmff_stub(path) {
            AudioError::EmptyInput {
                path: path.to_path_buf(),
            }
        } else {
            mapped
        }
    })
}

fn is_header_only_isobmff_stub(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if metadata.len() < 8 || metadata.len() > MAX_HEADER_STUB_BYTES {
        return false;
    }
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    if bytes.len() as u64 > MAX_HEADER_STUB_BYTES {
        return false;
    }
    is_header_only_isobmff_stub_bytes(&bytes)
}

/// Returns `true` only when `bytes` is a complete, well-formed sequence of
/// top-level ISO-BMFF boxes starting with `ftyp`, with no `moov`/`moof` or
/// unknown boxes and zero `mdat` payload bytes (for example, a 28-byte bare
/// `ftyp` stub left when a recorder prepared a file without writing any audio).
fn is_header_only_isobmff_stub_bytes(bytes: &[u8]) -> bool {
    if bytes.len() < 8 {
        return false;
    }
    let mut offset = 0usize;
    let mut saw_ftyp = false;
    while offset < bytes.len() {
        if bytes.len() - offset < 8 {
            return false;
        }
        let size32 = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]);
        let box_type = [
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ];
        if !box_type.iter().all(|byte| (0x20..=0x7E).contains(byte)) {
            return false;
        }
        let (header_len, box_len) = match size32 {
            0 => (8usize, bytes.len() - offset),
            1 => {
                if bytes.len() - offset < 16 {
                    return false;
                }
                let size64 = u64::from_be_bytes([
                    bytes[offset + 8],
                    bytes[offset + 9],
                    bytes[offset + 10],
                    bytes[offset + 11],
                    bytes[offset + 12],
                    bytes[offset + 13],
                    bytes[offset + 14],
                    bytes[offset + 15],
                ]);
                let Ok(len) = usize::try_from(size64) else {
                    return false;
                };
                if len < 16 {
                    return false;
                }
                (16usize, len)
            }
            2..=7 => return false,
            n => (8usize, n as usize),
        };
        if box_len > bytes.len() - offset {
            return false;
        }
        let payload_len = box_len - header_len;
        match &box_type {
            b"ftyp" => {
                if offset != 0 || payload_len < 8 || !payload_len.is_multiple_of(4) {
                    return false;
                }
                let major = &bytes[offset + header_len..offset + header_len + 4];
                let compat = &bytes[offset + header_len + 8..offset + box_len];
                if !major.iter().all(|byte| (0x20..=0x7E).contains(byte))
                    || !compat.iter().all(|byte| (0x20..=0x7E).contains(byte))
                {
                    return false;
                }
                saw_ftyp = true;
            }
            b"free" | b"skip" | b"wide" => {
                if !saw_ftyp {
                    return false;
                }
            }
            b"mdat" => {
                if !saw_ftyp || payload_len != 0 {
                    return false;
                }
            }
            _ => return false,
        }
        offset += box_len;
    }
    saw_ftyp
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
    let mut input = open_audio_input(path)?;
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
        if decoder.channels() == 0 || decoder.rate() == 0 {
            Err(AudioError::CorruptInput {
                path: path.to_path_buf(),
                detail: "audio stream has invalid codec parameters".to_owned(),
                errno: None,
            })
        } else {
            Err(AudioError::NoDecodedAudio {
                path: path.to_path_buf(),
            })
        }
    } else {
        Ok(audio)
    }
}

fn decode_m4a(path: &Path, max_samples: usize) -> Result<Vec<f32>, AudioError> {
    let input = open_audio_input(path)?;
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
    let mut any_valid_params = false;
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
        if decoder.channels() > 0 && decoder.rate() > 0 {
            any_valid_params = true;
        }
        if !audio.is_empty() {
            remaining_samples -= audio.len();
            streams.push(audio);
        }
    }

    match streams.len() {
        0 if !any_valid_params => Err(AudioError::CorruptInput {
            path: path.to_path_buf(),
            detail: "audio stream has invalid codec parameters".to_owned(),
            errno: None,
        }),
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
    emitted_any: bool,
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
    let input = open_audio_input(path)?;
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
        emitted_any: false,
    })
}

fn open_m4a_streams(path: &Path) -> Result<Vec<M4aStream>, AudioError> {
    let input = open_audio_input(path)?;
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
    match take_pending(&mut decoder.cursor, count)? {
        Some(samples) => {
            decoder.emitted_any = true;
            Ok(Some(samples))
        }
        None => {
            if !decoder.emitted_any
                && (decoder.decoder.channels() == 0 || decoder.decoder.rate() == 0)
            {
                Err(AudioError::CorruptInput {
                    path: path.to_path_buf(),
                    detail: "audio stream has invalid codec parameters".to_owned(),
                    errno: None,
                })
            } else {
                Ok(None)
            }
        }
    }
}

fn pull_m4a(
    streams: &mut Vec<M4aStream>,
    probed: &mut bool,
    path: &Path,
    count: usize,
) -> Result<Option<Vec<f32>>, AudioError> {
    if !*probed {
        let mut keep = Vec::new();
        let mut any_valid_params = false;
        for mut stream in streams.drain(..) {
            fill_m4a(&mut stream, path, 1)?;
            if stream.decoder.channels() > 0 && stream.decoder.rate() > 0 {
                any_valid_params = true;
            }
            if !stream.cursor.pending.is_empty() {
                keep.push(stream);
            }
        }
        *streams = keep;
        *probed = true;
        if streams.is_empty() && !any_valid_params {
            return Err(AudioError::CorruptInput {
                path: path.to_path_buf(),
                detail: "audio stream has invalid codec parameters".to_owned(),
                errno: None,
            });
        }
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

    #[test]
    fn header_only_isobmff_stub_detection_separates_stubs_from_corrupt_media() {
        // Exact 28-byte M4A ftyp header written before any samples exist.
        const WATCH_STUB: &[u8] = &[
            0x00, 0x00, 0x00, 0x1c, b'f', b't', b'y', b'p', b'M', b'4', b'A', b' ', 0x00, 0x00,
            0x00, 0x00, b'M', b'4', b'A', b' ', b'm', b'p', b'4', b'2', b'i', b's', b'o', b'm',
        ];
        assert!(super::is_header_only_isobmff_stub_bytes(WATCH_STUB));

        // ftyp + empty mdat (8 bytes) + free box (12 bytes) is still header-only.
        let mut with_empty_mdat = WATCH_STUB.to_vec();
        with_empty_mdat.extend_from_slice(&[0, 0, 0, 8, b'm', b'd', b'a', b't']);
        with_empty_mdat.extend_from_slice(&[0, 0, 0, 12, b'f', b'r', b'e', b'e', 0, 0, 0, 0]);
        assert!(super::is_header_only_isobmff_stub_bytes(&with_empty_mdat));

        // ftyp + size-0 mdat at EOF with zero payload bytes is header-only.
        let mut with_eof_empty_mdat = WATCH_STUB.to_vec();
        with_eof_empty_mdat.extend_from_slice(&[0, 0, 0, 0, b'm', b'd', b'a', b't']);
        assert!(super::is_header_only_isobmff_stub_bytes(
            &with_eof_empty_mdat
        ));

        // ftyp + extended 64-bit empty mdat (16 bytes) is header-only.
        let mut with_64bit_empty_mdat = WATCH_STUB.to_vec();
        with_64bit_empty_mdat
            .extend_from_slice(&[0, 0, 0, 1, b'm', b'd', b'a', b't', 0, 0, 0, 0, 0, 0, 0, 16]);
        assert!(super::is_header_only_isobmff_stub_bytes(
            &with_64bit_empty_mdat
        ));

        // Truncated recording with media payload in mdat must stay corrupt.
        let mut with_media_payload = WATCH_STUB.to_vec();
        with_media_payload.extend_from_slice(&[0, 0, 0, 12, b'm', b'd', b'a', b't', 1, 2, 3, 4]);
        assert!(!super::is_header_only_isobmff_stub_bytes(
            &with_media_payload
        ));

        // Open-ended size-0 mdat containing media bytes must stay corrupt.
        let mut with_eof_media = WATCH_STUB.to_vec();
        with_eof_media.extend_from_slice(&[0, 0, 0, 0, b'm', b'd', b'a', b't', 0xff]);
        assert!(!super::is_header_only_isobmff_stub_bytes(&with_eof_media));

        // A file carrying a moov box that FFmpeg failed to open is damaged, not a stub.
        let mut with_moov = WATCH_STUB.to_vec();
        with_moov.extend_from_slice(&[0, 0, 0, 8, b'm', b'o', b'o', b'v']);
        assert!(!super::is_header_only_isobmff_stub_bytes(&with_moov));

        // Truncated ftyp, non-ASCII compatible brands, and arbitrary bytes stay corrupt.
        assert!(!super::is_header_only_isobmff_stub_bytes(&WATCH_STUB[..20]));
        assert!(!super::is_header_only_isobmff_stub_bytes(&[
            0, 0, 0, 0, b'f', b't', b'y', b'p', b'M', b'4', b'A', b' ', 0, 0, 0, 0, 0xff, 0xfe,
            0xfd, 0xfc,
        ]));
        assert!(!super::is_header_only_isobmff_stub_bytes(b"not media"));
        assert!(!super::is_header_only_isobmff_stub_bytes(&[
            0, 0, 0, 8, b'f', b'r', b'e', b'e'
        ]));
    }
}
