// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use image::{
    ColorType, ImageEncoder,
    codecs::{jpeg::JpegEncoder, png::PngEncoder, webp::WebPEncoder},
};

use crate::error::GrabFailure;
use crate::extract::RgbFrame;

pub(crate) fn refuse_conflicts(paths: &[PathBuf], force: bool) -> Result<(), GrabFailure> {
    let conflicts: Vec<_> = paths
        .iter()
        .filter(|path| path.exists() && !force)
        .map(|path| path.display().to_string())
        .collect();
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(GrabFailure::runtime(format!(
            "output path exists (use --force): {}",
            conflicts.join(", ")
        )))
    }
}

pub(crate) fn save_frame(frame: &RgbFrame, target: &Path) -> Result<(), GrabFailure> {
    let suffix = target
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let file = File::create(target).map_err(|error| GrabFailure::Io {
        path: target.to_path_buf(),
        source: error,
    })?;
    let writer = BufWriter::new(file);
    let color = ColorType::Rgb8.into();
    match suffix.as_str() {
        "png" => {
            PngEncoder::new(writer).write_image(&frame.pixels, frame.width, frame.height, color)
        }
        "jpg" | "jpeg" => JpegEncoder::new_with_quality(writer, 95).write_image(
            &frame.pixels,
            frame.width,
            frame.height,
            color,
        ),
        // image 0.25.10 exposes no lossy quality-configurable WebP encoder.
        "webp" => WebPEncoder::new_lossless(writer).write_image(
            &frame.pixels,
            frame.width,
            frame.height,
            color,
        ),
        _ => {
            return Err(GrabFailure::runtime(
                "--out must end in .png, .jpg, .jpeg, or .webp",
            ));
        }
    }
    .map_err(|error| GrabFailure::runtime(format!("failed to save image: {error}")))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use image::ImageReader;
    use tempfile::tempdir;

    use super::{RgbFrame, refuse_conflicts, save_frame};

    fn frame() -> RgbFrame {
        RgbFrame {
            width: 2,
            height: 1,
            pixels: vec![1, 2, 3, 200, 201, 202],
        }
    }

    #[test]
    fn png_preserves_decoded_pixels_and_jpeg_is_bounded() {
        let temp = tempdir().unwrap();
        let png = temp.path().join("frame.png");
        let jpeg = temp.path().join("frame.jpg");
        save_frame(&frame(), &png).unwrap();
        save_frame(&frame(), &jpeg).unwrap();
        assert_eq!(
            ImageReader::open(png)
                .unwrap()
                .decode()
                .unwrap()
                .into_rgb8()
                .into_raw(),
            frame().pixels
        );
        let decoded = ImageReader::open(jpeg)
            .unwrap()
            .decode()
            .unwrap()
            .into_rgb8()
            .into_raw();
        assert!(
            decoded
                .iter()
                .zip(&frame().pixels)
                .all(|(actual, expected)| actual.abs_diff(*expected) <= 8)
        );
    }

    #[test]
    fn webp_lossless_encoder_writes_a_decodable_image() {
        let temp = tempdir().unwrap();
        let webp = temp.path().join("frame.webp");
        save_frame(&frame(), &webp).unwrap();
        assert!(ImageReader::open(webp).unwrap().decode().is_ok());
    }

    #[test]
    fn all_conflicts_are_named_before_decode() {
        let temp = tempdir().unwrap();
        let first = temp.path().join("first.png");
        let second = temp.path().join("second.png");
        fs::write(&first, "x").unwrap();
        fs::write(&second, "x").unwrap();
        let error = refuse_conflicts(&[first, second], false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("first.png") && error.contains("second.png"));
        refuse_conflicts(&[], false).unwrap();
    }
}
