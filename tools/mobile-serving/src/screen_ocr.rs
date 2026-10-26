// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-process screen text for mobile describe.
//!
//! The `OAROCR` context is created on the first `recognize` inside a job.
//! A missing or rejected model file never builds a session.

use std::cell::Cell;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use sha2::{Digest, Sha256};
use solstone_core_describe::{OcrFailure, OcrWorker, OcrWorkerFactory, RawOccurrence};

use oar_ocr::core::config::{OrtExecutionProvider, OrtSessionConfig};
use oar_ocr::domain::tasks::{TextDetectionConfig, TextRecognitionConfig};
use oar_ocr::oarocr::{OAROCR, OAROCRBuilder};
use oar_ocr::processors::LimitType;

pub(crate) const PINNED_MODELS_DETAIL: &str = "pinned PP-OCRv6 models were not admitted (detector 193bab7a, recognizer 9ef676d6, dictionary c5cbe34e)";

const ENGINE_FAILED_DETAIL: &str = "screen-ocr engine failed after the pinned models were admitted";

const RECIPE_ID: &str = "mobile-screen-ocr";

const DETECTOR_NAME: &str = "detector.onnx";
const RECOGNIZER_NAME: &str = "recognizer.onnx";
const DICTIONARY_NAME: &str = "dictionary.txt";
const DETECTOR_SIZE: u64 = 1_780_590;
const RECOGNIZER_SIZE: u64 = 4_462_639;
const DICTIONARY_SIZE: u64 = 27_156;
const DETECTOR_SHA256: &str = "193bab7a04fca699a6c82e6abb5b81bdb28177f0abd4062552b04908dafb19f8";
const RECOGNIZER_SHA256: &str = "9ef676d6ed3c88256a2d92c640c44f25b0c40947e111b14b8be8f594091563e6";
const DICTIONARY_SHA256: &str = "c5cbe34ef40c29c4df07ed012bf96569cb69a2d2a01a07027e9f13cb832bd9cd";

struct PinnedFile {
    name: &'static str,
    size: u64,
    sha256: &'static str,
}

const PINNED_FILES: [PinnedFile; 3] = [
    PinnedFile {
        name: DETECTOR_NAME,
        size: DETECTOR_SIZE,
        sha256: DETECTOR_SHA256,
    },
    PinnedFile {
        name: RECOGNIZER_NAME,
        size: RECOGNIZER_SIZE,
        sha256: RECOGNIZER_SHA256,
    },
    PinnedFile {
        name: DICTIONARY_NAME,
        size: DICTIONARY_SIZE,
        sha256: DICTIONARY_SHA256,
    },
];

pub(crate) struct ScreenOcrFactory {
    payload: PathBuf,
    /// Read after the worker returns. Admission leaves the pinned-models sentence;
    /// a later engine failure replaces it before `OcrFailure::Unavailable`.
    detail: Rc<Cell<&'static str>>,
}

impl ScreenOcrFactory {
    pub(crate) fn new(payload: &Path) -> Self {
        Self {
            payload: payload.to_path_buf(),
            detail: Rc::new(Cell::new(PINNED_MODELS_DETAIL)),
        }
    }
}

impl OcrWorkerFactory for ScreenOcrFactory {
    fn recipe_id(&mut self) -> String {
        RECIPE_ID.to_owned()
    }

    fn create(&mut self) -> Result<Option<Box<dyn OcrWorker>>, OcrFailure> {
        let Some(models) = admit_models(&model_directory(&self.payload)) else {
            self.detail.set(PINNED_MODELS_DETAIL);
            return Ok(None);
        };
        Ok(Some(Box::new(ScreenOcrWorker {
            models,
            engine: None,
            pool: rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .map_err(|_| OcrFailure::Unavailable)?,
            detail: Rc::clone(&self.detail),
        })))
    }

    fn unavailable_detail(&mut self) -> Option<&'static str> {
        Some(self.detail.get())
    }
}

struct AdmittedModels {
    detector: PathBuf,
    recognizer: PathBuf,
    dictionary: PathBuf,
}

struct ScreenOcrWorker {
    models: AdmittedModels,
    engine: Option<OAROCR>,
    pool: rayon::ThreadPool,
    detail: Rc<Cell<&'static str>>,
}

impl ScreenOcrWorker {
    fn engine_unavailable(&self) -> OcrFailure {
        self.detail.set(ENGINE_FAILED_DETAIL);
        OcrFailure::Unavailable
    }
}

