// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{FaceDetection, PassResult};
use image::{Rgb, RgbImage};
use ort::{session::Session, value::Tensor};

mod dynamic_model;

pub struct FaceModels {
    detector: Session,
    embedder: Session,
}

impl FaceModels {
    pub fn open() -> PassResult<Self> {
        #[cfg(windows)]
        solstone_core_speakers_onnx::windows_runtime::bootstrap_windows_onnx()?;
        #[cfg(not(windows))]
        {
            let runtime = if let Some(path) = std::env::var_os("ORT_DYLIB_PATH") {
                std::path::PathBuf::from(path)
            } else {
                solstone_core_installed_payload::resolve_current_screen_ocr_bundle()
                    .map_err(|e| e.to_string())?
                    .runtime_library
                    .path()
                    .to_path_buf()
            };
            ort::init_from(runtime).map_err(|e| e.to_string())?.commit();
        }
        let open = |name| -> PassResult<Session> {
            let path =
                solstone_core_transcribe::resolve_model_asset(name).map_err(|e| e.to_string())?;
            let mut builder = Session::builder()
                .map_err(|e| e.to_string())?
                .with_intra_threads(2)
                .map_err(|e| e.to_string())?;
            builder.commit_from_file(path).map_err(|e| e.to_string())
        };
        // YuNet is fully convolutional, but its ONNX shape declarations say
        // 640x640. Relax only those declarations in memory, like setInputSize.
        // The bundled graph and its verified digest remain unchanged.
        let detector_path =
            solstone_core_transcribe::resolve_model_asset("face_detection_yunet_2023mar.onnx")
                .map_err(|e| e.to_string())?;
        let bytes = std::fs::read(detector_path).map_err(|e| e.to_string())?;
        let bytes = dynamic_model::yunet(&bytes)?;
        let detector = Session::builder()
            .map_err(|e| e.to_string())?
            .with_intra_threads(2)
            .map_err(|e| e.to_string())?
            .commit_from_memory(&bytes)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            detector,
            embedder: open("face_recognition_sface_2021dec.onnx")?,
        })
    }

    /// YuNet's BGR input, bottom/right 32-pixel padding, and three stride heads.
    /// The decode and alignment follow OpenCV's FaceDetectorYN/FaceRecognizerSF.
    pub fn detect(&mut self, image: &RgbImage) -> PassResult<Vec<FaceDetection>> {
        let scale = if image.width() <= 1920 { 1.5 } else { 1.0 };
        let scaled = if scale > 1.0 {
            resize_cubic(image, scale)
        } else {
            image.clone()
        };
        let w = scaled.width().div_ceil(32) as usize * 32;
        let h = scaled.height().div_ceil(32) as usize * 32;
        let mut input = vec![0.0; 3 * w * h];
        for (x, y, p) in scaled.enumerate_pixels() {
            for c in 0..3 {
                input[c * w * h + y as usize * w + x as usize] = p[2 - c] as f32;
            }
        }
        let input = Tensor::from_array(([1, 3, h, w], input)).map_err(|e| e.to_string())?;
        let output = self
            .detector
            .run(ort::inputs![input])
            .map_err(|e| e.to_string())?;
        let mut candidates = Vec::new();
        for stride in [8, 16, 32] {
            let get = |head: &str| -> PassResult<&[f32]> {
                output[format!("{head}_{stride}")]
                    .try_extract_tensor::<f32>()
                    .map(|(_, v)| v)
                    .map_err(|e| e.to_string())
            };
            let cls = get("cls")?;
            let obj = get("obj")?;
            let bbox = get("bbox")?;
            let kps = get("kps")?;
            let cols = w / stride;
            let rows = h / stride;
            if cls.len() != rows * cols
                || obj.len() != cls.len()
                || bbox.len() != cls.len() * 4
                || kps.len() != cls.len() * 10
            {
                return Err("YuNet output shape mismatch".into());
            }
            for i in 0..cls.len() {
                let score = (cls[i].clamp(0., 1.) * obj[i].clamp(0., 1.)).sqrt();
                if score < 0.7 {
                    continue;
                }
                let s = stride as f32;
                let cx = (i % cols) as f32;
                let cy = (i / cols) as f32;
                let fw = bbox[i * 4 + 2].exp() * s;
                let fh = bbox[i * 4 + 3].exp() * s;
                let mut lm = [0.; 10];
                for n in 0..5 {
                    lm[2 * n] = (kps[i * 10 + 2 * n] + cx) * s;
                    lm[2 * n + 1] = (kps[i * 10 + 2 * n + 1] + cy) * s;
                }
                candidates.push(FaceDetection {
                    bbox: [
                        (cx + bbox[i * 4]) * s - fw / 2.,
                        (cy + bbox[i * 4 + 1]) * s - fh / 2.,
                        fw,
                        fh,
                    ],
                    landmarks: lm,
                    score,
                });
            }
        }
        candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
        candidates.truncate(200);
        let mut faces: Vec<FaceDetection> = Vec::new();
        for f in candidates {
            if faces.iter().any(|k| iou(k.bbox, f.bbox) > 0.3) {
                continue;
            }
            // NMS is on the detector's integer boxes, before native-size scaling.
            faces.push(f);
        }
        for f in &mut faces {
            for v in f.bbox.iter_mut().chain(f.landmarks.iter_mut()) {
                *v /= scale;
            }
        }
        Ok(faces.into_iter().filter(|f| f.bbox[2] >= 18.).collect())
    }

    pub fn embed(
        &mut self,
        image: &RgbImage,
        face: &FaceDetection,
    ) -> PassResult<([f32; 128], f32)> {
        let crop = align(image, &face.landmarks)?;
        let mut data = vec![0.; 3 * 112 * 112];
        for (x, y, p) in crop.enumerate_pixels() {
            for c in 0..3 {
                data[c * 112 * 112 + y as usize * 112 + x as usize] = p[c] as f32;
            }
        }
        let input = Tensor::from_array(([1, 3, 112, 112], data)).map_err(|e| e.to_string())?;
        let out = self
            .embedder
            .run(ort::inputs![input])
            .map_err(|e| e.to_string())?;
        let (_, values) = out[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        let mut emb: [f32; 128] = values
            .try_into()
            .map_err(|_| "SFace output is not 128-dimensional")?;
        let norm = emb.iter().map(|v| v * v).sum::<f32>().sqrt();
        if !norm.is_finite() || norm <= 0. {
            return Err("SFace returned an invalid embedding".into());
        }
        for v in &mut emb {
            *v /= norm;
        }
        Ok((emb, sharpness(&crop)))
    }
}

