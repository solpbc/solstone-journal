// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded storage for the maintained global clustering calculation.
//!
//! Distance and linkage values stay on disk in fixed tiles. At most
//! [`CLUSTER_RESIDENT_TILES`] tiles and two embedding rows are resident.
//! Active flags, cluster sizes, merge records, and cut membership use the
//! same private directory and the same tile edge. Their resident bytes are
//! [`CLUSTER_LINKAGE_RESIDENT_BYTES`], independent of the interval count.
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

/// Ceiling, in bytes, of the linkage-metadata tiles. Flags, sizes, one merge
/// record, the two member-id buffers, the member index, and the label tiles
/// are all inside this number. It does not grow with the interval count.
/// Distance tiles are separate.
pub const CLUSTER_LINKAGE_RESIDENT_BYTES: usize = {
    let edge = CLUSTER_TILE_EDGE;
    let tiles = CLUSTER_RESIDENT_TILES;
    edge * tiles + edge * 8 * tiles + 32 + edge * 8 * 2 + edge * 16 * tiles + edge * 4 * tiles
};

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
        for _ in 0..8 {
            let seq = WORK_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("solstone-cluster-{}-{seq}", std::process::id()));
            if claim_cluster_dir(&path).is_ok() {
                return Ok(Self(path));
            }
        }
        Err(DiarizationError::MatrixStore {
            detail: "could not create a private cluster directory".to_owned(),
        })
    }
}

fn claim_cluster_dir(path: &Path) -> Result<(), std::io::Error> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "cluster scratch symlink",
            ));
        }
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "cluster scratch exists",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
    };
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    builder.create(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(std::io::Error::other(
            "cluster scratch is not a private directory",
        ));
    }
    Ok(())
}

