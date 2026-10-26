// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Scratch files for one embedded recording's draft rows.
//!
//! Words, interval rows, statement embeddings, turns, and per-unit PCM stay on
//! disk. A reader holds one record, one embedding row, or one statement's PCM
//! span. The PCM index is one entry per native unit.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use solstone_core_speakers::SpeakerTurnRange;
use solstone_core_speakers::diarization::SpeakerInterval;

use crate::backend::parakeet_cpp::TranscriptionWord;
use crate::speakers::UnitStatementEmbedding;

const EMBEDDING_WIDTH: usize = 256;
const COPY_CHUNK: usize = 64 * 1024;
const STATEMENT_RECORD: usize = 24 + EMBEDDING_WIDTH * 4;

/// One statement embedding recovered from the spool.
pub(crate) struct StoredStatement {
    pub(crate) duration_s: f64,
    pub(crate) values: Vec<f32>,
}

struct UnitPcm {
    origin: usize,
    len: usize,
    path: PathBuf,
}

/// Private scratch for one recognize walk.
pub(crate) struct DraftSpool {
    dir: tempfile::TempDir,
    words: File,
    word_count: u64,
    intervals: File,
    interval_count: u64,
    embeddings: File,
    embeddings_path: PathBuf,
    statements: File,
    statement_count: u64,
    turns: File,
    turn_count: u64,
    turns_known: bool,
    labels: Option<File>,
    units: Vec<UnitPcm>,
}

impl DraftSpool {
    pub(crate) fn open(scratch: &Path) -> Result<Self, std::io::Error> {
        fs::create_dir_all(scratch)?;
        let dir = tempfile::Builder::new()
            .prefix("solstone-embedded-draft-")
            .tempdir_in(scratch)?;
        let embeddings_path = dir.path().join("interval-embeddings.f32le");
        Ok(Self {
            words: File::create(dir.path().join("words.bin"))?,
            word_count: 0,
            intervals: File::create(dir.path().join("intervals.bin"))?,
            interval_count: 0,
            embeddings: File::create(&embeddings_path)?,
            embeddings_path,
            statements: File::create(dir.path().join("statements.bin"))?,
            statement_count: 0,
            turns: File::create(dir.path().join("turns.bin"))?,
            turn_count: 0,
            turns_known: false,
            labels: None,
            units: Vec::new(),
            dir,
        })
    }

    pub(crate) fn word_count(&self) -> u64 {
        self.word_count
    }

    pub(crate) fn interval_count(&self) -> u64 {
        self.interval_count
    }

    pub(crate) fn turns_known(&self) -> bool {
        self.turns_known
    }

    pub(crate) fn set_turns_known(&mut self) {
        self.turns_known = true;
    }

    pub(crate) fn embeddings_path(&self) -> &Path {
        &self.embeddings_path
    }

    pub(crate) fn directory(&self) -> &Path {
        self.dir.path()
    }

    pub(crate) fn sync_rows(&mut self) -> Result<(), std::io::Error> {
        self.words.flush()?;
        self.intervals.flush()?;
        self.embeddings.flush()?;
        self.statements.flush()?;
        self.turns.flush()?;
        Ok(())
    }

    /// Read the next statement's words, stopping at punctuation or the last word.
    ///
    /// One statement is resident. An unpunctuated recording is that one statement.
    pub(crate) fn next_statement_words(
        &mut self,
        index: &mut u64,
    ) -> Result<Vec<TranscriptionWord>, std::io::Error> {
        if *index == 0 {
            self.words.seek(SeekFrom::Start(0))?;
        }
        let mut words = Vec::new();
        while *index < self.word_count {
            let word = read_word(&mut self.words)?;
            *index += 1;
            let boundary = word.word.trim_end().ends_with(['.', '?', '!']);
            words.push(word);
            if boundary {
                break;
            }
        }
        Ok(words)
    }

