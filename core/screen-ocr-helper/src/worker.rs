// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::protocol::{self, ErrorBody, ErrorCode, Header, ReadyIdentity, Region, ResponseBody};
use crate::recipe::{ModelAsset, Recipe};
use crate::{WorkerError, WorkerOptions};
use image::{ImageFormat, ImageReader, RgbImage};
use oar_ocr::core::config::{OrtExecutionProvider, OrtSessionConfig};
use oar_ocr::domain::tasks::{TextDetectionConfig, TextRecognitionConfig};
use oar_ocr::oarocr::{OAROCR, OAROCRBuilder};
use oar_ocr::processors::LimitType;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};

pub(crate) fn run(options: WorkerOptions, recipe: Recipe) -> Result<(), WorkerError> {
    let (ocr, identity) = initialize(options, &recipe)?;
    let mut stdout = io::stdout().lock();
    let ready_body = serde_json::to_vec(&identity).map_err(|_| WorkerError::Startup)?;
    protocol::write_message(
        &mut stdout,
        &Header::Ready {
            version: protocol::PROTOCOL_VERSION,
        },
        &ready_body,
    )
    .map_err(|_| WorkerError::Protocol)?;
    stdout.flush().map_err(|_| WorkerError::Protocol)?;

    let mut stdin = io::stdin().lock();
    while let Some((header, png)) =
        protocol::read_message(&mut stdin).map_err(|_| WorkerError::Protocol)?
    {
        let Header::Request {
            request_id,
            png_sha256,
            ..
        } = &header
        else {
            return Err(WorkerError::Protocol);
        };
        protocol::validate_png_binding(&header, &png).map_err(|_| WorkerError::Protocol)?;

        let image = match decode_png(&png, recipe.limits.max_decoded_pixels) {
            Ok(image) => image,
            Err(()) => {
                write_error(&mut stdout, request_id, png_sha256, ErrorCode::InvalidPng)?;
                continue;
            }
        };

        let (width, height) = image.dimensions();
        let response = match recognize(&ocr, image, width, height) {
            Ok(response) => response,
            Err(code) => {
                write_error(&mut stdout, request_id, png_sha256, code)?;
                return Err(WorkerError::FatalRequest);
            }
        };
        if protocol::validate_response_body(&response, width, height).is_err() {
            write_error(
                &mut stdout,
                request_id,
                png_sha256,
                ErrorCode::ResponseLimit,
            )?;
            return Err(WorkerError::FatalRequest);
        }

        let response_bytes = limited_json(&response).map_err(|_| {
            let _ = write_error(
                &mut stdout,
                request_id,
                png_sha256,
                ErrorCode::ResponseLimit,
            );
            WorkerError::FatalRequest
        })?;
        protocol::write_message(
            &mut stdout,
            &Header::Response {
                version: protocol::PROTOCOL_VERSION,
                request_id: request_id.clone(),
                png_sha256: png_sha256.clone(),
            },
            &response_bytes,
        )
        .map_err(|_| WorkerError::Protocol)?;
        stdout.flush().map_err(|_| WorkerError::Protocol)?;
    }
    Ok(())
}

