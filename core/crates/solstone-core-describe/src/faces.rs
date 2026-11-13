// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local, provider-free face extraction. Hidden segment files are private inputs
//! for faceprints; they are never appended to the screen transcript.

mod inference;
mod labels;
use crate::{OcrWorkerFactory, RawOccurrence, screen_ocr};
use image::{RgbImage, codecs::jpeg::JpegEncoder};
pub use inference::{FaceModels, align};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_segment::output_attempt::{InputObservation, OutputClaims, SegmentCommit};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

pub type PassResult<T> = Result<T, String>;
pub const SCHEMA_VERSION: u32 = 1;
pub const WIDTH: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FaceDetection {
    #[serde(rename = "box")]
    pub bbox: [f32; 4],
    pub landmarks: [f32; 10],
    pub score: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FaceLabel {
    pub text: String,
    #[serde(rename = "box")]
    pub bbox: [f32; 4],
    pub participant_idx: usize,
    pub participant_name: String,
    #[serde(rename = "match")]
    pub match_score: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FaceRow {
    pub frame_id: u64,
    pub timestamp: f64,
    pub face_idx: usize,
    #[serde(rename = "box")]
    pub bbox: [f32; 4],
    pub frame_w: u32,
    pub frame_h: u32,
    pub det_score: f32,
    pub landmarks: [f32; 10],
    pub sharpness: f32,
    pub quality: f32,
    pub category: String,
    pub label: Option<FaceLabel>,
    pub in_box: Option<usize>,
    pub crop: Option<String>,
    pub emb_row: usize,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct PassStats {
    pub frames: usize,
    pub faces: usize,
    pub anchored: usize,
    pub in_box: usize,
    pub crops: usize,
}

pub fn enabled(journal: &Path) -> PassResult<bool> {
    let config =
        solstone_core_journal_config::read_journal_config(journal).map_err(|e| e.to_string())?;
    Ok(config
        .config
        .as_ref()
        .and_then(|v| v.get("faces"))
        .and_then(|v| v.get("enabled"))
        .and_then(Value::as_bool)
        == Some(true))
}

/// File names are source-specific because one segment can hold several screens.
pub fn sidecar_path(video: &Path) -> PathBuf {
    video.with_file_name(format!("{}.jsonl", prefix(video)))
}
fn embeddings_path(video: &Path, bytes: &[u8]) -> PathBuf {
    video.with_file_name(format!("{}-{:x}.npy", prefix(video), Sha256::digest(bytes)))
}
fn prefix(video: &Path) -> String {
    let hash = format!(
        "{:x}",
        Sha256::digest(video.file_name().unwrap_or_default().as_encoded_bytes())
    );
    format!(".faces-v1-{}", &hash[..16])
}
fn crop_reference(prefix: &str, frame: u64, face: usize, bytes: &[u8]) -> String {
    format!(
        "{prefix}-crops/{frame}-{face}-{:x}.jpg",
        Sha256::digest(bytes)
    )
}
/// Bind face evidence to the frame rows, excluding the processing header.
/// Readers hold the screen output claim and compare this with the face header
/// before consuming the sidecar; a mismatch requires rebuilding that evidence.
pub fn screen_body_digest(screen: &Path) -> io::Result<String> {
    let bytes = fs::read(screen)?;
    // Processing/retry metadata lives on the first line and may change while
    // describe deliberately retains the previous frame reading unchanged.
    let body = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(&[][..], |index| &bytes[index + 1..]);
    Ok(format!("{:x}", Sha256::digest(body)))
}

pub struct FacePass {
    models: Option<FaceModels>,
    rows: Vec<FaceRow>,
    embeddings: Vec<[f32; 128]>,
    crops: Vec<(String, Vec<u8>)>,
    prefix: String,
    pub stats: PassStats,
}
impl FacePass {
    pub fn new(video: &Path) -> Self {
        Self {
            models: None,
            rows: Vec::new(),
            embeddings: Vec::new(),
            crops: Vec::new(),
            prefix: prefix(video),
            stats: PassStats::default(),
        }
    }

    /// Consume the native PNG that describe already holds for meeting extraction.
    pub(crate) fn add_frame(
        &mut self,
        row: &Value,
        png: &[u8],
        cached: Option<&screen_ocr::PrivateSidecar>,
        ocr: &mut dyn OcrWorkerFactory,
    ) -> PassResult<()> {
        let Some(parts) = row
            .pointer("/content/meeting/participants")
            .and_then(Value::as_array)
        else {
            return Ok(());
        };
        if parts.is_empty() {
            return Ok(());
        }
        let frame_id = row
            .get("frame_id")
            .and_then(Value::as_u64)
            .ok_or("meeting row omitted frame_id")?;
        let timestamp = row
            .get("timestamp")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
            .ok_or("meeting row omitted timestamp")?;
        let image = image::load_from_memory(png)
            .map_err(|e| e.to_string())?
            .to_rgb8();
        if self.models.is_none() {
            self.models = Some(FaceModels::open()?);
        }
        let models = self.models.as_mut().expect("initialized");
        let faces = models.detect(&image)?;
        self.stats.frames += 1;
        let texts = if let Some(frame) = cached.and_then(|s| {
            s.frames.iter().find(|f| {
                f.frame_id == frame_id
                    && f.state == screen_ocr::CoverageState::Success
                    && f.png_sha256 == screen_ocr::png_sha256(png)
            })
        }) {
            frame.occurrences.clone()
        } else {
            meeting_ocr(&image, parts, ocr)?
        };
        let labels = labels::assign(&faces, parts, &texts);
        let boxes: Vec<Option<usize>> = faces
            .iter()
            .map(|f| labels::in_box(f, parts, image.width(), image.height()))
            .collect();
        for (face_idx, face) in faces.iter().enumerate() {
            let (emb, sharpness) = models.embed(&image, face)?;
            let label = labels.get(&face_idx).cloned();
            let in_box = boxes[face_idx];
            let sole = in_box.is_some_and(|p| boxes.iter().filter(|v| **v == Some(p)).count() == 1);
            let crop = if label.is_some() || sole {
                let bytes = crop_jpeg(&image, face.bbox)?;
                let name = crop_reference(&self.prefix, frame_id, face_idx, &bytes);
                self.crops.push((name.clone(), bytes));
                Some(name)
            } else {
                None
            };
            self.stats.faces += 1;
            self.stats.anchored += usize::from(label.is_some());
            self.stats.in_box += usize::from(in_box.is_some());
            self.stats.crops += usize::from(crop.is_some());
            self.rows.push(FaceRow {
                frame_id,
                timestamp,
                face_idx,
                bbox: face.bbox,
                frame_w: image.width(),
                frame_h: image.height(),
                det_score: face.score,
                landmarks: face.landmarks,
                sharpness,
                quality: face.score
                    * (face.bbox[2] / 110.).min(1.)
                    * (sharpness / 400.).min(1.).sqrt(),
                category: row
                    .pointer("/analysis/primary")
                    .and_then(Value::as_str)
                    .unwrap_or("meeting")
                    .to_owned(),
                label,
                in_box,
                crop,
                emb_row: self.embeddings.len(),
            });
            self.embeddings.push(emb);
        }
        Ok(())
    }

    pub(crate) fn prepare(
        &self,
        video: &Path,
        stop: &dyn Fn() -> bool,
    ) -> io::Result<PreparedFaces> {
        let segment = video
            .parent()
            .ok_or_else(|| io::Error::other("segment missing"))?;
        let mut files = Vec::new();
        let mut npy_bytes = Vec::new();
        write_npy(&mut npy_bytes, &self.embeddings)?;
        let npy = embeddings_path(video, &npy_bytes);
        files.push(crate::publication::StagedFile::prepare(&npy, stop, |w| {
            w.write_all(&npy_bytes)
        })?);
        let current = self
            .crops
            .iter()
            .map(|(name, _)| name.rsplit('/').next().unwrap().to_owned())
            .collect::<BTreeSet<_>>();
        let path = sidecar_path(video);
        let header = json!({"schema_version":SCHEMA_VERSION,"detector":{"id":"opencv-yunet-2023mar","sha256":solstone_core_transcribe::YUNET_SHA256},"embedder":{"id":"opencv-sface-2021dec","sha256":solstone_core_transcribe::SFACE_SHA256,"width":WIDTH},"source_name":video.file_name().and_then(|s|s.to_str()),"embeddings":npy.file_name().and_then(|s|s.to_str())});
        // The manifest is staged at install, after describe has published the
        // exact screen bytes that it must bind. Payloads are prepared here.
        let directory = segment.join(format!("{}-crops", self.prefix));
        let mut stale = Vec::new();
        match fs::read_dir(&directory) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".jpg") && !current.contains(&name) {
                        stale.push(name);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut stale_embeddings = Vec::new();
        for entry in fs::read_dir(segment)? {
            let name = entry?.file_name();
            if let Some(name) = name.to_str()
                && name.starts_with(&format!("{}-", self.prefix))
                && name.ends_with(".npy")
                && npy.file_name() != Some(std::ffi::OsStr::new(name))
            {
                stale_embeddings.push(name.to_owned());
            }
        }
        Ok(PreparedFaces {
            files,
            manifest: path,
            screen: video.with_extension("jsonl"),
            header,
            rows: self.rows.clone(),
            segment: segment.to_owned(),
            directory,
            crops: self.crops.clone(),
            stale,
            stale_embeddings,
        })
    }
}

pub(crate) struct PreparedFaces {
    files: Vec<crate::publication::StagedFile>,
    manifest: PathBuf,
    screen: PathBuf,
    header: Value,
    rows: Vec<FaceRow>,
    segment: PathBuf,
    directory: PathBuf,
    crops: Vec<(String, Vec<u8>)>,
    stale: Vec<String>,
    stale_embeddings: Vec<String>,
}
impl PreparedFaces {
    pub(crate) fn install(&self, commit: &SegmentCommit<'_>) -> io::Result<()> {
        commit.revalidate().map_err(io::Error::other)?;
        for file in &self.files {
            file.validate_stage()?;
        }
        let mut header = self.header.clone();
        header["screen_body_sha256"] = Value::String(screen_body_digest(&self.screen)?);
        let manifest = crate::publication::StagedFile::prepare(&self.manifest, &|| false, |w| {
            writeln!(w, "{header}")?;
            for row in &self.rows {
                serde_json::to_writer(&mut *w, row)?;
                writeln!(w)?;
            }
            Ok(())
        })?;
        if !self.crops.is_empty() {
            solstone_core_journal_io::create_directory_with_mode(&self.directory, 0o700)
                .map_err(io::Error::other)?;
            for (name, bytes) in &self.crops {
                let staged = crate::publication::StagedFile::prepare(
                    &self.segment.join(name),
                    &|| false,
                    |w| w.write_all(bytes),
                )?;
                staged.install()?;
            }
        }
        for file in &self.files {
            file.install()?;
        }
        // Immutable payloads land first. The screen digest makes an older
        // manifest detectably stale if a crash followed screen publication.
        manifest.install()?;
        for name in &self.stale {
            #[expect(
                clippy::disallowed_methods,
                reason = "system internal: retire superseded private derived face crops under the segment commit"
            )]
            solstone_core_journal_io::remove_file(&self.directory, name)
                .map_err(io::Error::other)?;
        }
        for name in &self.stale_embeddings {
            #[expect(
                clippy::disallowed_methods,
                reason = "system internal: retire superseded immutable face embeddings after publishing the new header"
            )]
            solstone_core_journal_io::remove_file(&self.segment, name).map_err(io::Error::other)?;
        }
        Ok(())
    }
}