fn iou(a: [f32; 4], b: [f32; 4]) -> f32 {
    let a = a.map(|v| v as i32 as f32);
    let b = b.map(|v| v as i32 as f32);
    let inter = ((a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0])).max(0.)
        * ((a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1])).max(0.);
    inter / (a[2] * a[3] + b[2] * b[3] - inter).max(1.)
}

/// Least-squares orientation-preserving five-point similarity transform.
/// Inverse sampling uses OpenCV's 1/32-pixel linear interpolation grid.
pub fn align(image: &RgbImage, lm: &[f32; 10]) -> PassResult<RgbImage> {
    let dst = [
        [38.2946, 51.6963],
        [73.5318, 51.5014],
        [56.0252, 71.7366],
        [41.5493, 92.3655],
        [70.7299, 92.2041],
    ];
    let sx = (0..5).map(|i| lm[2 * i] as f64).sum::<f64>() / 5.;
    let sy = (0..5).map(|i| lm[2 * i + 1] as f64).sum::<f64>() / 5.;
    let dx = 56.0262;
    let dy = 71.9008;
    let (mut a, mut b, mut den) = (0., 0., 0.);
    for i in 0..5 {
        let x = lm[2 * i] as f64 - sx;
        let y = lm[2 * i + 1] as f64 - sy;
        let u = dst[i][0] - dx;
        let v = dst[i][1] - dy;
        a += x * u + y * v;
        b += x * v - y * u;
        den += x * x + y * y;
    }
    if den < 1e-9 {
        return Err("degenerate face landmarks".into());
    }
    a /= den;
    b /= den;
    let tx = dx - a * sx + b * sy;
    let ty = dy - b * sx - a * sy;
    let d = a * a + b * b;
    if d < 1e-12 {
        return Err("degenerate face alignment".into());
    }
    Ok(RgbImage::from_fn(112, 112, |x, y| {
        let u = x as f64 - tx;
        let v = y as f64 - ty;
        let x = ((a * u + b * v) / d * 32.).round_ties_even() / 32.;
        let y = ((-b * u + a * v) / d * 32.).round_ties_even() / 32.;
        let ix = x.floor() as i32;
        let iy = y.floor() as i32;
        let fx = x - x.floor();
        let fy = y - y.floor();
        let mut p = [0; 3];
        for (c, out) in p.iter_mut().enumerate() {
            let mut sum = 0.;
            for oy in 0..2 {
                for ox in 0..2 {
                    let xx = ix + ox;
                    let yy = iy + oy;
                    if xx >= 0 && yy >= 0 && xx < image.width() as i32 && yy < image.height() as i32
                    {
                        sum += image.get_pixel(xx as u32, yy as u32)[c] as f64
                            * if ox == 0 { 1. - fx } else { fx }
                            * if oy == 0 { 1. - fy } else { fy };
                    }
                }
            }
            *out = sum.round().clamp(0., 255.) as u8;
        }
        Rgb(p)
    }))
}