fn initialize(
    options: WorkerOptions,
    recipe: &Recipe,
) -> Result<(OAROCR, ReadyIdentity), WorkerError> {
    // ORT reads these settings during initialization. Set them before Rayon
    // creates its global pool or any native runtime work begins.
    unsafe {
        std::env::set_var("ORT_DISABLE_TELEMETRY", "1");
        std::env::remove_var("ORT_DYLIB_PATH");
    }
    if recipe.settings.rayon_threads != 1 || !recipe.settings.cpu_only || recipe.settings.telemetry
    {
        return Err(WorkerError::Startup);
    }
    rayon::ThreadPoolBuilder::new()
        .num_threads(recipe.settings.rayon_threads)
        .build_global()
        .map_err(|_| WorkerError::Startup)?;

    let asset_dir = options
        .asset_dir
        .canonicalize()
        .map_err(|_| WorkerError::Startup)?;
    let (runtime_root, runtime_library) =
        canonical_private_runtime_paths(&options.runtime_root, &options.runtime_library, |path| {
            path.canonicalize().map_err(|_| ())
        })?;
    if !runtime_root.is_dir() || !runtime_library.is_file() {
        return Err(WorkerError::Startup);
    }

    let detector_path = asset_path(&asset_dir, &recipe.models.detector.filename)?;
    let recognizer_path = asset_path(&asset_dir, &recipe.models.recognizer.filename)?;
    let dictionary_path = asset_path(&asset_dir, &recipe.models.dictionary.filename)?;
    let detector_sha256 = verify_model(&detector_path, &recipe.models.detector)?;
    let recognizer_sha256 = verify_model(&recognizer_path, &recipe.models.recognizer)?;
    let dictionary_len = dictionary_path
        .metadata()
        .map_err(|_| WorkerError::Startup)?
        .len();
    if dictionary_len > recipe.limits.max_dictionary_bytes as u64 {
        return Err(WorkerError::Startup);
    }
    let dictionary_bytes = std::fs::read(&dictionary_path).map_err(|_| WorkerError::Startup)?;
    let dictionary_sha256 = protocol::sha256_hex(&dictionary_bytes);
    let dictionary = std::str::from_utf8(&dictionary_bytes).map_err(|_| WorkerError::Startup)?;
    if dictionary_sha256 != recipe.models.dictionary.materialized_sha256
        || !dictionary_bytes.ends_with(b"\n")
        || dictionary.lines().count() != recipe.models.dictionary.materialized_entries
    {
        return Err(WorkerError::Startup);
    }

    let runtime_sha256 = hash_file(&runtime_library)?;
    let helper_path = std::env::current_exe()
        .map_err(|_| WorkerError::Startup)?
        .canonicalize()
        .map_err(|_| WorkerError::Startup)?;
    let helper_sha256 = hash_file(&helper_path)?;

    let initialized = ort::init_from(&runtime_library)
        .map_err(|_| WorkerError::Startup)?
        .with_telemetry(false)
        .commit();
    require_runtime_commit(initialized)?;

    let detector_limit_type = match recipe.settings.detector_limit_type.as_str() {
        "max" => LimitType::Max,
        _ => return Err(WorkerError::Startup),
    };
    let detection_config = TextDetectionConfig {
        score_threshold: recipe.settings.detector_score_threshold,
        box_threshold: recipe.settings.detector_box_threshold,
        unclip_ratio: recipe.settings.detector_unclip_ratio,
        max_candidates: recipe.settings.detector_max_candidates,
        limit_side_len: Some(recipe.settings.detector_limit_side_len),
        limit_type: Some(detector_limit_type),
        max_side_len: Some(recipe.settings.detector_max_side_len),
    };
    let recognition_config = TextRecognitionConfig {
        score_threshold: recipe.settings.recognizer_score_threshold,
    };
    let ort_config = OrtSessionConfig::new()
        .with_execution_providers(vec![OrtExecutionProvider::CPU])
        .with_intra_threads(recipe.settings.ort_intra_threads)
        .with_inter_threads(recipe.settings.ort_inter_threads);
    let ocr = OAROCRBuilder::new(&detector_path, &recognizer_path, &dictionary_path)
        .text_detection_config(detection_config)
        .text_recognition_config(recognition_config)
        .ort_session(ort_config)
        .image_batch_size(recipe.settings.image_batch_size)
        .region_batch_size(recipe.settings.region_batch_size)
        .build()
        .map_err(|_| WorkerError::Startup)?;

    let identity = ReadyIdentity {
        protocol_version: recipe.protocol_version,
        recipe_sha256: recipe.identity(),
        helper_sha256,
        detector_sha256,
        recognizer_sha256,
        dictionary_sha256,
        runtime_sha256,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        settings: recipe.settings.clone(),
        limits: recipe.limits.clone(),
    };
    Ok((ocr, identity))
}

fn require_runtime_commit(committed: bool) -> Result<(), WorkerError> {
    committed.then_some(()).ok_or(WorkerError::Startup)
}

