// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded storage for the maintained global clustering calculation.
//!
//! Distance and linkage values stay on disk in fixed tiles. At most
//! [`CLUSTER_RESIDENT_TILES`] tiles and two embedding rows are resident.
//! A tile write, a tile load, and a tile-row of the pair scan are cancellation
//! points inside the matrix pass. The cosine definitions are unchanged:
//! agglomerative clustering uses unclipped `1 - dot`, and silhouette uses
//! `max(1 - dot, 0)`.

use std::cell::Cell;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::diarization::DiarizationError;

pub const CLUSTER_TILE_EDGE: usize = 32;
pub const CLUSTER_RESIDENT_TILES: usize = 2;

thread_local! {
    static TILE_WORK_UNITS: Cell<u64> = const { Cell::new(0) };
}
static WORK_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// Bytes of matrix tiles plus the two cached embedding rows.
#[cfg(test)]
pub fn cluster_matrix_resident_bytes(cols: usize) -> usize {
    let tile =
        CLUSTER_TILE_EDGE * CLUSTER_TILE_EDGE * std::mem::size_of::<f64>() * CLUSTER_RESIDENT_TILES;
    let rows = cols.saturating_mul(std::mem::size_of::<f64>()) * 2;
    tile + rows
}

/// Tile materializations on this thread since the last take.
#[cfg(test)]
pub fn take_cluster_tile_work_units() -> u64 {
    TILE_WORK_UNITS.with(|cell| cell.replace(0))
}

fn note_tile_work_unit() {
    TILE_WORK_UNITS.with(|cell| cell.set(cell.get().saturating_add(1)));
}

struct WorkDir(PathBuf);

impl WorkDir {
    fn new() -> Result<Self, DiarizationError> {
        let seq = WORK_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("solstone-cluster-{}-{seq}", std::process::id()));
        fs::create_dir(&path).map_err(|error| DiarizationError::MatrixStore {
            detail: error.to_string(),
        })?;
        Ok(Self(path))
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) struct BoundRows {
    _dir: WorkDir,
    file: File,
    rows: usize,
    cols: usize,
    cache: [Option<(usize, Vec<f64>)>; 2],
}