/// Backfill seam: raw recording plus its existing screen JSONL. No providers.
/// The caller must have opted in; the strict check is repeated at publication.
pub fn run_segment(
    journal: &Path,
    video: &Path,
    screen: &Path,
    stop: &dyn Fn() -> bool,
) -> PassResult<PassStats> {
    let resolver = screen_ocr::InstalledHelperResolver;
    let mut ocr = screen_ocr::ResolvedWorkerFactory::new(&resolver);
    run_segment_with_ocr(journal, video, screen, &mut ocr, stop)
}

/// Developer entry using an explicitly selected, verified installed payload's OCR helper.
/// Source builds can use this without rebuilding an unrelated complete payload.
pub fn run_segment_using_payload(
    journal: &Path,
    video: &Path,
    screen: &Path,
    executable: &Path,
    stop: &dyn Fn() -> bool,
) -> PassResult<PassStats> {
    let resolver = screen_ocr::InstalledHelperResolver;
    let mut ocr = screen_ocr::ResolvedWorkerFactory::from_executable(&resolver, executable)?;
    run_segment_with_ocr(journal, video, screen, &mut ocr, stop)
}

fn run_segment_with_ocr(
    journal: &Path,
    video: &Path,
    screen: &Path,
    ocr: &mut dyn OcrWorkerFactory,
    stop: &dyn Fn() -> bool,
) -> PassResult<PassStats> {
    if !enabled(journal)? {
        return Ok(PassStats::default());
    }
    if video.with_extension("jsonl") != screen {
        return Err("screen JSONL must be the recording's sibling".into());
    }
    let claims = OutputClaims::acquire([screen.to_owned()], Default::default())
        .map_err(|e| e.to_string())?;
    let input =
        InputObservation::capture_checked(&claims, video, stop).map_err(|e| e.to_string())?;
    let screen_input =
        InputObservation::capture_checked(&claims, screen, stop).map_err(|e| e.to_string())?;
    let mut wanted = BTreeMap::new();
    for line in BufReader::new(fs::File::open(screen).map_err(|e| e.to_string())?).lines() {
        let row: Value =
            serde_json::from_str(&line.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if row
            .pointer("/content/meeting/participants")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
        {
            let id = row
                .get("frame_id")
                .and_then(Value::as_u64)
                .ok_or("meeting row omitted frame_id")?;
            wanted.insert(id, row);
        }
    }
    let segment = video.parent().ok_or("segment missing")?;
    let cached = screen_ocr::load_segment_sidecars(segment)
        .into_iter()
        .find(|s| s.source_name == video.file_name().unwrap_or_default().to_string_lossy());
    let mut pass = FacePass::new(video);
    if !wanted.is_empty() {
        let config = crate::WinnowConfig {
            dhash_threshold: 0,
            scene_cut_threshold: 0,
            min_stride_seconds: 0.,
        };
        let mut pull = crate::decode::FramePull::open(video, config, true)
            .map_err(|_| "could not decode recording")?;
        pull.retain_pngs(wanted.keys().copied().collect());
        loop {
            match pull.next(stop) {
                crate::decode::PullNext::Frame(frame) => {
                    let row = wanted
                        .remove(&frame.frame_id)
                        .ok_or("unexpected decoded frame")?;
                    let ts = row
                        .get("timestamp")
                        .and_then(Value::as_f64)
                        .ok_or("meeting timestamp missing")?;
                    if (ts - frame.timestamp).abs() > 0.002 {
                        return Err("meeting frame timestamp differs from raw recording".into());
                    }
                    pass.add_frame(&row, &frame.png, cached.as_ref(), ocr)?;
                }
                crate::decode::PullNext::Done => break,
                crate::decode::PullNext::Cancelled => return Err("face pass cancelled".into()),
            }
        }
        if !wanted.is_empty() {
            return Err("meeting frames missing from raw recording".into());
        }
    }
    let prepared = pass.prepare(video, stop).map_err(|e| e.to_string())?;
    drop(
        screen_input
            .commit_checked(&claims, stop)
            .map_err(|e| e.to_string())?,
    );
    let commit = input
        .commit_checked(&claims, stop)
        .map_err(|e| e.to_string())?;
    if !enabled(journal)? {
        return Err("faces disabled during face pass".into());
    }
    prepared.install(&commit).map_err(|e| e.to_string())?;
    Ok(pass.stats)
}

fn meeting_ocr(
    image: &RgbImage,
    parts: &[Value],
    factory: &mut dyn OcrWorkerFactory,
) -> PassResult<Vec<RawOccurrence>> {
    let [x, y, w, h] = labels::meeting_region(parts, image.width(), image.height());
    let region = image::imageops::crop_imm(image, x, y, w, h).to_image();
    let scale = if w < 1400 {
        (1400. / w as f32).min(2.)
    } else {
        1.
    };
    let region = if scale > 1. {
        inference::resize_cubic(&region, scale)
    } else {
        region
    };
    let mut bytes = std::io::Cursor::new(Vec::new());
    region
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let mut worker = factory
        .create()
        .map_err(|e| format!("meeting OCR: {}", e.code()))?
        .ok_or("meeting OCR helper unavailable")?;
    let mut texts = worker
        .recognize(bytes.get_ref())
        .map_err(|e| format!("meeting OCR: {}", e.code()))?;
    for text in &mut texts {
        for point in &mut text.geometry {
            point[0] = point[0] / scale + x as f32;
            point[1] = point[1] / scale + y as f32;
        }
    }
    Ok(texts)
}
fn crop_jpeg(image: &RgbImage, b: [f32; 4]) -> PassResult<Vec<u8>> {
    let x = (b[0] - b[2] * 0.45).max(0.) as u32;
    let y = (b[1] - b[3] * 0.45).max(0.) as u32;
    let x1 = (b[0] + b[2] * 1.45).clamp(0., image.width() as f32) as u32;
    let y1 = (b[1] + b[3] * 1.45).clamp(0., image.height() as f32) as u32;
    if x >= x1 || y >= y1 {
        return Err("face crop is outside frame".into());
    }
    let crop = image::imageops::crop_imm(image, x, y, x1 - x, y1 - y).to_image();
    let crop = image::imageops::resize(
        &crop,
        256,
        (256 * (y1 - y) / (x1 - x)).max(1),
        image::imageops::FilterType::CatmullRom,
    );
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 92)
        .encode_image(&crop)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}
