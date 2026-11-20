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
    let input = open_input(path).map_err(decode_error)?;
    let mut scaler = None;
    let mut results = vec![None; ids.len()];
    let mut decoded_index = 0_i64;
    visit_frames(input, |frame, _| {
        decoded_index += 1;
        for (position, _) in ids
            .iter()
            .enumerate()
            .filter(|(_, id)| **id == decoded_index)
        {
            results[position] = Some(to_rgb(&frame, &mut scaler)?);
        }
        Ok(results.iter().all(Option::is_some))
    })?;
    Ok(results)
}

/// The frame on screen at a segment-relative timestamp, read only from the
/// admitted recording. The selected frame starts at or before the requested
/// time; the next presentation timestamp ends that frame's interval.
pub fn frame_at(
    file: std::fs::File,
    offset_ms: u32,
) -> Result<Option<(u64, image::RgbImage)>, GrabFailure> {
    ffmpeg::init().map_err(decode_error)?;
    let io = ffmpeg::format::context::StreamIo::from_read_seek(file).map_err(decode_error)?;
    let mut options = ffmpeg::Dictionary::new();
    options.set("format_whitelist", INPUT_FORMATS);
    options.set("protocol_whitelist", "");
    let mut input =
        ffmpeg::format::input_from_stream(io, None, Some(options)).map_err(decode_error)?;
    let wanted_us = i64::from(offset_ms) * 1000;
    let video = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| GrabFailure::runtime("failed to decode video: no video stream"))?;
    let duration_us = if video.duration() > 0 {
        use ffmpeg::Rescale;
        let start_pts = video.start_time().max(0);
        video
            .duration()
            .saturating_add(start_pts)
            .rescale(video.time_base(), (1, 1_000_000))
    } else {
        input.duration()
    };
    if duration_us > 0 && wanted_us >= duration_us {
        return Ok(None);
    }
    // Seek to a keyframe at or before the moment. Unindexed recordings still
    // work with the ordinary linear decode from their beginning.
    if offset_ms > 0 {
        let _ = input.seek(wanted_us, ..wanted_us);
    }
    let mut selected = None;
    visit_frames(input, |frame, base| {
        let seconds = frame.pts().expect("visit_frames checks pts") as f64 * f64::from(base);
        if seconds * 1000.0 > f64::from(offset_ms) + 0.000_001 {
            return Ok(true);
        }
        selected = Some(((seconds * 1000.0).round() as u64, frame));
        Ok(false)
    })?;
    let Some((actual, frame)) = selected else {
        return Ok(None);
    };
    let rgb = to_rgb(&frame, &mut None)?;
    let image = image::RgbImage::from_raw(rgb.width, rgb.height, rgb.pixels)
        .ok_or_else(|| GrabFailure::runtime("invalid RGB frame"))?;
    Ok(Some((actual, image)))
}

fn visit_frames(
    mut input: ffmpeg::format::context::Input,
    mut visit: impl FnMut(ffmpeg::frame::Video, ffmpeg::Rational) -> Result<bool, GrabFailure>,
) -> Result<(), GrabFailure> {
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| GrabFailure::runtime("failed to decode video: no video stream"))?;
    let index = stream.index();
    let base = stream.time_base();
    let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .map_err(decode_error)?;
    let mut decoder = context.decoder().video().map_err(decode_error)?;
    loop {
        let mut packet = ffmpeg::Packet::empty();
        let eof = match packet.read(&mut input) {
            Ok(()) if packet.stream() != index => continue,
            Ok(()) => {
                decoder.send_packet(&packet).map_err(decode_error)?;
                false
            }
            Err(ffmpeg::Error::Eof) => {
                decoder.send_eof().map_err(decode_error)?;
                true
            }
            Err(error) => return Err(decode_error(error)),
        };
        loop {
            let mut frame = ffmpeg::frame::Video::empty();
            match decoder.receive_frame(&mut frame) {
                Ok(()) => {
                    if frame.pts().is_some() && visit(frame, base)? {
                        return Ok(());
                    }
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
                Err(ffmpeg::Error::Eof) => return Ok(()),
                Err(error) => return Err(decode_error(error)),
            }
        }
        if eof {
            return Ok(());
        }
    }
}

fn to_rgb(
    frame: &ffmpeg::frame::Video,
    scaler: &mut Option<ffmpeg::software::scaling::context::Context>,
) -> Result<RgbFrame, GrabFailure> {
    let mut rgb = ffmpeg::frame::Video::empty();
    rgb_scaler_for(scaler, frame)
        .and_then(|s| s.run(frame, &mut rgb))
        .map_err(decode_error)?;
    copy_rgb(&rgb).ok_or_else(|| GrabFailure::runtime("failed to decode video: invalid RGB frame"))
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
