// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Scratch files for one embedded recording's draft rows.
//!
//! Words, interval rows, statement embeddings, turns, and per-unit PCM stay on
//! disk. A reader holds one word, one embedding row, or one statement's PCM.
//! Statement text is written by the caller; an unpunctuated recording is not
//! assembled into one word vector. The PCM index is a fixed record per native
//! unit, scanned with a checkpoint, not a resident path list.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use solstone_core_observe_audio::SAMPLE_RATE;
use solstone_core_speakers::SENTENCE_TURN_INSET_S;
use solstone_core_speakers::diarization::SpeakerInterval;

use crate::backend::parakeet_cpp::TranscriptionWord;
use crate::speakers::UnitStatementEmbedding;

const EMBEDDING_WIDTH: usize = 256;
const COPY_CHUNK: usize = 64 * 1024;
const STATEMENT_RECORD: usize = 24 + EMBEDDING_WIDTH * 4;
const SCAN_CHECKPOINT: u64 = 32;
const TURN_TILE: u64 = 32;
const UNIT_RECORD: u64 = 32;
const COVER_RECORD: u64 = 24;
const PIECE_RECORD: u64 = 32;
const NONE_LEN: u64 = u64::MAX;

/// One statement's place in the word spool. The words themselves stay on disk.
pub(crate) struct WordSpan {
    pub(crate) origin: u64,
    pub(crate) words: u64,
    pub(crate) start_s: f64,
    pub(crate) end_s: f64,
}

/// One statement embedding recovered from the spool.
pub(crate) struct StoredStatement {
    pub(crate) duration_s: f64,
    pub(crate) values: Vec<f32>,
}

/// One reduced-audio interval copied from a unit's pre-restoration mapping.
#[derive(Clone, Copy)]
pub(crate) struct StoredSpan {
    pub(crate) original_start_s: f64,
    pub(crate) original_end_s: f64,
    pub(crate) reduced_start_s: f64,
    pub(crate) reduced_end_s: f64,
}

/// Sample count of the buffer a statement embedding would see.
pub(crate) struct StatementAudioMeasure {
    pub(crate) samples: usize,
    /// True when the count is the reduced concatenation, not `end - start`.
    pub(crate) reduced: bool,
}

/// Whole-statement PCM assembled only by the spool test oracle.
#[cfg(all(test, feature = "full-tests"))]
pub(crate) struct StatementAudio {
    pub(crate) samples: Vec<f32>,
    pub(crate) reduced: bool,
}

/// Overlapping turns already merged on disk. The caller streams one range at a time.
pub(crate) struct PreparedTurns {
    pub(crate) count: u64,
    pub(crate) boundary_suspect: bool,
}

struct UnitRecord {
    origin: usize,
    masked_len: usize,
    reduced_len: Option<usize>,
    map_count: usize,
}

struct Cover {
    start: u64,
    end: u64,
    seq: u64,
}

struct AudioPiece {
    seq: u64,
    offset: usize,
    len: usize,
    reduced: bool,
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
    units: File,
    unit_count: u64,
}

