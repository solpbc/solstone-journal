// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reader for transcript embedding sidecars written by this crate.

use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use zip::ZipArchive;

use solstone_core_npy::{NpyBlob, parse_npy};

/// Embeddings paired with their statement IDs and durations in on-disk order.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingsFile {
    pub statements: Vec<(i64, Vec<f32>)>,
    pub durations_s: Vec<f64>,
}

/// Failure while reading a present embeddings sidecar.
#[derive(Debug)]
pub enum EmbeddingsError {
    Io { path: PathBuf, detail: String },
    Archive(String),
    Invalid(String),
}

impl fmt::Display for EmbeddingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, detail } => write!(formatter, "{}: {detail}", path.display()),
            Self::Archive(detail) | Self::Invalid(detail) => formatter.write_str(detail),
        }
    }
}

impl Error for EmbeddingsError {}

/// Load one embeddings sidecar, treating an absent file or required member as absent data.
pub fn load_embeddings_file(path: &Path) -> Result<Option<EmbeddingsFile>, EmbeddingsError> {
    if !path.exists() {
        return Ok(None);
    }
    let file = File::open(path).map_err(|error| EmbeddingsError::Io {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| EmbeddingsError::Archive(error.to_string()))?;
    let Some(embedding_bytes) = optional_member(&mut archive, "embeddings.npy")? else {
        return Ok(None);
    };
    let Some(statement_id_bytes) = optional_member(&mut archive, "statement_ids.npy")? else {
        return Ok(None);
    };
    let embeddings =
        parse_npy(&embedding_bytes).map_err(|error| EmbeddingsError::Invalid(error.to_string()))?;
    let statement_ids = parse_npy(&statement_id_bytes)
        .map_err(|error| EmbeddingsError::Invalid(error.to_string()))?;
    let rows = f32_rows(&embeddings)?;
    let ids = i32_vector(&statement_ids)?;
    if rows.len() != ids.len() {
        return Err(EmbeddingsError::Invalid(
            "embeddings and statement_ids row counts differ".to_owned(),
        ));
    }
    let statement_count = rows.len();
    let durations_s =
        if let Some(duration_bytes) = optional_member(&mut archive, "durations_s.npy")? {
            if let Ok(blob) = parse_npy(&duration_bytes) {
                if blob.descr == "<f4" && !blob.fortran_order && blob.shape.len() == 1 {
                    let parsed = blob
                        .payload
                        .chunks_exact(4)
                        .map(|bytes| {
                            f32::from_le_bytes(bytes.try_into().expect("four-byte chunk")) as f64
                        })
                        .collect::<Vec<_>>();
                    let mut durations = Vec::with_capacity(statement_count);
                    for i in 0..statement_count {
                        durations.push(parsed.get(i).copied().unwrap_or(0.0));
                    }
                    durations
                } else {
                    vec![0.0; statement_count]
                }
            } else {
                vec![0.0; statement_count]
            }
        } else {
            vec![0.0; statement_count]
        };
    Ok(Some(EmbeddingsFile {
        statements: ids
            .into_iter()
            .zip(rows)
            .map(|(id, embedding)| (i64::from(id), embedding))
            .collect(),
        durations_s,
    }))
}

fn optional_member(
    archive: &mut ZipArchive<File>,
    name: &str,
) -> Result<Option<Vec<u8>>, EmbeddingsError> {
    match archive.by_name(name) {
        Ok(mut member) => {
            let mut bytes = Vec::new();
            member
                .read_to_end(&mut bytes)
                .map_err(|error| EmbeddingsError::Archive(error.to_string()))?;
            Ok(Some(bytes))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(error) => Err(EmbeddingsError::Archive(error.to_string())),
    }
}

fn f32_rows(blob: &NpyBlob<'_>) -> Result<Vec<Vec<f32>>, EmbeddingsError> {
    if blob.descr != "<f4" || blob.fortran_order || blob.shape.len() != 2 || blob.shape[1] != 256 {
        return Err(EmbeddingsError::Invalid(
            "embeddings.npy must be a C-order (rows, 256) <f4 array".to_owned(),
        ));
    }
    Ok(blob
        .payload
        .chunks_exact(4 * 256)
        .map(|row| {
            row.chunks_exact(4)
                .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte chunk")))
                .collect()
        })
        .collect())
}

fn i32_vector(blob: &NpyBlob<'_>) -> Result<Vec<i32>, EmbeddingsError> {
    if blob.descr != "<i4" || blob.fortran_order || blob.shape.len() != 1 {
        return Err(EmbeddingsError::Invalid(
            "statement_ids.npy must be a C-order one-dimensional <i4 array".to_owned(),
        ));
    }
    Ok(blob
        .payload
        .chunks_exact(4)
        .map(|bytes| i32::from_le_bytes(bytes.try_into().expect("four-byte chunk")))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    static TEST_SEQ: AtomicUsize = AtomicUsize::new(0);

    struct TempTestDir(PathBuf);
    impl TempTestDir {
        fn new() -> Self {
            let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
            let path = PathBuf::from("/var/tmp").join(format!(
                "solstone-embeddings-test-{}-{seq}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_test_npz(dir: &TempTestDir, durations: Option<Vec<f32>>) -> PathBuf {
        let payload_path = dir.path().join("payload.f32");
        let mut raw_bytes = Vec::new();
        for _ in 0..2 {
            for col in 0..256 {
                raw_bytes.extend_from_slice(&(col as f32).to_le_bytes());
            }
        }
        fs::write(&payload_path, raw_bytes).unwrap();
        let npz_path = dir.path().join("segment.npz");
        let dur_val = match durations {
            Some(d) => serde_json::json!(d),
            None => serde_json::json!([1.25, 2.5]),
        };
        let request = serde_json::json!({
            "schema": "solstone-speaker-transcript-write-request-v1",
            "output": {
                "jsonl_path": dir.path().join("segment.jsonl"),
                "npz_path": npz_path,
                "redo": true,
            },
            "base_time_us_of_day": 100_000_u64,
            "source": "audio",
            "statements": [
                {"id": 1, "start_offset_us": 0, "text": "one"},
                {"id": 2, "start_offset_us": 1_000_000, "text": "two"},
            ],
            "header": {"raw": "audio.wav", "model": "model", "device": "cpu", "compute_type": "int8"},
            "embeddings": {
                "payload_path": payload_path,
                "payload_format": "raw-f32le-row-major-v1",
                "dtype": "float32-le",
                "shape": [2, 256],
                "byte_count": 2 * 256 * 4,
                "statement_ids": [1, 2],
                "durations_s": dur_val,
                "encoder": "test-encoder",
            }
        });
        crate::writer::write_request(serde_json::to_vec(&request).unwrap().as_slice()).unwrap();
        npz_path
    }

    #[test]
    fn writer_produced_npz_round_trips_durations_and_statements() {
        let dir = TempTestDir::new();
        let npz_path = write_test_npz(&dir, Some(vec![1.5, 3.25]));
        let file = load_embeddings_file(&npz_path).unwrap().expect("loaded");
        assert_eq!(file.statements.len(), 2);
        assert_eq!(file.statements[0].0, 1);
        assert_eq!(file.statements[1].0, 2);
        assert_eq!(file.durations_s, vec![1.5, 3.25]);
    }

    #[test]
    fn missing_durations_member_yields_zero_durations() {
        let dir = TempTestDir::new();
        let npz_path = write_test_npz(&dir, Some(vec![1.5, 3.25]));
        // Re-pack the archive omitting durations_s.npy
        let file = File::open(&npz_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut members = Vec::new();
        for i in 0..archive.len() {
            let mut member = archive.by_index(i).unwrap();
            if member.name() != "durations_s.npy" {
                let mut data = Vec::new();
                member.read_to_end(&mut data).unwrap();
                members.push((member.name().to_owned(), data));
            }
        }
        let stripped_path = dir.path().join("stripped.npz");
        let dest = File::create(&stripped_path).unwrap();
        let mut zip_writer = ZipWriter::new(dest);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, data) in members {
            zip_writer.start_file(name, options).unwrap();
            zip_writer.write_all(&data).unwrap();
        }
        zip_writer.finish().unwrap();

        let file = load_embeddings_file(&stripped_path)
            .unwrap()
            .expect("loaded");
        assert_eq!(file.statements.len(), 2);
        assert_eq!(file.durations_s, vec![0.0, 0.0]);
    }

    #[test]
    fn shorter_durations_member_pads_with_zeros() {
        let dir = TempTestDir::new();
        let npz_path = write_test_npz(&dir, Some(vec![1.5, 3.25]));
        // Re-pack with a 1-element durations_s.npy (<f4)
        let file = File::open(&npz_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut members = Vec::new();
        for i in 0..archive.len() {
            let mut member = archive.by_index(i).unwrap();
            let name = member.name().to_owned();
            let mut data = Vec::new();
            member.read_to_end(&mut data).unwrap();
            if name == "durations_s.npy" {
                let single_dur =
                    solstone_core_npy::write_npy("<f4", "(1,)", &4.5_f32.to_le_bytes());
                members.push((name, single_dur));
            } else {
                members.push((name, data));
            }
        }
        let custom_path = dir.path().join("custom.npz");
        let dest = File::create(&custom_path).unwrap();
        let mut zip_writer = ZipWriter::new(dest);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, data) in members {
            zip_writer.start_file(name, options).unwrap();
            zip_writer.write_all(&data).unwrap();
        }
        zip_writer.finish().unwrap();

        let file = load_embeddings_file(&custom_path).unwrap().expect("loaded");
        assert_eq!(file.statements.len(), 2);
        assert_eq!(file.durations_s, vec![4.5, 0.0]);
    }

    #[test]
    fn wrong_dtype_durations_member_yields_zero_durations() {
        let dir = TempTestDir::new();
        let npz_path = write_test_npz(&dir, Some(vec![1.5, 3.25]));
        // Re-pack with an <i4 durations_s.npy
        let file = File::open(&npz_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut members = Vec::new();
        for i in 0..archive.len() {
            let mut member = archive.by_index(i).unwrap();
            let name = member.name().to_owned();
            let mut data = Vec::new();
            member.read_to_end(&mut data).unwrap();
            if name == "durations_s.npy" {
                let wrong_dtype =
                    solstone_core_npy::write_npy("<i4", "(2,)", &[1, 2, 3, 4, 5, 6, 7, 8]);
                members.push((name, wrong_dtype));
            } else {
                members.push((name, data));
            }
        }
        let custom_path = dir.path().join("wrong_dtype.npz");
        let dest = File::create(&custom_path).unwrap();
        let mut zip_writer = ZipWriter::new(dest);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, data) in members {
            zip_writer.start_file(name, options).unwrap();
            zip_writer.write_all(&data).unwrap();
        }
        zip_writer.finish().unwrap();

        let file = load_embeddings_file(&custom_path).unwrap().expect("loaded");
        assert_eq!(file.statements.len(), 2);
        assert_eq!(file.durations_s, vec![0.0, 0.0]);
    }
}
