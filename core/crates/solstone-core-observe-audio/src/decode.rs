// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
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

/// Minimum requested clip length accepted by [`decode_clip_wav_from_file`] (1 second).
pub const MIN_CLIP_DURATION_MS: u64 = 1_000;
/// Maximum clip length returned by [`decode_clip_wav_from_file`] (60 seconds).
pub const MAX_CLIP_DURATION_MS: u64 = 60_000;

const SAMPLES_PER_MS: u64 = {
    assert!(SAMPLE_RATE.is_multiple_of(1_000));
    (SAMPLE_RATE / 1_000) as u64
};
const MAX_CLIP_AUDIO_STREAMS: usize = 8;
const MAX_LINEAR_SCAN_SAMPLES: u64 = (SAMPLE_RATE as u64) * 60 * 15;
const SEEK_THRESHOLD_MS: u64 = 10_000;
const SEEK_PREROLL_MS: u64 = 2_000;
const MAX_SEEK_PREROLL_SAMPLES: u64 = (SAMPLE_RATE as u64) * 60;
const INTERLEAVE_EOF_MARGIN_MS: u64 = 5_000;

/// One bounded mono 16 kHz PCM-16 WAV clip sliced from an admitted audio file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedAudioClip {
    /// Canonical 44-byte RIFF/WAVE header + mono 16 kHz PCM-16 samples (zero container metadata).
    pub wav: Vec<u8>,
    /// Start offset in milliseconds from the beginning of the file.
    pub start_ms: u64,
    /// Actual clip duration in milliseconds.
    pub duration_ms: u64,
    /// Total recording duration in milliseconds when known from container metadata or EOF.
    pub total_duration_ms: Option<u64>,
    /// True when audio continues past `start_ms + duration_ms`.
    pub has_more: bool,
    /// Output sample rate in hertz (always [`SAMPLE_RATE`]).
    pub sample_rate_hz: u32,
    /// Output channel count (always `1`).
    pub channels: u16,
}

enum ClipPassOutcome {
    Done(Option<DecodedAudioClip>),
    FallbackLinear,
}

struct ClipStream {
    stream_index: usize,
    time_base: ffmpeg::Rational,
    start_time: i64,
    decoder: ffmpeg::decoder::Audio,
    resampler: Option<ffmpeg::software::resampling::Context>,
    sample_pos: Option<u64>,
    clip: Vec<f32>,
    scanned_samples: u64,
    saw_beyond: bool,
    saw_any_samples: bool,
}

/// Decodes a bounded mono 16 kHz PCM-16 WAV clip from an already-opened audio file.
///
/// Container metadata is stripped by construction: audio is decoded to mono
/// 16 kHz PCM and re-wrapped in a canonical 44-byte RIFF/WAVE header.
///
/// When `max_end_ms` is `Some(cap)`, decoding never returns audio past `cap`
/// milliseconds from the start of `file`.
///
/// Returns `Ok(None)` when `file` holds valid audio that ends at or before `start_ms`
/// (or when `start_ms >= max_end_ms`).
pub fn decode_clip_wav_from_file(
    mut file: fs::File,
    start_ms: u64,
    duration_ms: u64,
    max_end_ms: Option<u64>,
) -> Result<Option<DecodedAudioClip>, AudioError> {
    let path = PathBuf::from("<stream>");
    let metadata = file.metadata().map_err(|source| AudioError::Io {
        path: path.clone(),
        source,
    })?;
    let file_len = metadata.len();
    if file_len == 0 {
        return Err(AudioError::EmptyInput { path });
    }
    if max_end_ms.is_some_and(|cap| start_ms >= cap) {
        return Ok(None);
    }

    let clamped_duration = duration_ms.clamp(MIN_CLIP_DURATION_MS, MAX_CLIP_DURATION_MS);
    let effective_duration_ms = match max_end_ms {
        Some(cap) => clamped_duration.min(cap.saturating_sub(start_ms)),
        None => clamped_duration,
    };
    if effective_duration_ms == 0 {
        return Ok(None);
    }

    let stub_candidate = if file_len <= MAX_HEADER_STUB_BYTES {
        let mut bytes = Vec::with_capacity(file_len as usize);
        file.read_to_end(&mut bytes)
            .map_err(|source| AudioError::Io {
                path: path.clone(),
                source,
            })?;
        file.seek(SeekFrom::Start(0))
            .map_err(|source| AudioError::Io {
                path: path.clone(),
                source,
            })?;
        Some(bytes)
    } else {
        None
    };

    ffmpeg::init().map_err(|error| ffmpeg_error(&path, error))?;

    if start_ms > SEEK_THRESHOLD_MS
        && let Ok(mut backup) = file.try_clone()
    {
        match decode_clip_pass(
            file,
            start_ms,
            effective_duration_ms,
            max_end_ms,
            true,
            stub_candidate.as_deref(),
            &path,
        ) {
            Ok(ClipPassOutcome::Done(result)) => return Ok(result),
            Ok(ClipPassOutcome::FallbackLinear) | Err(AudioError::CorruptInput { .. }) => {
                backup
                    .seek(SeekFrom::Start(0))
                    .map_err(|source| AudioError::Io {
                        path: path.clone(),
                        source,
                    })?;
                file = backup;
            }
            Err(error) => return Err(error),
        }
    }

    match decode_clip_pass(
        file,
        start_ms,
        effective_duration_ms,
        max_end_ms,
        false,
        stub_candidate.as_deref(),
        &path,
    )? {
        ClipPassOutcome::Done(result) => Ok(result),
        ClipPassOutcome::FallbackLinear => Ok(None),
    }
}