impl OcrWorker for ScreenOcrWorker {
    fn recognize(&mut self, png: &[u8]) -> Result<Vec<RawOccurrence>, OcrFailure> {
        if self.engine.is_none() {
            // A false commit means a runtime was already committed in this process.
            let _committed = ort::init().with_telemetry(false).commit();
            let engine = match build_engine(&self.models) {
                Ok(engine) => engine,
                Err(_) => return Err(self.engine_unavailable()),
            };
            self.engine = Some(engine);
        }
        let Some(engine) = self.engine.as_ref() else {
            return Err(self.engine_unavailable());
        };
        let image = image::ImageReader::with_format(Cursor::new(png), image::ImageFormat::Png)
            .decode()
            .map_err(|_| self.engine_unavailable())?
            .to_rgb8();
        let predicted = self
            .pool
            .install(|| engine.predict(vec![image]))
            .map_err(|_| self.engine_unavailable())?;
        let Some(result) = predicted.first() else {
            return Err(self.engine_unavailable());
        };
        let mut occurrences = Vec::new();
        for region in &result.text_regions {
            let Some(text) = region.text.as_deref() else {
                continue;
            };
            let line_order =
                u32::try_from(occurrences.len()).map_err(|_| self.engine_unavailable())?;
            occurrences.push(RawOccurrence {
                line_order,
                text: text.to_owned(),
                confidence: region.confidence,
                geometry: region
                    .bounding_box
                    .points
                    .iter()
                    .map(|point| [point.x, point.y])
                    .collect(),
                recurrence: 0,
            });
        }
        Ok(occurrences)
    }
}

fn model_directory(payload: &Path) -> PathBuf {
    match std::env::var_os("SOLSTONE_SCREEN_OCR_MODELS") {
        Some(path) => PathBuf::from(path),
        None => payload.join("lib/solstone-screen-ocr/models"),
    }
}

fn admit_models(directory: &Path) -> Option<AdmittedModels> {
    for file in PINNED_FILES {
        let path = directory.join(file.name);
        let metadata = std::fs::metadata(&path).ok()?;
        if !metadata.is_file() || metadata.len() != file.size {
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if digest != file.sha256 {
            return None;
        }
    }
    Some(AdmittedModels {
        detector: directory.join(DETECTOR_NAME),
        recognizer: directory.join(RECOGNIZER_NAME),
        dictionary: directory.join(DICTIONARY_NAME),
    })
}

fn build_engine(models: &AdmittedModels) -> Result<OAROCR, OcrFailure> {
    let detection = TextDetectionConfig {
        score_threshold: 0.2,
        box_threshold: 0.4,
        unclip_ratio: 1.4,
        max_candidates: 3000,
        limit_side_len: Some(1920),
        limit_type: Some(LimitType::Max),
        max_side_len: Some(4096),
    };
    let recognition = TextRecognitionConfig {
        score_threshold: 0.0,
    };
    let ort_config = OrtSessionConfig::new()
        .with_execution_providers(vec![OrtExecutionProvider::CPU])
        .with_intra_threads(1)
        .with_inter_threads(1);
    OAROCRBuilder::new(
        models.detector.as_path(),
        models.recognizer.as_path(),
        models.dictionary.as_path(),
    )
    .text_detection_config(detection)
    .text_recognition_config(recognition)
    .ort_session(ort_config)
    .image_batch_size(1)
    .region_batch_size(1)
    .build()
    .map_err(|_| OcrFailure::Unavailable)
}

#[cfg(all(test, feature = "full-tests", feature = "screen-ocr-native"))]
mod tests {
    use super::*;

    #[test]
    fn known_text_and_blank_frame() {
        let _models =
            std::env::var_os("SOLSTONE_SCREEN_OCR_MODELS").expect("SOLSTONE_SCREEN_OCR_MODELS");
        let text_png =
            std::env::var_os("SOLSTONE_SCREEN_OCR_TEXT_PNG").expect("SOLSTONE_SCREEN_OCR_TEXT_PNG");
        let blank_png = std::env::var_os("SOLSTONE_SCREEN_OCR_BLANK_PNG")
            .expect("SOLSTONE_SCREEN_OCR_BLANK_PNG");
        let mut factory = ScreenOcrFactory::new(Path::new("payload"));
        let mut worker = factory
            .create()
            .expect("create")
            .expect("pinned models were not admitted");
        let text = std::fs::read(&text_png).expect("text png");
        let regions = worker.recognize(&text).expect("text recognition");
        let texts: Vec<&str> = regions.iter().map(|region| region.text.as_str()).collect();
        assert_eq!(texts, ["SYNTHETIC MEDIA FRAME 0OO", "alpha beta"]);
        let blank = std::fs::read(&blank_png).expect("blank png");
        let regions = worker.recognize(&blank).expect("blank recognition");
        assert!(regions.is_empty());
    }
}
