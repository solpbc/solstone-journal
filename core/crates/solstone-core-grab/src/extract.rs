// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use ffmpeg_next as ffmpeg;

use crate::error::GrabFailure;

/// Demuxers this call site reads; every other demuxer compiled into FFmpeg is refused.
const INPUT_FORMATS: &str = "matroska,mov";

/// Opens a local media file, restricted to [`INPUT_FORMATS`] and the `file` protocol.
fn open_input(path: &Path) -> Result<ffmpeg::format::context::Input, ffmpeg::Error> {
    let mut options = ffmpeg::Dictionary::new();
    options.set("protocol_whitelist", "file");
    options.set("format_whitelist", INPUT_FORMATS);
    ffmpeg::format::input_with_dictionary(path, options)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RgbFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub(crate) fn decode_frames(
    path: &Path,
    ids: &[i64],
) -> Result<Vec<Option<RgbFrame>>, GrabFailure> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    ffmpeg::init().map_err(|error| {
        GrabFailure::runtime(format!("failed to initialize video decoder: {error}"))
    })?;
    let mut input = open_input(path)
        .map_err(|error| GrabFailure::runtime(format!("failed to decode video: {error}")))?;
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| GrabFailure::runtime("failed to decode video: no video stream"))?;
    let index = stream.index();
    let parameters = stream.parameters();
    let context =
        ffmpeg::codec::context::Context::from_parameters(parameters).map_err(decode_error)?;
    let mut decoder = context.decoder().video().map_err(decode_error)?;
    // Built from each decoded frame, never from stream parameters: a truncated
    // file can leave the stream's pixel format unknown, and FFmpeg aborts the
    // process when asked to scale from an unknown format.
    let mut scaler = None;
    let mut results = vec![None; ids.len()];
    let mut decoded_index = 0_i64;
    loop {
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => {
                if packet.stream() == index {
                    decoder.send_packet(&packet).map_err(decode_error)?;
                    receive(
                        &mut decoder,
                        &mut scaler,
                        ids,
                        &mut results,
                        &mut decoded_index,
                    )?;
                    if results.iter().all(Option::is_some) {
                        return Ok(results);
                    }
                }
            }
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(decode_error(error)),
        }
    }
    decoder.send_eof().map_err(decode_error)?;
    receive(
        &mut decoder,
        &mut scaler,
        ids,
        &mut results,
        &mut decoded_index,
    )?;
    Ok(results)
}

fn rgb_scaler_for<'a>(
    slot: &'a mut Option<ffmpeg::software::scaling::context::Context>,
    decoded: &ffmpeg::frame::Video,
) -> Result<&'a mut ffmpeg::software::scaling::context::Context, ffmpeg::Error> {
    let matches_frame = slot.as_ref().is_some_and(|scaler| {
        let input = scaler.input();
        input.format == decoded.format()
            && input.width == decoded.width()
            && input.height == decoded.height()
    });
    if !matches_frame {
        *slot = Some(ffmpeg::software::scaling::context::Context::get(
            decoded.format(),
            decoded.width(),
            decoded.height(),
            ffmpeg::format::Pixel::RGB24,
            decoded.width(),
            decoded.height(),
            ffmpeg::software::scaling::flag::Flags::BILINEAR,
        )?);
    }
    slot.as_mut().ok_or(ffmpeg::Error::Bug)
}

fn receive(
    decoder: &mut ffmpeg::decoder::Video,
    scaler: &mut Option<ffmpeg::software::scaling::context::Context>,
    ids: &[i64],
    results: &mut [Option<RgbFrame>],
    decoded_index: &mut i64,
) -> Result<(), GrabFailure> {
    loop {
        let mut frame = ffmpeg::frame::Video::empty();
        match decoder.receive_frame(&mut frame) {
            Ok(()) => {
                if frame.pts().is_none() {
                    continue;
                }
                for (position, _id) in ids
                    .iter()
                    .enumerate()
                    .filter(|(_, id)| **id - 1 == *decoded_index)
                {
                    let mut rgb = ffmpeg::frame::Video::empty();
                    rgb_scaler_for(scaler, &frame)
                        .and_then(|scaler| scaler.run(&frame, &mut rgb))
                        .map_err(decode_error)?;
                    results[position] = Some(copy_rgb(&rgb).ok_or_else(|| {
                        GrabFailure::runtime("failed to decode video: invalid RGB frame")
                    })?);
                }
                *decoded_index += 1;
            }
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => return Ok(()),
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(error) => return Err(decode_error(error)),
        }
    }
}

fn copy_rgb(frame: &ffmpeg::frame::Video) -> Option<RgbFrame> {
    let width = usize::try_from(frame.width()).ok()?;
    let height = usize::try_from(frame.height()).ok()?;
    let row = width.checked_mul(3)?;
    if frame.planes() == 0 || frame.stride(0) < row {
        return None;
    }
    let data = frame.data(0);
    let needed = frame.stride(0).checked_mul(height)?;
    if data.len() < needed {
        return None;
    }
    let mut pixels = Vec::with_capacity(row.checked_mul(height)?);
    for offset in (0..height).map(|row_index| row_index * frame.stride(0)) {
        pixels.extend_from_slice(data.get(offset..offset + row)?);
    }
    Some(RgbFrame {
        width: frame.width(),
        height: frame.height(),
        pixels,
    })
}

fn decode_error(error: ffmpeg::Error) -> GrabFailure {
    GrabFailure::runtime(format!("failed to decode video: {error}"))
}