fn decode_clip_pass(
    file: fs::File,
    start_ms: u64,
    effective_duration_ms: u64,
    max_end_ms: Option<u64>,
    use_seek: bool,
    stub_candidate: Option<&[u8]>,
    path: &Path,
) -> Result<ClipPassOutcome, AudioError> {
    let io = ffmpeg::format::context::StreamIo::from_read_seek(file)
        .map_err(|error| corrupt_input(path, error))?;
    let mut options = ffmpeg::Dictionary::new();
    options.set("format_whitelist", INPUT_FORMATS);
    options.set("protocol_whitelist", "");
    let mut input =
        ffmpeg::format::input_from_stream(io, None, Some(options)).map_err(|error| {
            let mapped = corrupt_input(path, error);
            if !mapped.is_transient_decode()
                && stub_candidate.is_some_and(is_header_only_isobmff_stub_bytes)
            {
                AudioError::EmptyInput {
                    path: path.to_path_buf(),
                }
            } else {
                mapped
            }
        })?;

    let container_duration_ms = audio_container_duration_ms(&input);

    let mut streams = Vec::new();
    for stream in input
        .streams()
        .filter(|stream| stream.parameters().medium() == ffmpeg::media::Type::Audio)
        .take(MAX_CLIP_AUDIO_STREAMS)
    {
        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|error| corrupt_input(path, error))?;
        let decoder = context
            .decoder()
            .audio()
            .map_err(|error| corrupt_input(path, error))?;
        let start_time = stream.start_time().max(0);
        streams.push(ClipStream {
            stream_index: stream.index(),
            time_base: stream.time_base(),
            start_time,
            decoder,
            resampler: None,
            sample_pos: (!use_seek).then_some(0),
            clip: Vec::new(),
            scanned_samples: 0,
            saw_beyond: false,
            saw_any_samples: false,
        });
    }
    if streams.is_empty() {
        return Err(AudioError::NoAudioStream {
            path: path.to_path_buf(),
        });
    }

    if use_seek {
        let pre_roll_ms = start_ms.saturating_sub(SEEK_PREROLL_MS);
        let pre_roll_us = i64::try_from(pre_roll_ms)
            .unwrap_or(i64::MAX / 1_000)
            .saturating_mul(1_000);
        if input.seek(pre_roll_us, ..pre_roll_us).is_err() {
            return Ok(ClipPassOutcome::FallbackLinear);
        }
    }

    let start_samples = start_ms.saturating_mul(SAMPLES_PER_MS);
    let want_samples =
        usize::try_from(effective_duration_ms.saturating_mul(SAMPLES_PER_MS)).unwrap_or(usize::MAX);
    let end_samples = start_samples.saturating_add(want_samples as u64);
    let end_ms = start_ms.saturating_add(effective_duration_ms);
    let cap_samples = max_end_ms.map(|cap| cap.saturating_mul(SAMPLES_PER_MS));
    let stop_sample = if cap_samples.is_some_and(|cap| end_samples >= cap) {
        end_samples
    } else {
        end_samples.saturating_add(1)
    };
    let max_scan_budget = MAX_LINEAR_SCAN_SAMPLES.saturating_add(want_samples as u64);

    let mut hit_eof = false;
    loop {
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => {
                let Some(stream) = streams
                    .iter_mut()
                    .find(|stream| stream.stream_index == packet.stream())
                else {
                    continue;
                };
                if stream.sample_pos.is_some_and(|pos| pos >= stop_sample) {
                    if let Some(pts) = packet.pts() {
                        use ffmpeg::Rescale;
                        let packet_ms = pts
                            .saturating_sub(stream.start_time)
                            .max(0)
                            .rescale(stream.time_base, (1, 1_000))
                            .max(0) as u64;
                        if packet_ms >= end_ms.saturating_add(INTERLEAVE_EOF_MARGIN_MS) {
                            break;
                        }
                    }
                    continue;
                }
                stream
                    .decoder
                    .send_packet(&packet)
                    .map_err(|error| corrupt_input(path, error))?;
                if !drain_clip_decoder(
                    stream,
                    use_seek,
                    start_samples,
                    end_samples,
                    stop_sample,
                    max_scan_budget,
                    path,
                )? {
                    return Ok(ClipPassOutcome::FallbackLinear);
                }
                if streams
                    .iter()
                    .all(|s| s.sample_pos.is_some_and(|pos| pos >= stop_sample))
                {
                    break;
                }
            }
            Err(ffmpeg::Error::Eof) => {
                hit_eof = true;
                break;
            }
            Err(error) => return Err(corrupt_input(path, error)),
        }
    }

    if hit_eof {
        for stream in &mut streams {
            if stream.sample_pos.is_some_and(|pos| pos >= stop_sample) {
                continue;
            }
            stream
                .decoder
                .send_eof()
                .map_err(|error| corrupt_input(path, error))?;
            if !drain_clip_decoder(
                stream,
                use_seek,
                start_samples,
                end_samples,
                stop_sample,
                max_scan_budget,
                path,
            )? {
                return Ok(ClipPassOutcome::FallbackLinear);
            }
            if stream.sample_pos.is_some() {
                flush_clip_resampler(
                    stream,
                    start_samples,
                    end_samples,
                    stop_sample,
                    max_scan_budget,
                    path,
                )?;
            }
        }
    }

    let any_valid_params = streams
        .iter()
        .any(|stream| stream.decoder.channels() > 0 && stream.decoder.rate() > 0);
    let any_samples = streams.iter().any(|stream| stream.saw_any_samples);
    let any_saw_beyond = streams.iter().any(|stream| stream.saw_beyond);
    let exact_total_ms = if hit_eof {
        streams
            .iter()
            .filter_map(|stream| stream.sample_pos)
            .max()
            .map(|samples| samples / SAMPLES_PER_MS)
    } else {
        None
    };

    if use_seek && !any_samples {
        if container_duration_ms.is_some_and(|total| start_ms >= total) {
            return Ok(ClipPassOutcome::Done(None));
        }
        return Ok(ClipPassOutcome::FallbackLinear);
    }

    let mut clips: Vec<Vec<f32>> = streams
        .into_iter()
        .filter(|stream| !stream.clip.is_empty())
        .map(|stream| stream.clip)
        .collect();

    if clips.is_empty() {
        if !any_samples && start_ms == 0 {
            if !any_valid_params {
                return Err(AudioError::CorruptInput {
                    path: path.to_path_buf(),
                    detail: "audio stream has invalid codec parameters".to_owned(),
                    errno: None,
                });
            }
            return Err(AudioError::NoDecodedAudio {
                path: path.to_path_buf(),
            });
        }
        if !any_valid_params && !any_samples {
            return Err(AudioError::CorruptInput {
                path: path.to_path_buf(),
                detail: "audio stream has invalid codec parameters".to_owned(),
                errno: None,
            });
        }
        return Ok(ClipPassOutcome::Done(None));
    }

    let mut mixed = match clips.len() {
        1 => clips.pop().expect("one clip exists"),
        _ => mix_m4a_streams(clips),
    };
    mixed.truncate(want_samples);

    let wav = crate::wav::audio_to_wav_bytes(&mixed, SAMPLE_RATE)?;
    let actual_duration_ms = (mixed.len() as u64)
        .div_ceil(SAMPLES_PER_MS)
        .max(1)
        .min(effective_duration_ms);
    let has_more = any_saw_beyond
        && max_end_ms.is_none_or(|cap| start_ms.saturating_add(actual_duration_ms) < cap);
    let total_duration_ms = exact_total_ms
        .or(container_duration_ms)
        .map(|total| max_end_ms.map_or(total, |cap| total.min(cap)));

    Ok(ClipPassOutcome::Done(Some(DecodedAudioClip {
        wav,
        start_ms,
        duration_ms: actual_duration_ms,
        total_duration_ms,
        has_more,
        sample_rate_hz: SAMPLE_RATE,
        channels: 1,
    })))
}