    pub(crate) fn append_words(
        &mut self,
        words: &[TranscriptionWord],
    ) -> Result<(), std::io::Error> {
        for word in words {
            let bytes = word.word.as_bytes();
            let len = u32::try_from(bytes.len()).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "word text exceeds u32")
            })?;
            self.words.write_all(&len.to_le_bytes())?;
            self.words.write_all(bytes)?;
            self.words.write_all(&word.start.to_le_bytes())?;
            self.words.write_all(&word.end.to_le_bytes())?;
            match word.probability {
                Some(probability) => {
                    self.words.write_all(&[1])?;
                    self.words.write_all(&probability.to_le_bytes())?;
                }
                None => {
                    self.words.write_all(&[0])?;
                    self.words.write_all(&0_f64.to_le_bytes())?;
                }
            }
            self.word_count += 1;
        }
        Ok(())
    }

    pub(crate) fn append_intervals(
        &mut self,
        intervals: &[SpeakerInterval],
        embeddings: &[f32],
    ) -> Result<(), std::io::Error> {
        if embeddings.len() != intervals.len() * EMBEDDING_WIDTH {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "interval embedding width",
            ));
        }
        for interval in intervals {
            self.intervals.write_all(&interval.start_s.to_le_bytes())?;
            self.intervals.write_all(&interval.end_s.to_le_bytes())?;
            self.intervals.write_all(&[interval.local_class])?;
        }
        write_f32s(&mut self.embeddings, embeddings)?;
        self.interval_count += intervals.len() as u64;
        Ok(())
    }

    pub(crate) fn append_statement_rows(
        &mut self,
        rows: &[UnitStatementEmbedding],
    ) -> Result<(), std::io::Error> {
        for row in rows {
            if row.values.len() != EMBEDDING_WIDTH {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "statement embedding width",
                ));
            }
            write_statement(
                &mut self.statements,
                row.start_s,
                row.end_s,
                row.duration_s,
                &row.values,
            )?;
            self.statement_count += 1;
        }
        Ok(())
    }

    pub(crate) fn append_turns(&mut self, turns: &[(f64, f64)]) -> Result<(), std::io::Error> {
        for (start, end) in turns {
            self.turns.write_all(&start.to_le_bytes())?;
            self.turns.write_all(&end.to_le_bytes())?;
            self.turn_count += 1;
        }
        Ok(())
    }

    pub(crate) fn store_unit_pcm(
        &mut self,
        origin: usize,
        samples: &[f32],
    ) -> Result<(), std::io::Error> {
        let path = self
            .dir
            .path()
            .join(format!("unit-{}-{}.f32le", origin, self.units.len()));
        let mut file = File::create(&path)?;
        write_f32s(&mut file, samples)?;
        file.sync_all()?;
        self.units.push(UnitPcm {
            origin,
            len: samples.len(),
            path,
        });
        Ok(())
    }

    pub(crate) fn clear_rows(&mut self) -> Result<(), std::io::Error> {
        truncate(&mut self.words)?;
        truncate(&mut self.intervals)?;
        truncate(&mut self.embeddings)?;
        truncate(&mut self.statements)?;
        truncate(&mut self.turns)?;
        self.word_count = 0;
        self.interval_count = 0;
        self.statement_count = 0;
        self.turn_count = 0;
        self.turns_known = false;
        self.labels = None;
        Ok(())
    }

    pub(crate) fn write_labels(&mut self, labels: &[usize]) -> Result<(), std::io::Error> {
        if labels.len() as u64 != self.interval_count {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "cluster label count",
            ));
        }
        let path = self.dir.path().join("labels.u32le");
        let mut file = File::create(&path)?;
        for label in labels {
            let label = u32::try_from(*label).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "cluster label exceeds u32")
            })?;
            file.write_all(&label.to_le_bytes())?;
        }
        file.sync_all()?;
        self.labels = Some(File::open(path)?);
        Ok(())
    }

    pub(crate) fn for_each_labeled_interval(
        &mut self,
        mut visit: impl FnMut(SpeakerInterval, usize),
    ) -> Result<(), std::io::Error> {
        let Some(labels) = self.labels.as_mut() else {
            return Ok(());
        };
        self.intervals.seek(SeekFrom::Start(0))?;
        labels.seek(SeekFrom::Start(0))?;
        for _ in 0..self.interval_count {
            let mut record = [0_u8; 17];
            self.intervals.read_exact(&mut record)?;
            let start_s = f64::from_le_bytes(record[0..8].try_into().expect("8 bytes"));
            let end_s = f64::from_le_bytes(record[8..16].try_into().expect("8 bytes"));
            let mut label_bytes = [0_u8; 4];
            labels.read_exact(&mut label_bytes)?;
            visit(
                SpeakerInterval {
                    start_s,
                    end_s,
                    local_class: record[16],
                },
                u32::from_le_bytes(label_bytes) as usize,
            );
        }
        Ok(())
    }

    pub(crate) fn find_statement_row(
        &mut self,
        start_s: f64,
        end_s: f64,
    ) -> Result<Option<StoredStatement>, std::io::Error> {
        self.statements.seek(SeekFrom::Start(0))?;
        let mut record = vec![0_u8; STATEMENT_RECORD];
        for _ in 0..self.statement_count {
            self.statements.read_exact(&mut record)?;
            let start = f64::from_le_bytes(record[0..8].try_into().expect("8 bytes"));
            let end = f64::from_le_bytes(record[8..16].try_into().expect("8 bytes"));
            if start == start_s && end == end_s {
                let duration_s = f64::from_le_bytes(record[16..24].try_into().expect("8 bytes"));
                let mut values = Vec::with_capacity(EMBEDDING_WIDTH);
                for chunk in record[24..].chunks_exact(4) {
                    values.push(f32::from_le_bytes(chunk.try_into().expect("4 bytes")));
                }
                return Ok(Some(StoredStatement { duration_s, values }));
            }
        }
        Ok(None)
    }

    pub(crate) fn overlapping_turns(
        &mut self,
        start_s: f64,
        end_s: f64,
    ) -> Result<Vec<SpeakerTurnRange>, std::io::Error> {
        self.turns.seek(SeekFrom::Start(0))?;
        let mut ranges = Vec::new();
        for _ in 0..self.turn_count {
            let mut record = [0_u8; 16];
            self.turns.read_exact(&mut record)?;
            let start = f64::from_le_bytes(record[0..8].try_into().expect("8 bytes"));
            let end = f64::from_le_bytes(record[8..16].try_into().expect("8 bytes"));
            if end >= start_s && start <= end_s {
                ranges.push(SpeakerTurnRange {
                    start_s: start,
                    end_s: end,
                });
            }
        }
        Ok(merge_turn_ranges(ranges))
    }

    /// Samples `[start, end)` with a later unit's bytes winning the overlap.
    pub(crate) fn read_pcm_span(
        &self,
        start: usize,
        end: usize,
    ) -> Result<Vec<f32>, std::io::Error> {
        if end < start {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "pcm span is inverted",
            ));
        }
        let mut samples = vec![0.0_f32; end - start];
        for unit in &self.units {
            let unit_end = unit.origin.saturating_add(unit.len);
            let overlap_start = start.max(unit.origin);
            let overlap_end = end.min(unit_end);
            if overlap_start >= overlap_end {
                continue;
            }
            let mut file = File::open(&unit.path)?;
            let offset = (overlap_start - unit.origin) * std::mem::size_of::<f32>();
            file.seek(SeekFrom::Start(offset as u64))?;
            let dest = overlap_start - start;
            let count = overlap_end - overlap_start;
            read_f32s(&mut file, &mut samples[dest..dest + count])?;
        }
        Ok(samples)
    }
}