fn canonical_private_runtime_paths(
    runtime_root: &Path,
    runtime_library: &Path,
    canonicalize: impl Fn(&Path) -> Result<PathBuf, ()>,
) -> Result<(PathBuf, PathBuf), WorkerError> {
    if runtime_root.as_os_str().is_empty() || runtime_library.as_os_str().is_empty() {
        return Err(WorkerError::Startup);
    }

    let runtime_root = canonicalize(runtime_root).map_err(|_| WorkerError::Startup)?;
    let runtime_library = canonicalize(runtime_library).map_err(|_| WorkerError::Startup)?;
    if !runtime_root.is_absolute()
        || runtime_root.parent().is_none()
        || !runtime_library.is_absolute()
        || runtime_library == runtime_root
        || !runtime_library.starts_with(&runtime_root)
    {
        return Err(WorkerError::Startup);
    }
    Ok((runtime_root, runtime_library))
}

fn asset_path(asset_dir: &Path, filename: &str) -> Result<PathBuf, WorkerError> {
    let path = asset_dir
        .join(filename)
        .canonicalize()
        .map_err(|_| WorkerError::Startup)?;
    if !path.is_file() || !path.starts_with(asset_dir) {
        return Err(WorkerError::Startup);
    }
    Ok(path)
}

fn verify_model(path: &Path, model: &ModelAsset) -> Result<String, WorkerError> {
    let digest = hash_file(path)?;
    if digest != model.sha256 {
        return Err(WorkerError::Startup);
    }
    Ok(digest)
}

fn hash_file(path: &Path) -> Result<String, WorkerError> {
    let mut file = File::open(path).map_err(|_| WorkerError::Startup)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|_| WorkerError::Startup)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

fn decode_png(bytes: &[u8], max_pixels: u64) -> Result<RgbImage, ()> {
    if bytes.is_empty() || bytes.len() > protocol::MAX_PNG_BYTES {
        return Err(());
    }
    let dimensions = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png)
        .into_dimensions()
        .map_err(|_| ())?;
    if !protocol::dimensions_within_pixel_limit(dimensions.0, dimensions.1, max_pixels) {
        return Err(());
    }
    let image = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png)
        .decode()
        .map_err(|_| ())?
        .into_rgb8();
    Ok(image)
}

fn recognize(
    ocr: &OAROCR,
    image: RgbImage,
    width: u32,
    height: u32,
) -> Result<ResponseBody, ErrorCode> {
    let result = ocr
        .predict(vec![image])
        .map_err(|_| ErrorCode::InferenceFailed)?;
    let result = result.first().ok_or(ErrorCode::InferenceFailed)?;
    if result.text_regions.len() > protocol::MAX_REGIONS {
        return Err(ErrorCode::ResponseLimit);
    }
    let mut regions = Vec::with_capacity(result.text_regions.len());
    for region in &result.text_regions {
        let Some(text) = &region.text else {
            continue;
        };
        if text.len() > protocol::MAX_TEXT_BYTES
            || region.bounding_box.points.is_empty()
            || region.bounding_box.points.len() > protocol::MAX_GEOMETRY_POINTS
            || region
                .confidence
                .is_some_and(|score| !score.is_finite() || !(0.0..=1.0).contains(&score))
            || region.bounding_box.points.iter().any(|point| {
                !point.x.is_finite()
                    || !point.y.is_finite()
                    || point.x < 0.0
                    || point.y < 0.0
                    || point.x > width as f32
                    || point.y > height as f32
            })
        {
            return Err(ErrorCode::ResponseLimit);
        }
        regions.push(Region {
            text: text.to_string(),
            confidence: region.confidence,
            points: region
                .bounding_box
                .points
                .iter()
                .map(|point| protocol::Point {
                    x: point.x,
                    y: point.y,
                })
                .collect(),
        });
    }
    Ok(ResponseBody { regions })
}

fn limited_json(value: &impl serde::Serialize) -> Result<Vec<u8>, ()> {
    let mut output = LimitedBuffer::new(protocol::MAX_RESPONSE_BYTES);
    serde_json::to_writer(&mut output, value).map_err(|_| ())?;
    Ok(output.into_inner())
}