impl DraftSpool {
    pub(crate) fn open(scratch: &Path) -> Result<Self, std::io::Error> {
        fs::create_dir_all(scratch)?;
        let mut builder = tempfile::Builder::new();
        builder.prefix("solstone-embedded-draft-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(fs::Permissions::from_mode(0o700));
        }
        let dir = builder.tempdir_in(scratch)?;
        let embeddings_path = dir.path().join("interval-embeddings.f32le");
        Ok(Self {
            words: read_write(dir.path().join("words.bin"))?,
            word_count: 0,
            intervals: read_write(dir.path().join("intervals.bin"))?,
            interval_count: 0,
            embeddings: read_write(&embeddings_path)?,
            embeddings_path,
            statements: read_write(dir.path().join("statements.bin"))?,
            statement_count: 0,
            turns: read_write(dir.path().join("turns.bin"))?,
            turn_count: 0,
            turns_known: false,
            labels: None,
            units: read_write(dir.path().join("units.bin"))?,
            unit_count: 0,
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
        self.units.flush()?;
        Ok(())
    }

    /// Locate the next statement without retaining its words.
    ///
    /// An unpunctuated recording is still one statement. Its words stay in the
    /// spool; the caller reads them one at a time. Every 32 words polls `cancel`.
    pub(crate) fn next_statement_span(
        &mut self,
        index: &mut u64,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Option<WordSpan>, std::io::Error> {
        if *index >= self.word_count {
            return Ok(None);
        }
        if *index == 0 {
            self.words.seek(SeekFrom::Start(0))?;
        }
        let origin = self.words.stream_position()?;
        let mut words = 0_u64;
        let mut start_s = 0.0_f64;
        let mut end_s = 0.0_f64;
        while *index < self.word_count {
            if words.is_multiple_of(SCAN_CHECKPOINT) {
                cancelled(cancel)?;
            }
            let word = read_word(&mut self.words)?;
            if words == 0 {
                start_s = word.start;
            }
            end_s = word.end;
            *index += 1;
            words += 1;
            if word.word.trim_end().ends_with(['.', '?', '!']) {
                break;
            }
        }
        Ok(Some(WordSpan {
            origin,
            words,
            start_s,
            end_s,
        }))
    }

    pub(crate) fn seek_words(&mut self, origin: u64) -> Result<(), std::io::Error> {
        self.words.seek(SeekFrom::Start(origin))?;
        Ok(())
    }

    pub(crate) fn read_next_word(&mut self) -> Result<TranscriptionWord, std::io::Error> {
        read_word(&mut self.words)
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

    /// Store one unit's masked PCM and, when present, its reduced buffer.
    ///
    /// The reduced buffer is the post-mask audio the statement embedder saw,
    /// with the mapping from before timestamp restoration. The index is one
    /// fixed record; the samples live in `unit-{seq}` files.
    pub(crate) fn store_unit_pcm(
        &mut self,
        origin: usize,
        masked: &[f32],
        reduced: Option<(&[f32], &[StoredSpan])>,
    ) -> Result<(), std::io::Error> {
        let seq = self.unit_count;
        let masked_path = self.masked_path(seq);
        let mut masked_file = read_write(&masked_path)?;
        write_f32s(&mut masked_file, masked)?;
        masked_file.sync_all()?;
        let (reduced_len, map_count) = if let Some((samples, spans)) = reduced {
            let mut reduced_file = read_write(self.reduced_path(seq))?;
            write_f32s(&mut reduced_file, samples)?;
            reduced_file.sync_all()?;
            let mut map = read_write(self.map_path(seq))?;
            for span in spans {
                map.write_all(&span.original_start_s.to_le_bytes())?;
                map.write_all(&span.original_end_s.to_le_bytes())?;
                map.write_all(&span.reduced_start_s.to_le_bytes())?;
                map.write_all(&span.reduced_end_s.to_le_bytes())?;
            }
            map.sync_all()?;
            (
                u64::try_from(samples.len()).unwrap_or(NONE_LEN),
                spans.len() as u64,
            )
        } else {
            (NONE_LEN, 0)
        };
        self.units
            .seek(SeekFrom::Start(seq.saturating_mul(UNIT_RECORD)))?;
        self.units
            .write_all(&u64::try_from(origin).unwrap_or(0).to_le_bytes())?;
        self.units
            .write_all(&u64::try_from(masked.len()).unwrap_or(0).to_le_bytes())?;
        self.units.write_all(&reduced_len.to_le_bytes())?;
        self.units.write_all(&map_count.to_le_bytes())?;
        self.unit_count += 1;
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

    pub(crate) fn adopt_labels(&mut self, path: &Path) -> Result<(), std::io::Error> {
        let length = fs::metadata(path)?.len();
        let expected = self.interval_count.saturating_mul(4);
        if length != expected {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "cluster label count",
            ));
        }
        self.labels = Some(File::open(path)?);
        Ok(())
    }

    pub(crate) fn for_each_labeled_interval(
        &mut self,
        cancel: &dyn Fn() -> bool,
        mut visit: impl FnMut(SpeakerInterval, usize),
    ) -> Result<(), std::io::Error> {
        let Some(labels) = self.labels.as_mut() else {
            return Ok(());
        };
        self.intervals.seek(SeekFrom::Start(0))?;
        labels.seek(SeekFrom::Start(0))?;
        for index in 0..self.interval_count {
            if index.is_multiple_of(SCAN_CHECKPOINT) {
                cancelled(cancel)?;
            }
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
        cancel: &dyn Fn() -> bool,
    ) -> Result<Option<StoredStatement>, std::io::Error> {
        self.statements.seek(SeekFrom::Start(0))?;
        let mut record = vec![0_u8; STATEMENT_RECORD];
        for index in 0..self.statement_count {
            if index.is_multiple_of(SCAN_CHECKPOINT) {
                cancelled(cancel)?;
            }
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

    /// Spill overlapping turns, sort them in 32-record tiles, and merge overlaps.
    ///
    /// The range vector is not built here. [`Self::open_merged_turns`] reads one
    /// range at a time after statement admission.
    pub(crate) fn prepare_overlapping_turns(
        &mut self,
        start_s: f64,
        end_s: f64,
        cancel: &dyn Fn() -> bool,
    ) -> Result<PreparedTurns, std::io::Error> {
        self.turns.seek(SeekFrom::Start(0))?;
        let overlap_path = self.dir.path().join("turn-overlap.bin");
        let mut overlap = read_write(&overlap_path)?;
        let mut count = 0_u64;
        for index in 0..self.turn_count {
            if index.is_multiple_of(SCAN_CHECKPOINT) {
                cancelled(cancel)?;
            }
            let (start, end) = read_pair(&mut self.turns)?;
            if start.is_finite()
                && end.is_finite()
                && start < end
                && end >= start_s
                && start <= end_s
            {
                write_pair(&mut overlap, start, end)?;
                count += 1;
            }
        }
        overlap.sync_all()?;
        drop(overlap);
        let merged = self.sort_overlapping_turns(count, cancel)?;
        let boundary_suspect = self.sentence_is_boundary_suspect(start_s, end_s, cancel)?;
        Ok(PreparedTurns {
            count: merged,
            boundary_suspect,
        })
    }

    /// Open the merged turn file when it has ranges. An empty preparation reads nothing.
    pub(crate) fn open_merged_turns(&self, count: u64) -> Result<Option<File>, std::io::Error> {
        if count == 0 {
            return Ok(None);
        }
        Ok(Some(File::open(self.merged_turns_path())?))
    }

    /// Read one merged turn range. The caller polls between ranges.
    pub(crate) fn read_merged_range(file: &mut File) -> Result<(f64, f64), std::io::Error> {
        read_pair(file)
    }

    /// Count the samples the statement embedder would receive, without copying them.
    ///
    /// A span no unit reduced is `end - start`, including holes, and does not
    /// build a cover file. A span that meets a reduced unit streams covers and
    /// pieces to disk, one record at a time, with the later unit winning an overlap.
    pub(crate) fn measure_statement_audio(
        &mut self,
        start: usize,
        end: usize,
        cancel: &dyn Fn() -> bool,
    ) -> Result<StatementAudioMeasure, std::io::Error> {
        if end < start {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "pcm span is inverted",
            ));
        }
        if !self.span_has_reduced_unit(start, end, cancel)? {
            return Ok(StatementAudioMeasure {
                samples: end - start,
                reduced: false,
            });
        }
        self.scan_covers(start, end, cancel)?;
        let samples = self.write_pieces(cancel)?;
        Ok(StatementAudioMeasure {
            samples,
            reduced: true,
        })
    }

    /// Copy one statement after [`Self::measure_statement_audio`] accepted it.
    ///
    /// Test oracle only. Production embedding reads bounded slices.
    #[cfg(all(test, feature = "full-tests"))]
    pub(crate) fn read_statement_audio(
        &mut self,
        start: usize,
        end: usize,
        cancel: &dyn Fn() -> bool,
    ) -> Result<StatementAudio, std::io::Error> {
        if end < start {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "pcm span is inverted",
            ));
        }
        #[cfg(all(test, feature = "full-tests"))]
        note_whole_statement();
        if !self.span_has_reduced_unit(start, end, cancel)? {
            let mut samples = vec![0.0_f32; end - start];
            self.units.seek(SeekFrom::Start(0))?;
            for seq in 0..self.unit_count {
                if seq.is_multiple_of(SCAN_CHECKPOINT) {
                    cancelled(cancel)?;
                }
                let record = read_unit_record(&mut self.units)?;
                let unit_end = record.origin.saturating_add(record.masked_len);
                let overlap_start = start.max(record.origin);
                let overlap_end = end.min(unit_end);
                if overlap_start >= overlap_end {
                    continue;
                }
                let mut file = File::open(self.masked_path(seq))?;
                let offset = (overlap_start - record.origin) * std::mem::size_of::<f32>();
                file.seek(SeekFrom::Start(offset as u64))?;
                let dest = overlap_start - start;
                let count = overlap_end - overlap_start;
                read_f32s(&mut file, &mut samples[dest..dest + count])?;
            }
            return Ok(StatementAudio {
                samples,
                reduced: false,
            });
        }
        self.scan_covers(start, end, cancel)?;
        let total = self.write_pieces(cancel)?;
        let mut samples = vec![0.0_f32; total];
        self.copy_pieces(&mut samples, cancel)?;
        Ok(StatementAudio {
            samples,
            reduced: true,
        })
    }

    /// Copy one bounded slice of a statement measured by [`Self::measure_statement_audio`].
    ///
    /// `offset` is into that measured buffer. Unreduced audio zero-fills holes and
    /// lets a later unit overwrite an overlap. Reduced audio reads `pieces.bin`
    /// and does not rescan covers. The caller passes `reduced` from the measure.
    pub(crate) fn read_statement_slice(
        &mut self,
        span_start: usize,
        span_end: usize,
        offset: usize,
        dest: &mut [f32],
        reduced: bool,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), std::io::Error> {
        #[cfg(all(test, feature = "full-tests"))]
        note_slice(dest.len());
        if reduced {
            self.read_reduced_slice(offset, dest, cancel)?;
        } else {
            self.read_unreduced_slice(span_start, span_end, offset, dest, cancel)?;
        }
        #[cfg(all(test, feature = "full-tests"))]
        record_statement_probe(offset, dest);
        Ok(())
    }

    fn read_unreduced_slice(
        &mut self,
        span_start: usize,
        span_end: usize,
        offset: usize,
        dest: &mut [f32],
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), std::io::Error> {
        if span_end < span_start {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "pcm span is inverted",
            ));
        }
        let Some(slice_start) = span_start.checked_add(offset) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "statement slice offset overflows",
            ));
        };
        let Some(slice_end) = slice_start.checked_add(dest.len()) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "statement slice length overflows",
            ));
        };
        if slice_end > span_end {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "statement slice passes the span",
            ));
        }
        dest.fill(0.0);
        self.units.seek(SeekFrom::Start(0))?;
        for seq in 0..self.unit_count {
            if seq.is_multiple_of(SCAN_CHECKPOINT) {
                cancelled(cancel)?;
            }
            let record = read_unit_record(&mut self.units)?;
            let unit_end = record.origin.saturating_add(record.masked_len);
            let overlap_start = slice_start.max(record.origin);
            let overlap_end = slice_end.min(unit_end);
            if overlap_start >= overlap_end {
                continue;
            }
            let mut file = File::open(self.masked_path(seq))?;
            let file_offset = (overlap_start - record.origin) * std::mem::size_of::<f32>();
            file.seek(SeekFrom::Start(file_offset as u64))?;
            let dest_offset = overlap_start - slice_start;
            let count = overlap_end - overlap_start;
            read_f32s(&mut file, &mut dest[dest_offset..dest_offset + count])?;
        }
        Ok(())
    }

    fn read_reduced_slice(
        &mut self,
        offset: usize,
        dest: &mut [f32],
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), std::io::Error> {
        let Some(slice_end) = offset.checked_add(dest.len()) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "statement slice length overflows",
            ));
        };
        dest.fill(0.0);
        let mut file = File::open(self.pieces_path())?;
        let count = file.metadata()?.len() / PIECE_RECORD;
        let mut cursor = 0_usize;
        let mut covered = 0_usize;
        for index in 0..count {
            if index.is_multiple_of(SCAN_CHECKPOINT) {
                cancelled(cancel)?;
            }
            let piece = read_piece_record(&mut file)?;
            let Some(piece_end) = cursor.checked_add(piece.len) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "statement piece length overflows",
                ));
            };
            let overlap_start = offset.max(cursor);
            let overlap_end = slice_end.min(piece_end);
            if overlap_start < overlap_end {
                let local = overlap_start - cursor;
                let dest_offset = overlap_start - offset;
                let len = overlap_end - overlap_start;
                self.read_piece_range(&piece, local, &mut dest[dest_offset..dest_offset + len])?;
                covered += len;
            }
            cursor = piece_end;
            if cursor >= slice_end {
                break;
            }
        }
        if covered != dest.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "statement slice is short",
            ));
        }
        Ok(())
    }

    fn span_has_reduced_unit(
        &mut self,
        start: usize,
        end: usize,
        cancel: &dyn Fn() -> bool,
    ) -> Result<bool, std::io::Error> {
        self.units.seek(SeekFrom::Start(0))?;
        for seq in 0..self.unit_count {
            if seq.is_multiple_of(SCAN_CHECKPOINT) {
                cancelled(cancel)?;
            }
            let record = read_unit_record(&mut self.units)?;
            if record.reduced_len.is_none() {
                continue;
            }
            let unit_end = record.origin.saturating_add(record.masked_len);
            if start.max(record.origin) < end.min(unit_end) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn scan_covers(
        &mut self,
        start: usize,
        end: usize,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), std::io::Error> {
        self.reset_covers()?;
        self.units.seek(SeekFrom::Start(0))?;
        for seq in 0..self.unit_count {
            if seq.is_multiple_of(SCAN_CHECKPOINT) {
                cancelled(cancel)?;
            }
            let record = read_unit_record(&mut self.units)?;
            let unit_end = record.origin.saturating_add(record.masked_len);
            let overlap_start = start.max(record.origin);
            let overlap_end = end.min(unit_end);
            if overlap_start >= overlap_end {
                continue;
            }
            self.overlay_cover(overlap_start, overlap_end, seq, cancel)?;
        }
        Ok(())
    }

    fn reset_covers(&self) -> Result<(), std::io::Error> {
        let file = read_write(self.covers_path())?;
        file.set_len(0)?;
        file.sync_all()
    }

    /// Rewrite the sorted cover file with one new interval. One record is resident.
    fn overlay_cover(
        &self,
        start: usize,
        end: usize,
        seq: u64,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), std::io::Error> {
        if start >= end {
            return Ok(());
        }
        let covers_path = self.covers_path();
        let next_path = self.dir.path().join("covers-next.bin");
        let mut input = File::open(&covers_path)?;
        let mut output = read_write(&next_path)?;
        let incoming = Cover {
            start: start as u64,
            end: end as u64,
            seq,
        };
        let mut inserted = false;
        let mut seen = 0_u64;
        loop {
            if seen.is_multiple_of(SCAN_CHECKPOINT) {
                cancelled(cancel)?;
            }
            match read_cover(&mut input) {
                Ok(cover) => {
                    seen += 1;
                    emit_overlay(&mut output, &cover, &incoming, &mut inserted)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(error) => return Err(error),
            }
        }
        if !inserted {
            write_cover(&mut output, &incoming)?;
        }
        output.sync_all()?;
        drop(output);
        drop(input);
        match fs::remove_file(&covers_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::rename(&next_path, &covers_path)
    }

    fn write_pieces(&mut self, cancel: &dyn Fn() -> bool) -> Result<usize, std::io::Error> {
        let mut covers = File::open(self.covers_path())?;
        let mut pieces = read_write(self.pieces_path())?;
        let cover_count = covers.metadata()?.len() / COVER_RECORD;
        let mut total = 0_usize;
        let rate = f64::from(SAMPLE_RATE);
        for index in 0..cover_count {
            if index.is_multiple_of(SCAN_CHECKPOINT) {
                cancelled(cancel)?;
            }
            let cover = read_cover(&mut covers)?;
            let record = self.read_unit_at(cover.seq)?;
            let cover_start = u64_to_usize(cover.start)?;
            let cover_end = u64_to_usize(cover.end)?;
            let local_start = cover_start.saturating_sub(record.origin);
            let local_end = cover_end
                .saturating_sub(record.origin)
                .min(record.masked_len);
            if local_start >= local_end {
                continue;
            }
            let Some(reduced_len) = record.reduced_len else {
                let len = local_end - local_start;
                write_piece(&mut pieces, cover.seq, local_start, len, false)?;
                total += len;
                continue;
            };
            let mut maps = File::open(self.map_path(cover.seq))?;
            let map_count = u64::try_from(record.map_count).unwrap_or(0);
            for span_index in 0..map_count {
                if span_index.is_multiple_of(SCAN_CHECKPOINT) {
                    cancelled(cancel)?;
                }
                let span = read_span(&mut maps)?;
                let seg_start = seconds_to_sample(span.original_start_s, rate);
                let seg_end = seconds_to_sample(span.original_end_s, rate);
                let overlap_start = local_start.max(seg_start);
                let overlap_end = local_end.min(seg_end);
                if overlap_start >= overlap_end {
                    continue;
                }
                let reduced_start_s =
                    span.reduced_start_s + (overlap_start as f64 / rate - span.original_start_s);
                let reduced_end_s =
                    span.reduced_start_s + (overlap_end as f64 / rate - span.original_start_s);
                let reduced_start = seconds_to_sample(reduced_start_s, rate).min(reduced_len);
                let reduced_end = seconds_to_sample(reduced_end_s, rate).min(reduced_len);
                if reduced_start < reduced_end {
                    let len = reduced_end - reduced_start;
                    write_piece(&mut pieces, cover.seq, reduced_start, len, true)?;
                    total += len;
                }
            }
        }
        pieces.sync_all()?;
        Ok(total)
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn copy_pieces(
        &mut self,
        dest: &mut [f32],
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), std::io::Error> {
        let mut file = File::open(self.pieces_path())?;
        let count = file.metadata()?.len() / PIECE_RECORD;
        let mut filled = 0_usize;
        for index in 0..count {
            if index.is_multiple_of(SCAN_CHECKPOINT) {
                cancelled(cancel)?;
            }
            let piece = read_piece_record(&mut file)?;
            let next = filled + piece.len;
            self.read_piece(&piece, &mut dest[filled..next])?;
            filled = next;
        }
        Ok(())
    }

    fn read_unit_at(&mut self, seq: u64) -> Result<UnitRecord, std::io::Error> {
        self.units
            .seek(SeekFrom::Start(seq.saturating_mul(UNIT_RECORD)))?;
        read_unit_record(&mut self.units)
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn read_piece(&self, piece: &AudioPiece, dest: &mut [f32]) -> Result<(), std::io::Error> {
        self.read_piece_range(piece, 0, dest)
    }

    fn read_piece_range(
        &self,
        piece: &AudioPiece,
        local_offset: usize,
        dest: &mut [f32],
    ) -> Result<(), std::io::Error> {
        let path = if piece.reduced {
            self.reduced_path(piece.seq)
        } else {
            self.masked_path(piece.seq)
        };
        let mut file = File::open(path)?;
        let Some(sample) = piece.offset.checked_add(local_offset) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "statement piece offset overflows",
            ));
        };
        let offset = sample * std::mem::size_of::<f32>();
        file.seek(SeekFrom::Start(offset as u64))?;
        read_f32s(&mut file, dest)
    }

    fn covers_path(&self) -> PathBuf {
        self.dir.path().join("covers.bin")
    }

    fn pieces_path(&self) -> PathBuf {
        self.dir.path().join("pieces.bin")
    }

    fn masked_path(&self, seq: u64) -> PathBuf {
        self.dir.path().join(format!("unit-{seq}.f32le"))
    }

    fn reduced_path(&self, seq: u64) -> PathBuf {
        self.dir.path().join(format!("unit-{seq}.reduced.f32le"))
    }

    fn map_path(&self, seq: u64) -> PathBuf {
        self.dir.path().join(format!("unit-{seq}.map.bin"))
    }

    fn merged_turns_path(&self) -> PathBuf {
        self.dir.path().join("turn-merged.bin")
    }

    fn sort_overlapping_turns(
        &self,
        count: u64,
        cancel: &dyn Fn() -> bool,
    ) -> Result<u64, std::io::Error> {
        let merged_path = self.merged_turns_path();
        if count == 0 {
            read_write(&merged_path)?.set_len(0)?;
            return Ok(0);
        }
        let data_a = self.dir.path().join("turn-run-a.bin");
        let data_b = self.dir.path().join("turn-run-b.bin");
        let index_a = self.dir.path().join("turn-index-a.bin");
        let index_b = self.dir.path().join("turn-index-b.bin");
        let overlap_path = self.dir.path().join("turn-overlap.bin");
        let mut input = File::open(&overlap_path)?;
        let mut data = read_write(&data_a)?;
        let mut index = read_write(&index_a)?;
        let mut remaining = count;
        let mut run_count = 0_u64;
        let mut data_offset = 0_u64;
        while remaining > 0 {
            cancelled(cancel)?;
            let tile = remaining.min(TURN_TILE);
            let mut records = Vec::with_capacity(usize::try_from(tile).unwrap_or(0));
            for _ in 0..tile {
                records.push(read_pair(&mut input)?);
            }
            records.sort_by(|left, right| {
                left.0
                    .partial_cmp(&right.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for (start, end) in &records {
                write_pair(&mut data, *start, *end)?;
            }
            write_run(&mut index, data_offset, tile)?;
            data_offset += tile.saturating_mul(16);
            remaining -= tile;
            run_count += 1;
        }
        data.sync_all()?;
        index.sync_all()?;
        drop(data);
        drop(index);
        let mut source_a = true;
        let mut final_data = data_a.clone();
        while run_count > 1 {
            cancelled(cancel)?;
            let (src_data, src_index, dst_data, dst_index) = if source_a {
                (&data_a, &index_a, &data_b, &index_b)
            } else {
                (&data_b, &index_b, &data_a, &index_a)
            };
            let mut source_index = File::open(src_index)?;
            let mut dest_data = read_write(dst_data)?;
            let mut dest_index = read_write(dst_index)?;
            let mut seen = 0_u64;
            let mut out_runs = 0_u64;
            let mut out_offset = 0_u64;
            while seen < run_count {
                cancelled(cancel)?;
                let (left_offset, left_count) = read_run(&mut source_index)?;
                seen += 1;
                let written = if seen == run_count {
                    copy_run(src_data, left_offset, left_count, &mut dest_data, cancel)?
                } else {
                    let (right_offset, right_count) = read_run(&mut source_index)?;
                    seen += 1;
                    merge_runs(
                        src_data,
                        left_offset,
                        left_count,
                        right_offset,
                        right_count,
                        &mut dest_data,
                        cancel,
                    )?
                };
                write_run(&mut dest_index, out_offset, written)?;
                out_offset += written.saturating_mul(16);
                out_runs += 1;
            }
            dest_data.sync_all()?;
            dest_index.sync_all()?;
            run_count = out_runs;
            final_data = dst_data.clone();
            source_a = !source_a;
        }
        overlap_merge(&final_data, &merged_path, cancel)
    }

    fn sentence_is_boundary_suspect(
        &self,
        start_s: f64,
        end_s: f64,
        cancel: &dyn Fn() -> bool,
    ) -> Result<bool, std::io::Error> {
        if !start_s.is_finite() || !end_s.is_finite() {
            return Ok(false);
        }
        let path = self.merged_turns_path();
        if !path.is_file() {
            return Ok(false);
        }
        let mut file = File::open(path)?;
        let records = file.metadata()?.len() / 16;
        for index in 0..records {
            if index.is_multiple_of(SCAN_CHECKPOINT) {
                cancelled(cancel)?;
            }
            let (range_start, range_end) = read_pair(&mut file)?;
            if range_start >= start_s + SENTENCE_TURN_INSET_S
                && range_end <= end_s - SENTENCE_TURN_INSET_S
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn emit_overlay(
    output: &mut File,
    cover: &Cover,
    incoming: &Cover,
    inserted: &mut bool,
) -> Result<(), std::io::Error> {
    if cover.end <= incoming.start || cover.start >= incoming.end {
        if !*inserted && cover.start >= incoming.end {
            write_cover(output, incoming)?;
            *inserted = true;
        }
        return write_cover(output, cover);
    }
    if cover.start < incoming.start {
        write_cover(
            output,
            &Cover {
                start: cover.start,
                end: incoming.start,
                seq: cover.seq,
            },
        )?;
    }
    if !*inserted {
        write_cover(output, incoming)?;
        *inserted = true;
    }
    if cover.end > incoming.end {
        write_cover(
            output,
            &Cover {
                start: incoming.end,
                end: cover.end,
                seq: cover.seq,
            },
        )?;
    }
    Ok(())
}

fn write_cover(file: &mut File, cover: &Cover) -> Result<(), std::io::Error> {
    file.write_all(&cover.start.to_le_bytes())?;
    file.write_all(&cover.end.to_le_bytes())?;
    file.write_all(&cover.seq.to_le_bytes())
}

fn read_cover(file: &mut File) -> Result<Cover, std::io::Error> {
    let mut record = [0_u8; COVER_RECORD as usize];
    file.read_exact(&mut record)?;
    Ok(Cover {
        start: u64::from_le_bytes(record[0..8].try_into().expect("8 bytes")),
        end: u64::from_le_bytes(record[8..16].try_into().expect("8 bytes")),
        seq: u64::from_le_bytes(record[16..24].try_into().expect("8 bytes")),
    })
}

fn write_piece(
    file: &mut File,
    seq: u64,
    offset: usize,
    len: usize,
    reduced: bool,
) -> Result<(), std::io::Error> {
    file.write_all(&seq.to_le_bytes())?;
    file.write_all(&u64::try_from(offset).unwrap_or(0).to_le_bytes())?;
    file.write_all(&u64::try_from(len).unwrap_or(0).to_le_bytes())?;
    file.write_all(&(u64::from(reduced)).to_le_bytes())
}

fn read_piece_record(file: &mut File) -> Result<AudioPiece, std::io::Error> {
    let mut record = [0_u8; PIECE_RECORD as usize];
    file.read_exact(&mut record)?;
    Ok(AudioPiece {
        seq: u64::from_le_bytes(record[0..8].try_into().expect("8 bytes")),
        offset: u64_to_usize(u64::from_le_bytes(
            record[8..16].try_into().expect("8 bytes"),
        ))?,
        len: u64_to_usize(u64::from_le_bytes(
            record[16..24].try_into().expect("8 bytes"),
        ))?,
        reduced: u64::from_le_bytes(record[24..32].try_into().expect("8 bytes")) != 0,
    })
}

fn read_span(file: &mut File) -> Result<StoredSpan, std::io::Error> {
    let mut record = [0_u8; 32];
    file.read_exact(&mut record)?;
    Ok(StoredSpan {
        original_start_s: f64::from_le_bytes(record[0..8].try_into().expect("8 bytes")),
        original_end_s: f64::from_le_bytes(record[8..16].try_into().expect("8 bytes")),
        reduced_start_s: f64::from_le_bytes(record[16..24].try_into().expect("8 bytes")),
        reduced_end_s: f64::from_le_bytes(record[24..32].try_into().expect("8 bytes")),
    })
}

fn seconds_to_sample(seconds: f64, rate: f64) -> usize {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    (seconds * rate) as usize
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

fn read_write(path: impl AsRef<Path>) -> Result<File, std::io::Error> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
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

fn read_unit_record(file: &mut File) -> Result<UnitRecord, std::io::Error> {
    let mut record = [0_u8; 32];
    file.read_exact(&mut record)?;
    let origin = u64_to_usize(u64::from_le_bytes(
        record[0..8].try_into().expect("8 bytes"),
    ))?;
    let masked_len = u64_to_usize(u64::from_le_bytes(
        record[8..16].try_into().expect("8 bytes"),
    ))?;
    let reduced = u64::from_le_bytes(record[16..24].try_into().expect("8 bytes"));
    let map_count = u64_to_usize(u64::from_le_bytes(
        record[24..32].try_into().expect("8 bytes"),
    ))?;
    let reduced_len = if reduced == NONE_LEN {
        None
    } else {
        Some(u64_to_usize(reduced)?)
    };
    Ok(UnitRecord {
        origin,
        masked_len,
        reduced_len,
        map_count,
    })
}

fn u64_to_usize(value: u64) -> Result<usize, std::io::Error> {
    usize::try_from(value).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "unit length exceeds usize")
    })
}

fn read_pair(file: &mut File) -> Result<(f64, f64), std::io::Error> {
    let mut record = [0_u8; 16];
    file.read_exact(&mut record)?;
    Ok((
        f64::from_le_bytes(record[0..8].try_into().expect("8 bytes")),
        f64::from_le_bytes(record[8..16].try_into().expect("8 bytes")),
    ))
}

fn write_pair(file: &mut File, start: f64, end: f64) -> Result<(), std::io::Error> {
    file.write_all(&start.to_le_bytes())?;
    file.write_all(&end.to_le_bytes())
}

fn write_run(file: &mut File, offset: u64, count: u64) -> Result<(), std::io::Error> {
    file.write_all(&offset.to_le_bytes())?;
    file.write_all(&count.to_le_bytes())
}

fn read_run(file: &mut File) -> Result<(u64, u64), std::io::Error> {
    let mut record = [0_u8; 16];
    file.read_exact(&mut record)?;
    Ok((
        u64::from_le_bytes(record[0..8].try_into().expect("8 bytes")),
        u64::from_le_bytes(record[8..16].try_into().expect("8 bytes")),
    ))
}

fn copy_run(
    source: &Path,
    offset: u64,
    count: u64,
    dest: &mut File,
    cancel: &dyn Fn() -> bool,
) -> Result<u64, std::io::Error> {
    let mut input = File::open(source)?;
    input.seek(SeekFrom::Start(offset))?;
    for index in 0..count {
        if index.is_multiple_of(SCAN_CHECKPOINT) {
            cancelled(cancel)?;
        }
        let (start, end) = read_pair(&mut input)?;
        write_pair(dest, start, end)?;
    }
    Ok(count)
}

fn merge_runs(
    source: &Path,
    left_offset: u64,
    left_count: u64,
    right_offset: u64,
    right_count: u64,
    dest: &mut File,
    cancel: &dyn Fn() -> bool,
) -> Result<u64, std::io::Error> {
    let mut left = File::open(source)?;
    let mut right = File::open(source)?;
    left.seek(SeekFrom::Start(left_offset))?;
    right.seek(SeekFrom::Start(right_offset))?;
    let mut left_buf = Vec::new();
    let mut right_buf = Vec::new();
    let mut left_pos = 0_usize;
    let mut right_pos = 0_usize;
    let mut left_loaded = 0_u64;
    let mut right_loaded = 0_u64;
    let mut written = 0_u64;
    loop {
        if written.is_multiple_of(SCAN_CHECKPOINT) {
            cancelled(cancel)?;
        }
        if left_pos == left_buf.len() {
            if left_loaded == left_count {
                written += drain_rest(
                    &mut right,
                    right_count - right_loaded,
                    &right_buf[right_pos..],
                    dest,
                    cancel,
                )?;
                break;
            }
            let tile = (left_count - left_loaded).min(TURN_TILE);
            left_buf = read_records(&mut left, tile)?;
            left_loaded += tile;
            left_pos = 0;
        }
        if right_pos == right_buf.len() {
            if right_loaded == right_count {
                written += drain_rest(
                    &mut left,
                    left_count - left_loaded,
                    &left_buf[left_pos..],
                    dest,
                    cancel,
                )?;
                break;
            }
            let tile = (right_count - right_loaded).min(TURN_TILE);
            right_buf = read_records(&mut right, tile)?;
            right_loaded += tile;
            right_pos = 0;
        }
        let take_left = left_buf[left_pos].0 <= right_buf[right_pos].0;
        let (start, end) = if take_left {
            left_pos += 1;
            left_buf[left_pos - 1]
        } else {
            right_pos += 1;
            right_buf[right_pos - 1]
        };
        write_pair(dest, start, end)?;
        written += 1;
    }
    Ok(written)
}

fn drain_rest(
    file: &mut File,
    remaining_unread: u64,
    buffered: &[(f64, f64)],
    dest: &mut File,
    cancel: &dyn Fn() -> bool,
) -> Result<u64, std::io::Error> {
    let mut written = 0_u64;
    for (start, end) in buffered {
        if written.is_multiple_of(SCAN_CHECKPOINT) {
            cancelled(cancel)?;
        }
        write_pair(dest, *start, *end)?;
        written += 1;
    }
    for _ in 0..remaining_unread {
        if written.is_multiple_of(SCAN_CHECKPOINT) {
            cancelled(cancel)?;
        }
        let (start, end) = read_pair(file)?;
        write_pair(dest, start, end)?;
        written += 1;
    }
    Ok(written)
}

fn read_records(file: &mut File, count: u64) -> Result<Vec<(f64, f64)>, std::io::Error> {
    let mut records = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    for _ in 0..count {
        records.push(read_pair(file)?);
    }
    Ok(records)
}

fn overlap_merge(
    source: &Path,
    destination: &Path,
    cancel: &dyn Fn() -> bool,
) -> Result<u64, std::io::Error> {
    let mut input = File::open(source)?;
    let mut output = read_write(destination)?;
    let records = input.metadata()?.len() / 16;
    let mut last: Option<(f64, f64)> = None;
    let mut written = 0_u64;
    for index in 0..records {
        if index.is_multiple_of(SCAN_CHECKPOINT) {
            cancelled(cancel)?;
        }
        let (start, end) = read_pair(&mut input)?;
        last = Some(match last {
            Some((kept_start, kept_end)) if start <= kept_end => (kept_start, kept_end.max(end)),
            Some(previous) => {
                write_pair(&mut output, previous.0, previous.1)?;
                written += 1;
                (start, end)
            }
            None => (start, end),
        });
    }
    if let Some((start, end)) = last {
        write_pair(&mut output, start, end)?;
        written += 1;
    }
    output.sync_all()?;
    Ok(written)
}

fn truncate(file: &mut File) -> Result<(), std::io::Error> {
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)
}

#[cfg(all(test, feature = "full-tests"))]
thread_local! {
    static STATEMENT_MAX_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static STATEMENT_VISITED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static STATEMENT_WHOLE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(all(test, feature = "full-tests"))]
fn note_slice(len: usize) {
    STATEMENT_MAX_READ.with(|cell| {
        if len > cell.get() {
            cell.set(len);
        }
    });
    STATEMENT_VISITED.with(|cell| cell.set(cell.get().saturating_add(len)));
}

#[cfg(all(test, feature = "full-tests"))]
fn note_whole_statement() {
    STATEMENT_WHOLE.with(|cell| cell.set(cell.get().saturating_add(1)));
}

#[cfg(all(test, feature = "full-tests"))]
fn record_statement_probe(offset: usize, dest: &[f32]) {
    let Some(index) = crate::speakers::embedded_pcm_probe_index() else {
        return;
    };
    if index >= offset && index < offset + dest.len() {
        crate::speakers::record_embedded_pcm_probe_sample(dest[index - offset]);
    }
}

/// Clear the statement-read counters. Full tests share one thread.
#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn reset_statement_reads() {
    STATEMENT_MAX_READ.with(|cell| cell.set(0));
    STATEMENT_VISITED.with(|cell| cell.set(0));
    STATEMENT_WHOLE.with(|cell| cell.set(0));
}

/// Largest `read_statement_slice` destination since the last reset.
#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn statement_max_read() -> usize {
    STATEMENT_MAX_READ.with(|cell| cell.get())
}

/// Samples copied by `read_statement_slice` since the last reset.
#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn statement_visited() -> usize {
    STATEMENT_VISITED.with(|cell| cell.get())
}

/// Calls to the whole-statement PCM allocator since the last reset.
#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn statement_whole_reads() -> usize {
    STATEMENT_WHOLE.with(|cell| cell.get())
}

fn cancelled(cancel: &dyn Fn() -> bool) -> Result<(), std::io::Error> {
    if cancel() {
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "cancelled",
        ))
    } else {
        Ok(())
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    fn span(start: f64, end: f64) -> StoredSpan {
        StoredSpan {
            original_start_s: start,
            original_end_s: end,
            reduced_start_s: start,
            reduced_end_s: end,
        }
    }

    #[test]
    fn later_reduced_unit_replaces_the_overlap_without_a_resident_cover_list() {
        let scratch = tempfile::tempdir().expect("scratch");
        let mut spool = DraftSpool::open(scratch.path()).expect("spool");
        let rate = usize::try_from(SAMPLE_RATE).expect("rate");
        let first = vec![0.1_f32; rate];
        let second = vec![0.2_f32; rate];
        spool
            .store_unit_pcm(0, &first, Some((first.as_slice(), &[span(0.0, 1.0)])))
            .expect("first unit");
        spool
            .store_unit_pcm(
                rate / 2,
                &second,
                Some((second.as_slice(), &[span(0.0, 1.0)])),
            )
            .expect("second unit");
        let measured = spool
            .measure_statement_audio(0, rate + rate / 2, &|| false)
            .expect("measure");
        assert_eq!(measured.samples, rate + rate / 2);
        assert!(measured.reduced);
        let audio = spool
            .read_statement_audio(0, rate + rate / 2, &|| false)
            .expect("read");
        assert!(audio.reduced);
        assert_eq!(audio.samples.len(), rate + rate / 2);
        assert!(
            audio.samples[..rate / 2]
                .iter()
                .all(|sample| *sample == 0.1)
        );
        assert!(
            audio.samples[rate / 2..]
                .iter()
                .all(|sample| *sample == 0.2)
        );
    }

    #[test]
    fn unreduced_measure_counts_the_span_without_pieces() {
        let scratch = tempfile::tempdir().expect("scratch");
        let mut spool = DraftSpool::open(scratch.path()).expect("spool");
        let samples = vec![0.25_f32; 100];
        spool
            .store_unit_pcm(0, &samples, None)
            .expect("masked unit");
        let measured = spool
            .measure_statement_audio(0, 100, &|| false)
            .expect("measure");
        assert_eq!(measured.samples, 100);
        assert!(!measured.reduced);
        assert!(!spool.pieces_path().exists());
    }
}