impl BoundRows {
    pub(crate) fn from_f32(
        embeddings: &[f32],
        rows: usize,
        cols: usize,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Self, DiarizationError> {
        let mut store = Self::create(rows, cols)?;
        let mut row = vec![0.0_f64; cols];
        for index in 0..rows {
            if index % CLUSTER_TILE_EDGE == 0 {
                stop(cancel)?;
            }
            let start = index * cols;
            normalize_into(&embeddings[start..start + cols], &mut row);
            store.write_row(index, &row)?;
        }
        Ok(store)
    }

    pub(crate) fn from_f32le(
        path: &Path,
        rows: usize,
        cols: usize,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Self, DiarizationError> {
        let mut input = File::open(path).map_err(|error| DiarizationError::MatrixStore {
            detail: error.to_string(),
        })?;
        let mut store = Self::create(rows, cols)?;
        let mut bytes = vec![0_u8; cols * std::mem::size_of::<f32>()];
        let mut decoded = vec![0.0_f32; cols];
        let mut row = vec![0.0_f64; cols];
        for index in 0..rows {
            if index % CLUSTER_TILE_EDGE == 0 {
                stop(cancel)?;
            }
            input
                .read_exact(&mut bytes)
                .map_err(|error| DiarizationError::MatrixStore {
                    detail: error.to_string(),
                })?;
            for (slot, chunk) in decoded.iter_mut().zip(bytes.chunks_exact(4)) {
                *slot = f32::from_le_bytes(chunk.try_into().expect("four bytes"));
            }
            normalize_into(&decoded, &mut row);
            store.write_row(index, &row)?;
        }
        Ok(store)
    }

    fn create(rows: usize, cols: usize) -> Result<Self, DiarizationError> {
        let dir = WorkDir::new()?;
        let path = dir.0.join("rows.f64le");
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| DiarizationError::MatrixStore {
                detail: error.to_string(),
            })?;
        let bytes = rows
            .checked_mul(cols)
            .and_then(|cells| cells.checked_mul(std::mem::size_of::<f64>()))
            .ok_or(DiarizationError::ShapeOverflow { rows, cols })?;
        file.set_len(bytes as u64)
            .map_err(|error| DiarizationError::MatrixStore {
                detail: error.to_string(),
            })?;
        Ok(Self {
            _dir: dir,
            file,
            rows,
            cols,
            cache: [None, None],
        })
    }

    fn write_row(&mut self, index: usize, row: &[f64]) -> Result<(), DiarizationError> {
        let offset = (index * self.cols * std::mem::size_of::<f64>()) as u64;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(store_error)?;
        let mut bytes = Vec::with_capacity(row.len() * 8);
        for value in row {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        self.file.write_all(&bytes).map_err(store_error)?;
        self.cache[index % 2] = Some((index, row.to_vec()));
        Ok(())
    }

    pub(crate) fn dot(&mut self, left: usize, right: usize) -> Result<f64, DiarizationError> {
        self.load(left, 0)?;
        self.load(right, 1)?;
        let left_row = &self.cache[0].as_ref().expect("left row").1;
        let right_row = &self.cache[1].as_ref().expect("right row").1;
        Ok(left_row.iter().zip(right_row).map(|(a, b)| a * b).sum())
    }

    fn load(&mut self, index: usize, slot: usize) -> Result<(), DiarizationError> {
        if index >= self.rows {
            return Err(DiarizationError::MatrixStore {
                detail: format!("row {index} is outside {}", self.rows),
            });
        }
        if self.cache[slot]
            .as_ref()
            .is_some_and(|(cached, _)| *cached == index)
        {
            return Ok(());
        }
        if self.cache[1 - slot]
            .as_ref()
            .is_some_and(|(cached, _)| *cached == index)
        {
            self.cache.swap(slot, 1 - slot);
            return Ok(());
        }
        let offset = (index * self.cols * std::mem::size_of::<f64>()) as u64;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(store_error)?;
        let mut bytes = vec![0_u8; self.cols * 8];
        self.file.read_exact(&mut bytes).map_err(store_error)?;
        let mut row = Vec::with_capacity(self.cols);
        for chunk in bytes.chunks_exact(8) {
            row.push(f64::from_le_bytes(chunk.try_into().expect("eight bytes")));
        }
        self.cache[slot] = Some((index, row));
        Ok(())
    }

    pub(crate) fn unclipped(&mut self, left: usize, right: usize) -> Result<f64, DiarizationError> {
        if left == right {
            return Ok(0.0);
        }
        Ok(1.0 - self.dot(left, right)?)
    }

    pub(crate) fn clipped(&mut self, left: usize, right: usize) -> Result<f64, DiarizationError> {
        if left == right {
            return Ok(0.0);
        }
        Ok((1.0 - self.dot(left, right)?).max(0.0))
    }
}

fn normalize_into(row: &[f32], out: &mut [f64]) {
    let norm = row
        .iter()
        .map(|value| {
            let value = f64::from(*value);
            value * value
        })
        .sum::<f64>()
        .sqrt();
    let denom = if norm > crate::diarization::row_norm_epsilon() {
        norm
    } else {
        1.0
    };
    for (slot, value) in out.iter_mut().zip(row) {
        *slot = f64::from(*value) / denom;
    }
}

struct TileSlot {
    id: usize,
    values: Vec<f64>,
    dirty: bool,
}

pub(crate) struct LinkageTiles {
    _dir: WorkDir,
    stride: usize,
    per_side: usize,
    file: File,
    slots: [Option<TileSlot>; CLUSTER_RESIDENT_TILES],
    hand: usize,
}

impl LinkageTiles {
    pub(crate) fn from_unclipped(
        rows: &mut BoundRows,
        n: usize,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Self, DiarizationError> {
        let mut store = Self::allocate(2 * n - 1)?;
        let mut tile = vec![0.0_f64; CLUSTER_TILE_EDGE * CLUSTER_TILE_EDGE];
        let tile_count = store.per_side * store.per_side;
        for id in 0..tile_count {
            stop(cancel)?;
            store.fill_tile(id, &mut tile, |row, col| {
                if row < n && col < n {
                    rows.unclipped(row, col)
                } else {
                    Ok(f64::INFINITY)
                }
            })?;
            store.write_raw(id, &tile)?;
            note_tile_work_unit();
        }
        Ok(store)
    }

    /// Load a caller-supplied `n × n` distance block into the same tiled store.
    ///
    /// Cells outside that block stay infinite, matching the linkage matrix the
    /// agglomerative pass used to allocate in one buffer.
    #[cfg(test)]
    pub(crate) fn from_square(
        distances: &[f64],
        n: usize,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Self, DiarizationError> {
        let expected = n
            .checked_mul(n)
            .ok_or(DiarizationError::ShapeOverflow { rows: n, cols: n })?;
        if distances.len() != expected {
            return Err(DiarizationError::EmbeddingShapeMismatch {
                rows: n,
                cols: n,
                len: distances.len(),
            });
        }
        let mut store = Self::allocate(2 * n - 1)?;
        let mut tile = vec![0.0_f64; CLUSTER_TILE_EDGE * CLUSTER_TILE_EDGE];
        let tile_count = store.per_side * store.per_side;
        for id in 0..tile_count {
            stop(cancel)?;
            store.fill_tile(id, &mut tile, |row, col| {
                Ok(if row < n && col < n {
                    distances[row * n + col]
                } else {
                    f64::INFINITY
                })
            })?;
            store.write_raw(id, &tile)?;
            note_tile_work_unit();
        }
        Ok(store)
    }

    fn allocate(stride: usize) -> Result<Self, DiarizationError> {
        let dir = WorkDir::new()?;
        let path = dir.0.join("linkage.f64le");
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(store_error)?;
        let per_side = stride.div_ceil(CLUSTER_TILE_EDGE);
        let tile_cells = CLUSTER_TILE_EDGE * CLUSTER_TILE_EDGE;
        let bytes = per_side
            .checked_mul(per_side)
            .and_then(|tiles| tiles.checked_mul(tile_cells))
            .and_then(|cells| cells.checked_mul(std::mem::size_of::<f64>()))
            .ok_or(DiarizationError::ShapeOverflow {
                rows: stride,
                cols: stride,
            })?;
        file.set_len(bytes as u64).map_err(store_error)?;
        Ok(Self {
            _dir: dir,
            stride,
            per_side,
            file,
            slots: std::array::from_fn(|_| None),
            hand: 0,
        })
    }

    fn fill_tile(
        &self,
        id: usize,
        tile: &mut [f64],
        mut cell: impl FnMut(usize, usize) -> Result<f64, DiarizationError>,
    ) -> Result<(), DiarizationError> {
        let tile_row = id / self.per_side;
        let tile_col = id % self.per_side;
        for local_row in 0..CLUSTER_TILE_EDGE {
            for local_col in 0..CLUSTER_TILE_EDGE {
                let row = tile_row * CLUSTER_TILE_EDGE + local_row;
                let col = tile_col * CLUSTER_TILE_EDGE + local_col;
                tile[local_row * CLUSTER_TILE_EDGE + local_col] = cell(row, col)?;
            }
        }
        Ok(())
    }

    pub(crate) fn get(&mut self, row: usize, col: usize) -> Result<f64, DiarizationError> {
        let id = self.tile_id(row, col);
        let slot = self.slot_for(id)?;
        let local = self.local(row, col);
        Ok(self.slots[slot].as_ref().expect("tile").values[local])
    }

    pub(crate) fn set(
        &mut self,
        row: usize,
        col: usize,
        value: f64,
    ) -> Result<(), DiarizationError> {
        let id = self.tile_id(row, col);
        let slot = self.slot_for(id)?;
        let local = self.local(row, col);
        let tile = self.slots[slot].as_mut().expect("tile");
        tile.values[local] = value;
        tile.dirty = true;
        Ok(())
    }

    fn tile_id(&self, row: usize, col: usize) -> usize {
        (row / CLUSTER_TILE_EDGE) * self.per_side + (col / CLUSTER_TILE_EDGE)
    }

    fn local(&self, row: usize, col: usize) -> usize {
        (row % CLUSTER_TILE_EDGE) * CLUSTER_TILE_EDGE + (col % CLUSTER_TILE_EDGE)
    }

    fn slot_for(&mut self, id: usize) -> Result<usize, DiarizationError> {
        for index in 0..CLUSTER_RESIDENT_TILES {
            if self.slots[index].as_ref().is_some_and(|slot| slot.id == id) {
                return Ok(index);
            }
        }
        let index = self.hand % CLUSTER_RESIDENT_TILES;
        self.hand += 1;
        self.evict(index)?;
        self.load(index, id)?;
        note_tile_work_unit();
        Ok(index)
    }

    fn evict(&mut self, index: usize) -> Result<(), DiarizationError> {
        if let Some(slot) = self.slots[index].take()
            && slot.dirty
        {
            self.write_raw(slot.id, &slot.values)?;
        }
        Ok(())
    }

    fn load(&mut self, index: usize, id: usize) -> Result<(), DiarizationError> {
        let mut values = vec![0.0_f64; CLUSTER_TILE_EDGE * CLUSTER_TILE_EDGE];
        self.read_raw(id, &mut values)?;
        self.slots[index] = Some(TileSlot {
            id,
            values,
            dirty: false,
        });
        Ok(())
    }

    fn write_raw(&mut self, id: usize, values: &[f64]) -> Result<(), DiarizationError> {
        let offset =
            (id * CLUSTER_TILE_EDGE * CLUSTER_TILE_EDGE * std::mem::size_of::<f64>()) as u64;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(store_error)?;
        let mut bytes = Vec::with_capacity(values.len() * 8);
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        self.file.write_all(&bytes).map_err(store_error)
    }

    fn read_raw(&mut self, id: usize, values: &mut [f64]) -> Result<(), DiarizationError> {
        let offset = (id * values.len() * std::mem::size_of::<f64>()) as u64;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(store_error)?;
        let mut bytes = vec![0_u8; values.len() * 8];
        self.file.read_exact(&mut bytes).map_err(store_error)?;
        for (slot, chunk) in values.iter_mut().zip(bytes.chunks_exact(8)) {
            *slot = f64::from_le_bytes(chunk.try_into().expect("eight bytes"));
        }
        Ok(())
    }

    pub(crate) fn stride(&self) -> usize {
        self.stride
    }
}

fn stop(cancel: &dyn Fn() -> bool) -> Result<(), DiarizationError> {
    if cancel() {
        Err(DiarizationError::Cancelled)
    } else {
        Ok(())
    }
}

fn store_error(error: std::io::Error) -> DiarizationError {
    DiarizationError::MatrixStore {
        detail: error.to_string(),
    }
}