fn private_file(path: &Path) -> Result<File, DiarizationError> {
    if let Ok(meta) = fs::symlink_metadata(path)
        && (meta.file_type().is_symlink() || meta.is_file())
    {
        return Err(DiarizationError::MatrixStore {
            detail: "cluster scratch file already exists".to_owned(),
        });
    }
    let mut options = File::options();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(store_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(store_error)?;
    }
    Ok(file)
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let Ok(meta) = fs::symlink_metadata(&self.0) else {
            return;
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return;
        }
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
        let file = private_file(&path)?;
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
        let file = private_file(&path)?;
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

fn tile_count(nodes: usize) -> Result<usize, DiarizationError> {
    nodes
        .div_ceil(CLUSTER_TILE_EDGE)
        .checked_mul(1)
        .filter(|tiles| tiles.checked_mul(CLUSTER_TILE_EDGE).is_some())
        .ok_or(DiarizationError::ShapeOverflow {
            rows: nodes,
            cols: 1,
        })
}

struct FlagTile {
    id: usize,
    values: [u8; CLUSTER_TILE_EDGE],
    dirty: bool,
}

struct SizeTile {
    id: usize,
    values: [u64; CLUSTER_TILE_EDGE],
    dirty: bool,
}

/// Active flags and cluster sizes for one linkage pass. Both arrays live in
/// the private work directory. Two tiles of each are resident.
pub(crate) struct NodeScratch {
    _dir: WorkDir,
    nodes: usize,
    flags_file: File,
    sizes_file: File,
    flags: [Option<FlagTile>; CLUSTER_RESIDENT_TILES],
    sizes: [Option<SizeTile>; CLUSTER_RESIDENT_TILES],
    flag_hand: usize,
    size_hand: usize,
}

impl NodeScratch {
    pub(crate) fn new(nodes: usize) -> Result<Self, DiarizationError> {
        let tiles = tile_count(nodes)?;
        let dir = WorkDir::new()?;
        let flags_file = create_len(&dir.0, "active.u8", tiles * CLUSTER_TILE_EDGE)?;
        let sizes_file = create_len(
            &dir.0,
            "sizes.u64le",
            tiles * CLUSTER_TILE_EDGE * std::mem::size_of::<u64>(),
        )?;
        Ok(Self {
            _dir: dir,
            nodes,
            flags_file,
            sizes_file,
            flags: std::array::from_fn(|_| None),
            sizes: std::array::from_fn(|_| None),
            flag_hand: 0,
            size_hand: 0,
        })
    }

    pub(crate) fn nodes(&self) -> usize {
        self.nodes
    }

    pub(crate) fn activate_singletons(
        &mut self,
        count: usize,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), DiarizationError> {
        for index in 0..count {
            if index % CLUSTER_TILE_EDGE == 0 && cancel() {
                return Err(DiarizationError::Cancelled);
            }
            self.set_active(index, true)?;
            self.set_size(index, 1)?;
        }
        Ok(())
    }

    pub(crate) fn is_active(&mut self, index: usize) -> Result<bool, DiarizationError> {
        self.flag(index).map(|value| value != 0)
    }

    pub(crate) fn set_active(
        &mut self,
        index: usize,
        active: bool,
    ) -> Result<(), DiarizationError> {
        self.set_flag(index, u8::from(active))
    }

    pub(crate) fn size(&mut self, index: usize) -> Result<usize, DiarizationError> {
        let slot = self.size_slot(index / CLUSTER_TILE_EDGE)?;
        let value = self.sizes[slot].as_ref().expect("size tile").values[index % CLUSTER_TILE_EDGE];
        usize::try_from(value).map_err(|_| DiarizationError::ShapeOverflow {
            rows: self.nodes,
            cols: 1,
        })
    }

    pub(crate) fn set_size(&mut self, index: usize, size: usize) -> Result<(), DiarizationError> {
        let size = u64::try_from(size).map_err(|_| DiarizationError::ShapeOverflow {
            rows: self.nodes,
            cols: 1,
        })?;
        let slot = self.size_slot(index / CLUSTER_TILE_EDGE)?;
        let tile = self.sizes[slot].as_mut().expect("size tile");
        tile.values[index % CLUSTER_TILE_EDGE] = size;
        tile.dirty = true;
        Ok(())
    }

    fn flag(&mut self, index: usize) -> Result<u8, DiarizationError> {
        let slot = self.flag_slot(index / CLUSTER_TILE_EDGE)?;
        Ok(self.flags[slot].as_ref().expect("flag tile").values[index % CLUSTER_TILE_EDGE])
    }

    fn set_flag(&mut self, index: usize, value: u8) -> Result<(), DiarizationError> {
        let slot = self.flag_slot(index / CLUSTER_TILE_EDGE)?;
        let tile = self.flags[slot].as_mut().expect("flag tile");
        tile.values[index % CLUSTER_TILE_EDGE] = value;
        tile.dirty = true;
        Ok(())
    }

    fn flag_slot(&mut self, id: usize) -> Result<usize, DiarizationError> {
        for index in 0..CLUSTER_RESIDENT_TILES {
            if self.flags[index].as_ref().is_some_and(|tile| tile.id == id) {
                return Ok(index);
            }
        }
        let index = self.flag_hand % CLUSTER_RESIDENT_TILES;
        self.flag_hand += 1;
        self.evict_flag(index)?;
        let mut values = [0_u8; CLUSTER_TILE_EDGE];
        read_at(
            &mut self.flags_file,
            (id * CLUSTER_TILE_EDGE) as u64,
            &mut values,
        )?;
        self.flags[index] = Some(FlagTile {
            id,
            values,
            dirty: false,
        });
        Ok(index)
    }

    fn evict_flag(&mut self, index: usize) -> Result<(), DiarizationError> {
        if let Some(tile) = self.flags[index].take()
            && tile.dirty
        {
            write_at(
                &mut self.flags_file,
                (tile.id * CLUSTER_TILE_EDGE) as u64,
                &tile.values,
            )?;
        }
        Ok(())
    }

    fn size_slot(&mut self, id: usize) -> Result<usize, DiarizationError> {
        for index in 0..CLUSTER_RESIDENT_TILES {
            if self.sizes[index].as_ref().is_some_and(|tile| tile.id == id) {
                return Ok(index);
            }
        }
        let index = self.size_hand % CLUSTER_RESIDENT_TILES;
        self.size_hand += 1;
        self.evict_size(index)?;
        let mut bytes = [0_u8; CLUSTER_TILE_EDGE * 8];
        read_at(
            &mut self.sizes_file,
            (id * CLUSTER_TILE_EDGE * 8) as u64,
            &mut bytes,
        )?;
        let mut values = [0_u64; CLUSTER_TILE_EDGE];
        for (slot, chunk) in values.iter_mut().zip(bytes.chunks_exact(8)) {
            *slot = u64::from_le_bytes(chunk.try_into().expect("eight bytes"));
        }
        self.sizes[index] = Some(SizeTile {
            id,
            values,
            dirty: false,
        });
        Ok(index)
    }

    fn evict_size(&mut self, index: usize) -> Result<(), DiarizationError> {
        if let Some(tile) = self.sizes[index].take()
            && tile.dirty
        {
            let mut bytes = [0_u8; CLUSTER_TILE_EDGE * 8];
            for (slot, value) in bytes.chunks_exact_mut(8).zip(tile.values) {
                slot.copy_from_slice(&value.to_le_bytes());
            }
            write_at(
                &mut self.sizes_file,
                (tile.id * CLUSTER_TILE_EDGE * 8) as u64,
                &bytes,
            )?;
        }
        Ok(())
    }
}

/// One average-linkage merge, stored privately and read one record at a time.
pub(crate) struct MergeLog {
    _dir: WorkDir,
    file: File,
    count: usize,
}

/// Fields of one merge record.
pub(crate) struct StoredMerge {
    pub(crate) left: usize,
    pub(crate) right: usize,
    pub(crate) distance: f64,
    pub(crate) size: usize,
}

impl MergeLog {
    pub(crate) fn new() -> Result<Self, DiarizationError> {
        let dir = WorkDir::new()?;
        let file = private_file(&dir.0.join("merges.bin"))?;
        Ok(Self {
            _dir: dir,
            file,
            count: 0,
        })
    }

    pub(crate) fn push(
        &mut self,
        left: usize,
        right: usize,
        distance: f64,
        size: usize,
    ) -> Result<(), DiarizationError> {
        let record = merge_bytes(left, right, distance, size)?;
        self.file
            .seek(SeekFrom::Start((self.count * 32) as u64))
            .map_err(store_error)?;
        self.file.write_all(&record).map_err(store_error)?;
        self.count += 1;
        Ok(())
    }

    pub(crate) fn len(&self) -> usize {
        self.count
    }

    pub(crate) fn get(&mut self, index: usize) -> Result<StoredMerge, DiarizationError> {
        let mut record = [0_u8; 32];
        read_at(&mut self.file, (index * 32) as u64, &mut record)?;
        Ok(StoredMerge {
            left: u64_at(&record, 0)?,
            right: u64_at(&record, 8)?,
            distance: f64::from_le_bytes(record[16..24].try_into().expect("eight bytes")),
            size: u64_at(&record, 24)?,
        })
    }
}

struct IndexTile {
    id: usize,
    offset: [u64; CLUSTER_TILE_EDGE],
    count: [u64; CLUSTER_TILE_EDGE],
    dirty: bool,
}

/// Cut membership. Each point stays in exactly one list. Lists and their
/// index live in the private work directory; a merge sorts through two
/// tiles of point ids.
pub(crate) struct MemberLists {
    _dir: WorkDir,
    data: File,
    index_file: File,
    data_end: u64,
    index: [Option<IndexTile>; CLUSTER_RESIDENT_TILES],
    index_hand: usize,
}

impl MemberLists {
    pub(crate) fn singletons(points: usize) -> Result<Self, DiarizationError> {
        let nodes = points.saturating_mul(2).saturating_sub(1).max(points);
        let tiles = tile_count(nodes.max(1))?;
        let dir = WorkDir::new()?;
        let data = private_file(&dir.0.join("members.u64le"))?;
        let index_file = create_len(&dir.0, "member-index.bin", tiles * CLUSTER_TILE_EDGE * 16)?;
        let mut lists = Self {
            _dir: dir,
            data,
            index_file,
            data_end: 0,
            index: std::array::from_fn(|_| None),
            index_hand: 0,
        };
        let mut buffer = [0_u64; CLUSTER_TILE_EDGE];
        let mut start = 0_usize;
        while start < points {
            let count = (points - start).min(CLUSTER_TILE_EDGE);
            for (local, slot) in buffer.iter_mut().take(count).enumerate() {
                *slot = (start + local) as u64;
            }
            let offset = lists.append(&buffer[..count])?;
            for local in 0..count {
                lists.index_set(start + local, offset + (local as u64) * 8, 1)?;
            }
            start += count;
        }
        Ok(lists)
    }

    pub(crate) fn count(&mut self, node: usize) -> Result<usize, DiarizationError> {
        Ok(self.index_get(node)?.1)
    }

    pub(crate) fn clear(&mut self, node: usize) -> Result<(), DiarizationError> {
        self.index_set(node, 0, 0)
    }

    pub(crate) fn first(&mut self, node: usize) -> Result<u64, DiarizationError> {
        let (offset, count) = self.index_get(node)?;
        if count == 0 {
            return Err(DiarizationError::MatrixStore {
                detail: "empty cluster has no first member".to_owned(),
            });
        }
        let mut value = [0_u64; 1];
        self.read_ids(offset, &mut value)?;
        Ok(value[0])
    }

    pub(crate) fn for_each(
        &mut self,
        node: usize,
        mut visit: impl FnMut(u64) -> Result<(), DiarizationError>,
    ) -> Result<(), DiarizationError> {
        let (mut offset, count) = self.index_get(node)?;
        let mut left = count;
        let mut buffer = [0_u64; CLUSTER_TILE_EDGE];
        while left > 0 {
            let take = left.min(CLUSTER_TILE_EDGE);
            self.read_ids(offset, &mut buffer[..take])?;
            for value in buffer.iter().take(take) {
                visit(*value)?;
            }
            offset += (take * 8) as u64;
            left -= take;
        }
        Ok(())
    }

    /// Move `left` and `right` into `dest`, sorted by point id.
    pub(crate) fn merge_sorted(
        &mut self,
        left: usize,
        right: usize,
        dest: usize,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), DiarizationError> {
        let (left_offset, left_count) = self.index_get(left)?;
        let (right_offset, right_count) = self.index_get(right)?;
        let combined = left_count + right_count;
        let unsorted = self.data_end;
        self.copy_ids(left_offset, left_count)?;
        self.copy_ids(right_offset, right_count)?;
        let sorted = self.sort_ids(unsorted, combined, cancel)?;
        self.index_set(dest, sorted, combined)?;
        self.index_set(left, 0, 0)?;
        self.index_set(right, 0, 0)?;
        Ok(())
    }

    fn sort_ids(
        &mut self,
        offset: u64,
        count: usize,
        cancel: &dyn Fn() -> bool,
    ) -> Result<u64, DiarizationError> {
        if count <= CLUSTER_TILE_EDGE {
            let mut buffer = [0_u64; CLUSTER_TILE_EDGE];
            if count > 0 {
                self.read_ids(offset, &mut buffer[..count])?;
                buffer[..count].sort_unstable();
            }
            return self.append(&buffer[..count]);
        }
        if cancel() {
            return Err(DiarizationError::Cancelled);
        }
        let mid = count / 2;
        let left = self.sort_ids(offset, mid, cancel)?;
        let right = self.sort_ids(offset + (mid as u64) * 8, count - mid, cancel)?;
        self.merge_ids(left, mid, right, count - mid)
    }

    fn merge_ids(
        &mut self,
        left: u64,
        left_count: usize,
        right: u64,
        right_count: usize,
    ) -> Result<u64, DiarizationError> {
        let output = self.data_end;
        let mut left_index = 0_usize;
        let mut right_index = 0_usize;
        let mut left_buffer = [0_u64; CLUSTER_TILE_EDGE];
        let mut right_buffer = [0_u64; CLUSTER_TILE_EDGE];
        let mut left_have = 0_usize;
        let mut left_pos = 0_usize;
        let mut right_have = 0_usize;
        let mut right_pos = 0_usize;
        let mut pending = [0_u64; CLUSTER_TILE_EDGE];
        let mut filled = 0_usize;
        while left_index < left_count || right_index < right_count {
            if left_pos == left_have && left_index < left_count {
                let take = (left_count - left_index).min(CLUSTER_TILE_EDGE);
                self.read_ids(left + (left_index as u64) * 8, &mut left_buffer[..take])?;
                left_have = take;
                left_pos = 0;
            }
            if right_pos == right_have && right_index < right_count {
                let take = (right_count - right_index).min(CLUSTER_TILE_EDGE);
                self.read_ids(right + (right_index as u64) * 8, &mut right_buffer[..take])?;
                right_have = take;
                right_pos = 0;
            }
            let take_left = if left_index >= left_count {
                false
            } else if right_index >= right_count {
                true
            } else {
                left_buffer[left_pos] <= right_buffer[right_pos]
            };
            pending[filled] = if take_left {
                let value = left_buffer[left_pos];
                left_pos += 1;
                left_index += 1;
                value
            } else {
                let value = right_buffer[right_pos];
                right_pos += 1;
                right_index += 1;
                value
            };
            filled += 1;
            if filled == CLUSTER_TILE_EDGE {
                self.append(&pending)?;
                filled = 0;
            }
        }
        if filled > 0 {
            self.append(&pending[..filled])?;
        }
        Ok(output)
    }

    fn copy_ids(&mut self, offset: u64, count: usize) -> Result<(), DiarizationError> {
        let mut left = count;
        let mut cursor = offset;
        let mut buffer = [0_u64; CLUSTER_TILE_EDGE];
        while left > 0 {
            let take = left.min(CLUSTER_TILE_EDGE);
            self.read_ids(cursor, &mut buffer[..take])?;
            self.append(&buffer[..take])?;
            cursor += (take * 8) as u64;
            left -= take;
        }
        Ok(())
    }

    fn append(&mut self, values: &[u64]) -> Result<u64, DiarizationError> {
        let offset = self.data_end;
        self.data
            .seek(SeekFrom::Start(offset))
            .map_err(store_error)?;
        for value in values {
            self.data
                .write_all(&value.to_le_bytes())
                .map_err(store_error)?;
        }
        self.data_end += (values.len() * 8) as u64;
        Ok(offset)
    }

    fn read_ids(&mut self, offset: u64, values: &mut [u64]) -> Result<(), DiarizationError> {
        let mut bytes = vec![0_u8; values.len() * 8];
        read_at(&mut self.data, offset, &mut bytes)?;
        for (slot, chunk) in values.iter_mut().zip(bytes.chunks_exact(8)) {
            *slot = u64::from_le_bytes(chunk.try_into().expect("eight bytes"));
        }
        Ok(())
    }

    fn index_get(&mut self, node: usize) -> Result<(u64, usize), DiarizationError> {
        let slot = self.index_slot(node / CLUSTER_TILE_EDGE)?;
        let local = node % CLUSTER_TILE_EDGE;
        let tile = self.index[slot].as_ref().expect("index tile");
        let count =
            usize::try_from(tile.count[local]).map_err(|_| DiarizationError::ShapeOverflow {
                rows: node,
                cols: 1,
            })?;
        Ok((tile.offset[local], count))
    }

    fn index_set(
        &mut self,
        node: usize,
        offset: u64,
        count: usize,
    ) -> Result<(), DiarizationError> {
        let count = u64::try_from(count).map_err(|_| DiarizationError::ShapeOverflow {
            rows: node,
            cols: 1,
        })?;
        let slot = self.index_slot(node / CLUSTER_TILE_EDGE)?;
        let local = node % CLUSTER_TILE_EDGE;
        let tile = self.index[slot].as_mut().expect("index tile");
        tile.offset[local] = offset;
        tile.count[local] = count;
        tile.dirty = true;
        Ok(())
    }

    fn index_slot(&mut self, id: usize) -> Result<usize, DiarizationError> {
        for index in 0..CLUSTER_RESIDENT_TILES {
            if self.index[index].as_ref().is_some_and(|tile| tile.id == id) {
                return Ok(index);
            }
        }
        let index = self.index_hand % CLUSTER_RESIDENT_TILES;
        self.index_hand += 1;
        self.evict_index(index)?;
        let mut bytes = vec![0_u8; CLUSTER_TILE_EDGE * 16];
        read_at(
            &mut self.index_file,
            (id * CLUSTER_TILE_EDGE * 16) as u64,
            &mut bytes,
        )?;
        let mut offset = [0_u64; CLUSTER_TILE_EDGE];
        let mut count = [0_u64; CLUSTER_TILE_EDGE];
        for (local, chunk) in bytes.chunks_exact(16).enumerate() {
            offset[local] = u64::from_le_bytes(chunk[0..8].try_into().expect("eight bytes"));
            count[local] = u64::from_le_bytes(chunk[8..16].try_into().expect("eight bytes"));
        }
        self.index[index] = Some(IndexTile {
            id,
            offset,
            count,
            dirty: false,
        });
        Ok(index)
    }

    fn evict_index(&mut self, index: usize) -> Result<(), DiarizationError> {
        if let Some(tile) = self.index[index].take()
            && tile.dirty
        {
            let mut bytes = vec![0_u8; CLUSTER_TILE_EDGE * 16];
            for local in 0..CLUSTER_TILE_EDGE {
                bytes[local * 16..local * 16 + 8]
                    .copy_from_slice(&tile.offset[local].to_le_bytes());
                bytes[local * 16 + 8..local * 16 + 16]
                    .copy_from_slice(&tile.count[local].to_le_bytes());
            }
            write_at(
                &mut self.index_file,
                (tile.id * CLUSTER_TILE_EDGE * 16) as u64,
                &bytes,
            )?;
        }
        Ok(())
    }
}

struct LabelTile {
    id: usize,
    values: [u32; CLUSTER_TILE_EDGE],
    dirty: bool,
}

/// Cluster labels. Two tiles are resident; the full vector stays in the
/// private work directory until the caller copies it out.
pub(crate) struct LabelFile {
    _dir: WorkDir,
    file: File,
    len: usize,
    slots: [Option<LabelTile>; CLUSTER_RESIDENT_TILES],
    hand: usize,
}

impl LabelFile {
    pub(crate) fn zeros(len: usize) -> Result<Self, DiarizationError> {
        let tiles = tile_count(len.max(1))?;
        let dir = WorkDir::new()?;
        let file = create_len(&dir.0, "labels.u32le", tiles * CLUSTER_TILE_EDGE * 4)?;
        Ok(Self {
            _dir: dir,
            file,
            len,
            slots: std::array::from_fn(|_| None),
            hand: 0,
        })
    }

    pub(crate) fn from_slice(labels: &[usize]) -> Result<Self, DiarizationError> {
        let mut file = Self::zeros(labels.len())?;
        for (index, label) in labels.iter().enumerate() {
            file.set(index, *label)?;
        }
        Ok(file)
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn get(&mut self, index: usize) -> Result<usize, DiarizationError> {
        let slot = self.slot(index / CLUSTER_TILE_EDGE)?;
        Ok(
            self.slots[slot].as_ref().expect("label tile").values[index % CLUSTER_TILE_EDGE]
                as usize,
        )
    }

    pub(crate) fn set(&mut self, index: usize, label: usize) -> Result<(), DiarizationError> {
        let label = u32::try_from(label).map_err(|_| DiarizationError::ShapeOverflow {
            rows: self.len,
            cols: 1,
        })?;
        let slot = self.slot(index / CLUSTER_TILE_EDGE)?;
        let tile = self.slots[slot].as_mut().expect("label tile");
        tile.values[index % CLUSTER_TILE_EDGE] = label;
        tile.dirty = true;
        Ok(())
    }

    pub(crate) fn to_vec(&mut self) -> Result<Vec<usize>, DiarizationError> {
        let mut labels = Vec::with_capacity(self.len);
        for index in 0..self.len {
            labels.push(self.get(index)?);
        }
        Ok(labels)
    }

    pub(crate) fn write_to(
        &mut self,
        path: &Path,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), DiarizationError> {
        self.flush()?;
        let mut output = File::create(path).map_err(store_error)?;
        self.file.seek(SeekFrom::Start(0)).map_err(store_error)?;
        let mut left = (self.len * 4) as u64;
        let mut buffer = [0_u8; 64 * 1024];
        while left > 0 {
            if cancel() {
                return Err(DiarizationError::Cancelled);
            }
            let take = std::cmp::min(buffer.len() as u64, left) as usize;
            self.file
                .read_exact(&mut buffer[..take])
                .map_err(store_error)?;
            output.write_all(&buffer[..take]).map_err(store_error)?;
            left -= take as u64;
        }
        output.sync_all().map_err(store_error)
    }

    pub(crate) fn count_label(
        &mut self,
        label: usize,
        except: Option<usize>,
    ) -> Result<usize, DiarizationError> {
        let mut count = 0_usize;
        for index in 0..self.len {
            if except == Some(index) {
                continue;
            }
            if self.get(index)? == label {
                count += 1;
            }
        }
        Ok(count)
    }

    pub(crate) fn unique(&mut self) -> Result<Vec<usize>, DiarizationError> {
        let mut found = std::collections::BTreeSet::new();
        for index in 0..self.len {
            found.insert(self.get(index)?);
        }
        Ok(found.into_iter().collect())
    }

    fn flush(&mut self) -> Result<(), DiarizationError> {
        for index in 0..CLUSTER_RESIDENT_TILES {
            self.evict(index)?;
        }
        Ok(())
    }

    fn slot(&mut self, id: usize) -> Result<usize, DiarizationError> {
        for index in 0..CLUSTER_RESIDENT_TILES {
            if self.slots[index].as_ref().is_some_and(|tile| tile.id == id) {
                return Ok(index);
            }
        }
        let index = self.hand % CLUSTER_RESIDENT_TILES;
        self.hand += 1;
        self.evict(index)?;
        let mut bytes = [0_u8; CLUSTER_TILE_EDGE * 4];
        read_at(
            &mut self.file,
            (id * CLUSTER_TILE_EDGE * 4) as u64,
            &mut bytes,
        )?;
        let mut values = [0_u32; CLUSTER_TILE_EDGE];
        for (slot, chunk) in values.iter_mut().zip(bytes.chunks_exact(4)) {
            *slot = u32::from_le_bytes(chunk.try_into().expect("four bytes"));
        }
        self.slots[index] = Some(LabelTile {
            id,
            values,
            dirty: false,
        });
        Ok(index)
    }

    fn evict(&mut self, index: usize) -> Result<(), DiarizationError> {
        if let Some(tile) = self.slots[index].take()
            && tile.dirty
        {
            let mut bytes = [0_u8; CLUSTER_TILE_EDGE * 4];
            for (slot, value) in bytes.chunks_exact_mut(4).zip(tile.values) {
                slot.copy_from_slice(&value.to_le_bytes());
            }
            write_at(
                &mut self.file,
                (tile.id * CLUSTER_TILE_EDGE * 4) as u64,
                &bytes,
            )?;
        }
        Ok(())
    }
}

fn create_len(dir: &Path, name: &str, bytes: usize) -> Result<File, DiarizationError> {
    let file = private_file(&dir.join(name))?;
    file.set_len(bytes as u64).map_err(store_error)?;
    Ok(file)
}

fn read_at(file: &mut File, offset: u64, buffer: &mut [u8]) -> Result<(), DiarizationError> {
    file.seek(SeekFrom::Start(offset)).map_err(store_error)?;
    file.read_exact(buffer).map_err(store_error)
}

fn write_at(file: &mut File, offset: u64, buffer: &[u8]) -> Result<(), DiarizationError> {
    file.seek(SeekFrom::Start(offset)).map_err(store_error)?;
    file.write_all(buffer).map_err(store_error)
}

fn merge_bytes(
    left: usize,
    right: usize,
    distance: f64,
    size: usize,
) -> Result<[u8; 32], DiarizationError> {
    let left = u64::try_from(left).map_err(|_| DiarizationError::ShapeOverflow {
        rows: left,
        cols: 1,
    })?;
    let right = u64::try_from(right).map_err(|_| DiarizationError::ShapeOverflow {
        rows: right,
        cols: 1,
    })?;
    let size = u64::try_from(size).map_err(|_| DiarizationError::ShapeOverflow {
        rows: size,
        cols: 1,
    })?;
    let mut record = [0_u8; 32];
    record[0..8].copy_from_slice(&left.to_le_bytes());
    record[8..16].copy_from_slice(&right.to_le_bytes());
    record[16..24].copy_from_slice(&distance.to_le_bytes());
    record[24..32].copy_from_slice(&size.to_le_bytes());
    Ok(record)
}

fn u64_at(record: &[u8], offset: usize) -> Result<usize, DiarizationError> {
    let value = u64::from_le_bytes(record[offset..offset + 8].try_into().expect("eight bytes"));
    usize::try_from(value).map_err(|_| DiarizationError::ShapeOverflow {
        rows: offset,
        cols: 1,
    })
}

#[cfg(test)]
mod linkage_tests {
    use super::*;

    #[test]
    fn linkage_resident_budget_does_not_mention_the_interval_count() {
        assert_eq!(CLUSTER_LINKAGE_RESIDENT_BYTES, 2400);
        assert!(CLUSTER_LINKAGE_RESIDENT_BYTES < 64 * 1024);
    }

    #[test]
    fn merged_membership_matches_an_in_memory_sort() {
        let mut lists = MemberLists::singletons(5).unwrap();
        lists.merge_sorted(0, 2, 5, &|| false).unwrap();
        lists.merge_sorted(3, 4, 6, &|| false).unwrap();
        lists.merge_sorted(5, 6, 7, &|| false).unwrap();
        let mut members = Vec::new();
        lists
            .for_each(7, |id| {
                members.push(id);
                Ok(())
            })
            .unwrap();
        assert_eq!(members, vec![0, 2, 3, 4]);
        assert_eq!(lists.first(7).unwrap(), 0);
        assert_eq!(lists.count(0).unwrap(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn cluster_scratch_is_owner_only_and_does_not_follow_a_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = std::env::temp_dir().join(format!(
            "solstone-cluster-privacy-{}-{}",
            std::process::id(),
            WORK_DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let planted = root.join("planted");
        let target = root.join("target");
        fs::create_dir(&target).unwrap();
        symlink(&target, &planted).unwrap();
        let error = claim_cluster_dir(&planted).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(fs::read_dir(&target).unwrap().next().is_none());
        assert!(
            fs::symlink_metadata(&planted)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        let created = root.join("private");
        claim_cluster_dir(&created).unwrap();
        let directory = fs::symlink_metadata(&created).unwrap();
        assert!(directory.is_dir());
        assert_eq!(directory.permissions().mode() & 0o077, 0);
        let file_path = created.join("rows.bin");
        let file = private_file(&file_path).unwrap();
        drop(file);
        let file_meta = fs::symlink_metadata(&file_path).unwrap();
        assert_eq!(file_meta.permissions().mode() & 0o077, 0);
        let work = WorkDir(created);
        drop(work);
        assert!(!root.join("private").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