pub fn resize_cubic(image: &RgbImage, scale: f32) -> RgbImage {
    let w = (image.width() as f32 * scale).round_ties_even() as u32;
    let h = (image.height() as f32 * scale).round_ties_even() as u32;
    let coeff = |x: f64| {
        let x = x.abs();
        if x <= 1. {
            1.25 * x * x * x - 2.25 * x * x + 1.
        } else if x < 2. {
            -0.75 * x * x * x + 3.75 * x * x - 6. * x + 3.
        } else {
            0.
        }
    };
    RgbImage::from_fn(w, h, |x, y| {
        let sx = (x as f64 + 0.5) / scale as f64 - 0.5;
        let sy = (y as f64 + 0.5) / scale as f64 - 0.5;
        let mut p = [0; 3];
        for (c, out) in p.iter_mut().enumerate() {
            let mut value = 0.;
            for yy in -1..=2 {
                for xx in -1..=2 {
                    let ix = sx.floor() as i32 + xx;
                    let iy = sy.floor() as i32 + yy;
                    value += image.get_pixel(
                        ix.clamp(0, image.width() as i32 - 1) as u32,
                        iy.clamp(0, image.height() as i32 - 1) as u32,
                    )[c] as f64
                        * coeff(sx - ix as f64)
                        * coeff(sy - iy as f64);
                }
            }
            *out = value.round_ties_even().clamp(0., 255.) as u8;
        }
        Rgb(p)
    })
}

fn sharpness(image: &RgbImage) -> f32 {
    let gray: Vec<f64> = image
        .pixels()
        .map(|p| (0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64).round())
        .collect();
    let sample = |x: i32, y: i32| {
        let reflect = |v: i32| {
            if v < 0 {
                -v
            } else if v >= 112 {
                222 - v
            } else {
                v
            }
        };
        gray[(reflect(y) * 112 + reflect(x)) as usize]
    };
    let (mut sum, mut sq) = (0., 0.);
    for y in 0..112 {
        for x in 0..112 {
            let v = sample(x - 1, y) + sample(x + 1, y) + sample(x, y - 1) + sample(x, y + 1)
                - 4. * sample(x, y);
            sum += v;
            sq += v * v;
        }
    }
    (sq / 12544. - (sum / 12544.).powi(2)) as f32
}