fn audio_container_duration_ms(input: &ffmpeg::format::context::Input) -> Option<u64> {
    use ffmpeg::Rescale;
    let stream_ms = input
        .streams()
        .filter(|stream| stream.parameters().medium() == ffmpeg::media::Type::Audio)
        .filter_map(|stream| {
            let duration = stream.duration();
            if duration > 0 {
                let ms = duration.rescale(stream.time_base(), (1, 1_000));
                u64::try_from(ms).ok().filter(|ms| *ms > 0)
            } else {
                None
            }
        })
        .max();
    stream_ms.or_else(|| {
        let duration = input.duration();
        if duration > 0 {
            Some((duration as u64) / 1_000)
        } else {
            None
        }
    })
}

/// Drains decoded frames from one clip stream. Returns `Ok(false)` when a seek
/// pass could not establish a trustworthy starting timestamp before `start_samples`
/// and must fall back to a linear pass from the start of the file.
fn drain_clip_decoder(
    stream: &mut ClipStream,
    use_seek: bool,
    start_samples: u64,
    end_samples: u64,
    stop_sample: u64,
    max_scan_budget: u64,
    path: &Path,
) -> Result<bool, AudioError> {
    loop {
        let mut decoded = ffmpeg::frame::Audio::empty();
        match stream.decoder.receive_frame(&mut decoded) {
            Ok(()) => {
                normalize_channel_layout(&mut decoded);
                if stream.sample_pos.is_none() {
                    let Some(pts) = decoded.pts().or_else(|| decoded.timestamp()) else {
                        return Ok(false);
                    };
                    use ffmpeg::Rescale;
                    let rel_pts = pts.saturating_sub(stream.start_time);
                    let initial_pos = if rel_pts <= 0 {
                        0
                    } else {
                        rel_pts
                            .rescale(stream.time_base, (1, SAMPLE_RATE as i32))
                            .max(0) as u64
                    };
                    if use_seek
                        && (initial_pos > start_samples
                            || start_samples.saturating_sub(initial_pos) > MAX_SEEK_PREROLL_SAMPLES)
                    {
                        return Ok(false);
                    }
                    stream.sample_pos = Some(initial_pos);
                }
                if stream.resampler.is_none() {
                    stream.resampler = Some(make_resampler(&decoded, path)?);
                }
                let mut converted = ffmpeg::frame::Audio::empty();
                if decoded.rate() < SAMPLE_RATE {
                    let context = stream
                        .resampler
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
                stream
                    .resampler
                    .as_mut()
                    .expect("resampler initialized from decoded frame")
                    .run(&decoded, &mut converted)
                    .map_err(|error| ffmpeg_error(path, error))?;
                let samples = converted.plane::<f32>(0);
                consume_clip_samples(
                    stream,
                    samples,
                    start_samples,
                    end_samples,
                    stop_sample,
                    max_scan_budget,
                    path,
                )?;
                if stream.sample_pos.is_some_and(|pos| pos >= stop_sample) {
                    break;
                }
            }
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(corrupt_input(path, error)),
        }
    }
    Ok(true)
}

fn flush_clip_resampler(
    stream: &mut ClipStream,
    start_samples: u64,
    end_samples: u64,
    stop_sample: u64,
    max_scan_budget: u64,
    path: &Path,
) -> Result<(), AudioError> {
    let flush_sample_limit = usize::try_from(SAMPLE_RATE).expect("u32 always fits usize") * 2;
    let mut flushed_samples = 0;
    let mut iterations = 0;

    while let Some(delay) = stream.resampler.as_ref().and_then(|r| r.delay()) {
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
        let remaining = stream
            .resampler
            .as_mut()
            .expect("resampler present")
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
        consume_clip_samples(
            stream,
            samples,
            start_samples,
            end_samples,
            stop_sample,
            max_scan_budget,
            path,
        )?;
        flushed_samples = next_flushed_samples;
        if remaining.is_none()
            || produced == 0
            || stream.sample_pos.is_some_and(|p| p >= stop_sample)
        {
            break;
        }
    }
    Ok(())
}

fn consume_clip_samples(
    stream: &mut ClipStream,
    samples: &[f32],
    start_samples: u64,
    end_samples: u64,
    stop_sample: u64,
    max_scan_budget: u64,
    path: &Path,
) -> Result<(), AudioError> {
    let pos = stream.sample_pos.expect("sample_pos initialized");
    let len = samples.len() as u64;
    if len == 0 {
        return Ok(());
    }
    stream.saw_any_samples = true;
    stream.scanned_samples = stream.scanned_samples.saturating_add(len);
    if stream.scanned_samples > max_scan_budget {
        return Err(AudioError::DecodedSampleLimit {
            path: path.to_path_buf(),
            max_samples: usize::try_from(max_scan_budget).unwrap_or(usize::MAX),
        });
    }
    let next_pos = pos.saturating_add(len);
    if next_pos > start_samples && pos < end_samples {
        let lo = start_samples.saturating_sub(pos) as usize;
        let hi = (end_samples - pos).min(len) as usize;
        stream.clip.extend_from_slice(&samples[lo..hi]);
    }
    if stop_sample > end_samples && next_pos > end_samples {
        stream.saw_beyond = true;
    }
    stream.sample_pos = Some(next_pos);
    Ok(())
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

    struct TempAudioFile {
        path: std::path::PathBuf,
    }

    impl TempAudioFile {
        fn new(name: &str, bytes: &[u8]) -> Self {
            let unique = format!(
                "solstone-observe-audio-clip-{}-{}-{name}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            let path = std::env::temp_dir().join(unique);
            std::fs::write(&path, bytes).unwrap();
            Self { path }
        }

        fn open(&self) -> std::fs::File {
            std::fs::File::open(&self.path).unwrap()
        }
    }

    impl Drop for TempAudioFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn wav_with_metadata_tag(seconds: usize, sample_rate: u32, tag: &[u8]) -> Vec<u8> {
        let total = seconds * (sample_rate as usize);
        let mut samples = Vec::with_capacity(total);
        for index in 0..total {
            // Distinct amplitude step per second so window offsets are verifiable.
            let second = index / (sample_rate as usize);
            let amplitude = 0.05 * ((second % 10) as f32 + 1.0);
            let phase = 2.0 * std::f32::consts::PI * 440.0 * (index as f32) / (sample_rate as f32);
            samples.push(amplitude * phase.sin());
        }
        let mut wav = crate::wav::audio_to_wav_bytes(&samples, sample_rate).unwrap();
        // Append a RIFF LIST/INFO metadata chunk and update top-level RIFF size.
        let mut info = Vec::new();
        info.extend_from_slice(b"INFOINAM");
        info.extend_from_slice(&(tag.len() as u32).to_le_bytes());
        info.extend_from_slice(tag);
        if tag.len() % 2 == 1 {
            info.push(0);
        }
        wav.extend_from_slice(b"LIST");
        wav.extend_from_slice(&(info.len() as u32).to_le_bytes());
        wav.extend_from_slice(&info);
        let riff_size = (wav.len() - 8) as u32;
        wav[4..8].copy_from_slice(&riff_size.to_le_bytes());
        wav
    }

    #[test]
    fn decode_clip_wav_slices_strips_metadata_and_clamps_end() {
        let secret = b"SECRET_OWNER_METADATA_TAG";
        let raw = wav_with_metadata_tag(14, 16_000, secret);
        assert!(raw.windows(secret.len()).any(|window| window == secret));
        let file = TempAudioFile::new("tone.wav", &raw);

        // First 2-second clip from start (linear pass).
        let first = super::decode_clip_wav_from_file(file.open(), 0, 2_000, None)
            .unwrap()
            .expect("clip decoded");
        assert_eq!(first.start_ms, 0);
        assert_eq!(first.duration_ms, 2_000);
        assert_eq!(first.total_duration_ms, Some(14_000));
        assert!(first.has_more);
        assert_eq!(first.sample_rate_hz, 16_000);
        assert_eq!(first.channels, 1);
        assert_eq!(first.wav.len(), 44 + 2 * 16_000 * 2);
        assert_eq!(&first.wav[..4], b"RIFF");
        assert_eq!(&first.wav[8..12], b"WAVE");
        assert_eq!(&first.wav[36..40], b"data");
        assert!(
            !first
                .wav
                .windows(secret.len())
                .any(|window| window == secret),
            "output WAV must strip all container metadata"
        );

        // Seeked clip past SEEK_THRESHOLD_MS (11s..14s).
        let tail = super::decode_clip_wav_from_file(file.open(), 11_000, 10_000, None)
            .unwrap()
            .expect("tail decoded");
        assert_eq!(tail.start_ms, 11_000);
        assert_eq!(tail.duration_ms, 3_000);
        assert_eq!(tail.total_duration_ms, Some(14_000));
        assert!(!tail.has_more);
        assert_eq!(tail.wav.len(), 44 + 3 * 16_000 * 2);

        // Out-of-range offset returns Ok(None).
        assert_eq!(
            super::decode_clip_wav_from_file(file.open(), 20_000, 2_000, None).unwrap(),
            None
        );

        // max_end_ms clamps shared retained originals strictly to the segment's window.
        let clamped = super::decode_clip_wav_from_file(file.open(), 3_000, 5_000, Some(5_000))
            .unwrap()
            .expect("clamped clip decoded");
        assert_eq!(clamped.start_ms, 3_000);
        assert_eq!(clamped.duration_ms, 2_000);
        assert_eq!(clamped.total_duration_ms, Some(5_000));
        assert!(!clamped.has_more);
        assert_eq!(clamped.wav.len(), 44 + 2 * 16_000 * 2);
        assert_eq!(
            super::decode_clip_wav_from_file(file.open(), 5_000, 2_000, Some(5_000)).unwrap(),
            None
        );
    }

    #[test]
    fn decode_clip_wav_classifies_empty_stub_and_corrupt_inputs() {
        let empty = TempAudioFile::new("empty.wav", &[]);
        assert!(matches!(
            super::decode_clip_wav_from_file(empty.open(), 0, 5_000, None),
            Err(AudioError::EmptyInput { .. })
        ));

        const WATCH_STUB: &[u8] = &[
            0x00, 0x00, 0x00, 0x1c, b'f', b't', b'y', b'p', b'M', b'4', b'A', b' ', 0x00, 0x00,
            0x00, 0x00, b'M', b'4', b'A', b' ', b'm', b'p', b'4', b'2', b'i', b's', b'o', b'm',
        ];
        let stub = TempAudioFile::new("stub.m4a", WATCH_STUB);
        assert!(matches!(
            super::decode_clip_wav_from_file(stub.open(), 0, 5_000, None),
            Err(AudioError::EmptyInput { .. })
        ));

        let corrupt = TempAudioFile::new("broken.wav", b"not a valid audio file at all");
        assert!(matches!(
            super::decode_clip_wav_from_file(corrupt.open(), 0, 5_000, None),
            Err(AudioError::CorruptInput { .. })
        ));
    }
}