fn write_error<W: Write>(
    writer: &mut W,
    request_id: &str,
    png_sha256: &str,
    code: ErrorCode,
) -> Result<(), WorkerError> {
    let body = serde_json::to_vec(&ErrorBody { code }).map_err(|_| WorkerError::Protocol)?;
    protocol::write_message(
        writer,
        &Header::Error {
            version: protocol::PROTOCOL_VERSION,
            request_id: request_id.to_string(),
            png_sha256: png_sha256.to_string(),
        },
        &body,
    )
    .map_err(|_| WorkerError::Protocol)?;
    writer.flush().map_err(|_| WorkerError::Protocol)
}

struct LimitedBuffer {
    bytes: Vec<u8>,
    limit: usize,
}

impl LimitedBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(64 * 1024)),
            limit,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for LimitedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "response limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private_path(parts: &[&str]) -> PathBuf {
        #[cfg(windows)]
        let mut path = PathBuf::from("C:\\");
        #[cfg(not(windows))]
        let mut path = PathBuf::from("/");
        for part in parts {
            path.push(part);
        }
        path
    }

    fn filesystem_root() -> PathBuf {
        #[cfg(windows)]
        {
            PathBuf::from("C:\\")
        }
        #[cfg(not(windows))]
        {
            PathBuf::from("/")
        }
    }

    #[test]
    fn runtime_root_validation_uses_canonical_paths_and_rejects_symlink_escape() {
        let root_link = private_path(&["install", "member", "runtime-link"]);
        let library_link = root_link.join("onnxruntime.so");
        let canonical_root = private_path(&["install", "member", "runtime"]);
        let canonical_library = canonical_root.join("onnxruntime.so");
        let resolved = canonical_private_runtime_paths(&root_link, &library_link, |path| {
            if path == root_link {
                Ok(canonical_root.clone())
            } else if path == library_link {
                Ok(canonical_library.clone())
            } else {
                Err(())
            }
        })
        .unwrap();
        assert_eq!(
            resolved,
            (canonical_root.clone(), canonical_library.clone())
        );

        let outside_library = private_path(&["outside", "onnxruntime.so"]);
        let escaped = canonical_private_runtime_paths(&root_link, &library_link, |path| {
            if path == root_link {
                Ok(canonical_root.clone())
            } else if path == library_link {
                Ok(outside_library.clone())
            } else {
                Err(())
            }
        });
        assert_eq!(escaped, Err(WorkerError::Startup));

        let sibling_root = private_path(&["install", "member", "runtime"]);
        let sibling_library =
            private_path(&["install", "member", "runtime-other", "onnxruntime.so"]);
        let sibling = canonical_private_runtime_paths(&sibling_root, &sibling_library, |path| {
            Ok(path.to_path_buf())
        });
        assert_eq!(sibling, Err(WorkerError::Startup));

        let root_as_library =
            canonical_private_runtime_paths(&canonical_root, &canonical_root, |path| {
                Ok(path.to_path_buf())
            });
        assert_eq!(root_as_library, Err(WorkerError::Startup));
    }

    #[test]
    fn runtime_root_validation_rejects_empty_and_filesystem_root_inputs() {
        assert_eq!(
            canonical_private_runtime_paths(Path::new(""), Path::new("/runtime/lib.so"), |_| {
                panic!("empty paths are rejected before canonicalization")
            }),
            Err(WorkerError::Startup)
        );
        assert_eq!(
            canonical_private_runtime_paths(Path::new("runtime"), Path::new(""), |_| {
                panic!("empty paths are rejected before canonicalization")
            }),
            Err(WorkerError::Startup)
        );
        assert_eq!(
            canonical_private_runtime_paths(
                &filesystem_root(),
                &private_path(&["runtime", "lib.so"]),
                |path| Ok(path.to_path_buf())
            ),
            Err(WorkerError::Startup)
        );
    }

    #[test]
    fn runtime_commit_fails_closed_without_ort_initialization() {
        assert_eq!(require_runtime_commit(true), Ok(()));
        assert_eq!(require_runtime_commit(false), Err(WorkerError::Startup));
        assert!(!Recipe::load().unwrap().settings.telemetry);
    }
}