fn write_statement(
    file: &mut File,
    start_s: f64,
    end_s: f64,
    duration_s: f64,
    values: &[f32],
) -> Result<(), std::io::Error> {
    file.write_all(&start_s.to_le_bytes())?;
    file.write_all(&end_s.to_le_bytes())?;
    file.write_all(&duration_s.to_le_bytes())?;
    write_f32s(file, values)
}

fn write_f32s(file: &mut File, samples: &[f32]) -> Result<(), std::io::Error> {
    let mut bytes = [0_u8; COPY_CHUNK];
    let mut filled = 0;
    for sample in samples {
        let next = sample.to_le_bytes();
        if filled + next.len() > bytes.len() {
            file.write_all(&bytes[..filled])?;
            filled = 0;
        }
        bytes[filled..filled + next.len()].copy_from_slice(&next);
        filled += next.len();
    }
    if filled > 0 {
        file.write_all(&bytes[..filled])?;
    }
    Ok(())
}

fn read_f32s(file: &mut File, samples: &mut [f32]) -> Result<(), std::io::Error> {
    let mut bytes = vec![0_u8; samples.len() * 4];
    file.read_exact(&mut bytes)?;
    for (sample, chunk) in samples.iter_mut().zip(bytes.chunks_exact(4)) {
        *sample = f32::from_le_bytes(chunk.try_into().expect("4 bytes"));
    }
    Ok(())
}

fn read_word(file: &mut File) -> Result<TranscriptionWord, std::io::Error> {
    let mut len_bytes = [0_u8; 4];
    file.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    let mut text = vec![0_u8; len];
    file.read_exact(&mut text)?;
    let word = String::from_utf8(text).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "word text is not utf-8")
    })?;
    let mut times = [0_u8; 16];
    file.read_exact(&mut times)?;
    let start = f64::from_le_bytes(times[0..8].try_into().expect("8 bytes"));
    let end = f64::from_le_bytes(times[8..16].try_into().expect("8 bytes"));
    let mut flag = [0_u8; 1];
    file.read_exact(&mut flag)?;
    let mut probability_bytes = [0_u8; 8];
    file.read_exact(&mut probability_bytes)?;
    let probability = (flag[0] == 1).then(|| f64::from_le_bytes(probability_bytes));
    Ok(TranscriptionWord {
        word,
        start,
        end,
        probability,
    })
}

fn truncate(file: &mut File) -> Result<(), std::io::Error> {
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)
}

fn merge_turn_ranges(mut ranges: Vec<SpeakerTurnRange>) -> Vec<SpeakerTurnRange> {
    ranges.retain(|range| {
        range.start_s.is_finite() && range.end_s.is_finite() && range.start_s < range.end_s
    });
    ranges.sort_by(|left, right| {
        left.start_s
            .partial_cmp(&right.start_s)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut merged: Vec<SpeakerTurnRange> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start_s <= last.end_s
        {
            last.end_s = last.end_s.max(range.end_s);
        } else {
            merged.push(range);
        }
    }
    merged
}