fn write_npy(w: &mut dyn Write, rows: &[[f32; 128]]) -> io::Result<()> {
    let mut header = format!(
        "{{'descr': '<f4', 'fortran_order': False, 'shape': ({}, 128), }}",
        rows.len()
    );
    let pad = (64 - (10 + header.len() + 1) % 64) % 64;
    header.push_str(&" ".repeat(pad));
    header.push('\n');
    w.write_all(b"\x93NUMPY\x01\x00")?;
    w.write_all(&(header.len() as u16).to_le_bytes())?;
    w.write_all(header.as_bytes())?;
    for row in rows {
        for value in row {
            w.write_all(&value.to_le_bytes())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeddings_are_row_aligned_little_endian_numpy() {
        let rows = [[0.25; 128], [-0.5; 128]];
        let mut bytes = Vec::new();
        write_npy(&mut bytes, &rows).unwrap();
        assert_eq!(&bytes[..8], b"\x93NUMPY\x01\x00");
        let length = u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as usize;
        let header = std::str::from_utf8(&bytes[10..10 + length]).unwrap();
        assert!(header.contains("'shape': (2, 128)"));
        let values = bytes[10 + length..]
            .chunks_exact(4)
            .map(|v| f32::from_le_bytes(v.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(values, rows.into_iter().flatten().collect::<Vec<_>>());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn interrupted_republication_keeps_previous_payloads_readable() {
        let root = tempfile::tempdir().unwrap();
        let raw = root.path().join("screen.webm");
        let screen = raw.with_extension("jsonl");
        fs::write(&raw, b"synthetic raw observation").unwrap();
        fs::write(&screen, b"header\noriginal screen body\n").unwrap();
        let claims = OutputClaims::acquire([screen], Default::default()).unwrap();
        let input = InputObservation::capture_checked(&claims, &raw, &|| false).unwrap();
        let commit = input.commit_checked(&claims, &|| false).unwrap();
        let make_pass = |value, bytes: Vec<u8>| {
            let mut pass = FacePass::new(&raw);
            pass.embeddings.push([value; 128]);
            let name = crop_reference(&pass.prefix, 1, 0, &bytes);
            pass.crops.push((name, bytes));
            pass
        };
        let old = make_pass(0.25, vec![1, 2, 3]);
        old.prepare(&raw, &|| false)
            .unwrap()
            .install(&commit)
            .unwrap();
        let header_bytes = fs::read(sidecar_path(&raw)).unwrap();
        let header: Value = serde_json::from_slice(&header_bytes).unwrap();
        let old_npy = root.path().join(header["embeddings"].as_str().unwrap());
        let old_embeddings = fs::read(&old_npy).unwrap();
        let old_crop = root.path().join(&old.crops[0].0);

        fs::write(
            raw.with_extension("jsonl"),
            b"retry header\noriginal screen body\n",
        )
        .unwrap();
        assert_eq!(
            header["screen_body_sha256"],
            screen_body_digest(&raw.with_extension("jsonl")).unwrap()
        );

        // A crash after describe publishes a replacement screen must also be
        // detectable before any replacement face manifest becomes visible.
        let replacement_screen = b"replacement header\nreplacement screen body\n";
        fs::write(raw.with_extension("jsonl"), replacement_screen).unwrap();
        let replacement_digest = screen_body_digest(&raw.with_extension("jsonl")).unwrap();
        assert_ne!(
            header["screen_body_sha256"].as_str(),
            Some(replacement_digest.as_str())
        );

        let new = make_pass(0.5, vec![4, 5, 6]);
        let staged = new.prepare(&raw, &|| false).unwrap();
        // Simulate a crash after the new payloads land but before the header.
        let crop = &new.crops[0];
        crate::publication::StagedFile::prepare(&root.path().join(&crop.0), &|| false, |w| {
            w.write_all(&crop.1)
        })
        .unwrap()
        .install()
        .unwrap();
        staged.files[0].install().unwrap();
        assert_eq!(fs::read(sidecar_path(&raw)).unwrap(), header_bytes);
        assert_eq!(fs::read(&old_npy).unwrap(), old_embeddings);
        assert_eq!(fs::read(&old_crop).unwrap(), old.crops[0].1);
        drop(staged);

        new.prepare(&raw, &|| false)
            .unwrap()
            .install(&commit)
            .unwrap();
        let header: Value = serde_json::from_slice(&fs::read(sidecar_path(&raw)).unwrap()).unwrap();
        assert_eq!(header["screen_body_sha256"], replacement_digest);
        assert!(!old_npy.exists());
        assert!(!old_crop.exists());
        assert_eq!(
            fs::read(root.path().join(&new.crops[0].0)).unwrap(),
            new.crops[0].1
        );
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn only_explicit_true_enables_faces_and_disabled_pass_preserves_screen_bytes() {
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir(journal.path().join("config")).unwrap();
        let config = journal.path().join("config/journal.json");
        let raw = journal.path().join("missing_screen.webm");
        let screen = raw.with_extension("jsonl");
        let original = b"independent existing transcript bytes\n";
        fs::write(&screen, original).unwrap();
        for value in [
            json!({}),
            json!({"faces":{}}),
            json!({"faces":{"enabled":false}}),
            json!({"faces":{"enabled":"true"}}),
            json!({"faces":{"enabled":1}}),
        ] {
            fs::write(&config, value.to_string()).unwrap();
            assert!(!enabled(journal.path()).unwrap());
            assert_eq!(
                run_segment(journal.path(), &raw, &screen, &|| false)
                    .unwrap()
                    .frames,
                0
            );
            assert_eq!(fs::read(&screen).unwrap(), original);
            assert!(!sidecar_path(&raw).exists());
        }
        fs::write(&config, br#"{"faces":{"enabled":true}}"#).unwrap();
        assert!(enabled(journal.path()).unwrap());
    }
}
