// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reading a location-accuracy pack: the venue file or the containment file an
//! owner may download. `scripts/build_location_accuracy_packs.py` documents
//! the format and builds both.
//!
//! A pack is a tile directory plus compressed blocks. Only the header and the
//! directory are held in memory; a lookup reads and decodes the few blocks
//! around one position, and a small cache keeps the last ones decoded.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Deserialize;

const MAGIC: &[u8; 4] = b"SLPK";
const FORMAT_VERSION: u16 = 1;
const MAX_ZOOM: u32 = 16;
const DIRECTORY_ENTRY: usize = 20;
const CACHED_BLOCKS: usize = 256;
const METRES_PER_DEGREE: f64 = 111_195.0;

/// A pack could not be read.
#[derive(Debug)]
pub enum PackError {
    Io(io::Error),
    Format(String),
}

impl fmt::Display for PackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "location pack could not be read: {error}"),
            Self::Format(detail) => write!(formatter, "location pack is malformed: {detail}"),
        }
    }
}

impl std::error::Error for PackError {}

impl From<io::Error> for PackError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn malformed(detail: impl Into<String>) -> PackError {
    PackError::Format(detail.into())
}

/// Which of the two files this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackKind {
    Venues,
    Containment,
}

#[derive(Debug, Clone, Deserialize)]
struct Header {
    kind: String,
    version: String,
    #[serde(default)]
    licence: String,
    #[serde(default)]
    attribution: String,
    /// Licence notices that travel with the data, in full.
    #[serde(default)]
    notices: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    classes: Vec<[String; 3]>,
    #[serde(default)]
    tolerance_m: BTreeMap<String, f64>,
}

/// Where a containment feature comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    /// A neighbourhood, town or city boundary.
    Division,
    /// A named area: a park, campus, plaza, stadium grounds.
    LandUse,
    Airport,
    Building,
}

impl Layer {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "division" => Some(Self::Division),
            "land_use" => Some(Self::LandUse),
            "airport" => Some(Self::Airport),
            "building" => Some(Self::Building),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Division => "division",
            Self::LandUse => "land_use",
            Self::Airport => "airport",
            Self::Building => "building",
        }
    }
}

/// One named place from the venue file.
#[derive(Debug, Clone, PartialEq)]
pub struct Venue {
    pub name: String,
    /// Overture's basic category, e.g. `coffee_shop`; empty when it had none.
    pub category: String,
    /// The source's confidence that the place exists, 0–100. Not a claim
    /// about anyone being there.
    pub confidence: u8,
    pub latitude: f64,
    pub longitude: f64,
}

/// One named outline from the containment file.
#[derive(Debug, Clone, PartialEq)]
pub struct Feature {
    pub layer: Layer,
    pub subtype: String,
    pub class: String,
    pub name: String,
    /// West, south, east, north in degrees.
    pub bounds: [f64; 4],
    /// Polygons of rings of `[longitude, latitude]`; each polygon's first ring
    /// is its outline and the rest are holes.
    pub polygons: Vec<Vec<Vec<[f64; 2]>>>,
    /// The most any edge moved when the outline was simplified.
    pub tolerance_m: f64,
}

impl Feature {
    /// Rough area in square degrees, for choosing the smallest of several.
    pub fn extent(&self) -> f64 {
        (self.bounds[2] - self.bounds[0]) * (self.bounds[3] - self.bounds[1])
    }

    /// Whether the whole disc of `radius_m` around the point lies inside this
    /// outline, after widening the disc by the simplification tolerance.
    pub fn contains_disc(&self, latitude: f64, longitude: f64, radius_m: f64) -> bool {
        let radius = radius_m + self.tolerance_m;
        let scale_x = METRES_PER_DEGREE * latitude.to_radians().cos();
        let project = |point: &[f64; 2]| {
            (
                (point[0] - longitude) * scale_x,
                (point[1] - latitude) * METRES_PER_DEGREE,
            )
        };
        self.polygons.iter().any(|polygon| {
            let mut inside = false;
            for ring in polygon {
                if ring.len() < 3 {
                    continue;
                }
                let mut previous = project(&ring[ring.len() - 1]);
                for point in ring {
                    let current = project(point);
                    if (current.1 > 0.0) != (previous.1 > 0.0) {
                        let crossing = previous.0
                            + (0.0 - previous.1) * (current.0 - previous.0)
                                / (current.1 - previous.1);
                        if crossing > 0.0 {
                            inside = !inside;
                        }
                    }
                    if distance_to_segment(previous, current) < radius {
                        return false;
                    }
                    previous = current;
                }
            }
            inside
        })
    }
}

fn distance_to_segment(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length == 0.0 {
        0.0
    } else {
        ((-a.0) * dx + (-a.1) * dy) / length
    }
    .clamp(0.0, 1.0);
    let (x, y) = (a.0 + t * dx, a.1 + t * dy);
    (x * x + y * y).sqrt()
}

/// Great-circle distance in metres.
pub fn distance_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (phi1, phi2) = (lat1.to_radians(), lat2.to_radians());
    let dphi = (lat2 - lat1).to_radians();
    let dlambda = (lon2 - lon1).to_radians();
    let a = (dphi / 2.0).sin().powi(2) + phi1.cos() * phi2.cos() * (dlambda / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * a.sqrt().atan2((1.0 - a).sqrt())
}

/// An outline's header, kept per cached block; its geometry stays encoded
/// in the block until a lookup needs it.
struct FeatureHead {
    class: usize,
    name: String,
    bounds: [f64; 4],
    geometry: std::ops::Range<usize>,
}

enum Block {
    Venues(Vec<Venue>),
    Features {
        raw: Vec<u8>,
        heads: Vec<FeatureHead>,
    },
}

enum Source {
    File(Mutex<File>),
    Bytes(Vec<u8>),
}

/// An opened pack.
pub struct Pack {
    kind: PackKind,
    header: Header,
    /// (tile key, block offset, block length), sorted by key.
    directory: Vec<(u64, u64, u32)>,
    source: Source,
    cache: Mutex<VecDeque<(u64, Arc<Block>)>>,
}

impl fmt::Debug for Pack {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Pack")
            .field("kind", &self.kind)
            .field("version", &self.header.version)
            .field("tiles", &self.directory.len())
            .finish()
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], PackError> {
        let end = self
            .at
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| malformed("block ends early"))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn varint(&mut self) -> Result<u64, PackError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.take(1)?[0];
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(malformed("varint too long"))
    }

    fn count(&mut self) -> Result<usize, PackError> {
        let value = self.varint()?;
        // Every counted thing takes at least one byte.
        usize::try_from(value)
            .ok()
            .filter(|count| *count <= self.bytes.len())
            .ok_or_else(|| malformed("count larger than the block"))
    }

    fn zigzag(&mut self) -> Result<i64, PackError> {
        let value = self.varint()?;
        Ok(((value >> 1) as i64) ^ -((value & 1) as i64))
    }

    fn text(&mut self) -> Result<String, PackError> {
        let length = self.count()?;
        String::from_utf8(self.take(length)?.to_vec()).map_err(|_| malformed("name is not UTF-8"))
    }
}

fn degrees(micro: i64) -> f64 {
    micro as f64 / 1_000_000.0
}

fn tile_key(z: u32, x: u64, y: u64) -> u64 {
    (u64::from(z) << 56) | (x << 28) | y
}

#[cfg(test)]
thread_local! { pub static PACK_OPEN_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

impl Pack {
    /// Open a pack file, reading only its header and tile directory.
    pub fn open(path: &Path) -> Result<Self, PackError> {
        #[cfg(test)]
        PACK_OPEN_CALLS.with(|count| count.set(count.get() + 1));
        Self::open_file(File::open(path)?)
    }

    /// Verify all bytes and parse that same open file, before native pack use.
    pub fn open_verified(path: &Path, sha256: &str, size_bytes: u64) -> Result<Self, PackError> {
        use sha2::{Digest, Sha256};
        let mut file = File::open(path)?;
        if file.metadata()?.len() != size_bytes {
            return Err(malformed("installed pack size changed"));
        }
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let length = file.read(&mut buffer)?;
            if length == 0 {
                break;
            }
            digest.update(&buffer[..length]);
        }
        if !format!("{:x}", digest.finalize()).eq_ignore_ascii_case(sha256) {
            return Err(malformed(
                "installed pack digest changed; download it again",
            ));
        }
        file.seek(SeekFrom::Start(0))?;
        #[cfg(test)]
        PACK_OPEN_CALLS.with(|count| count.set(count.get() + 1));
        Self::open_file(file)
    }

    fn open_file(mut file: File) -> Result<Self, PackError> {
        let mut fixed = [0u8; 12];
        file.read_exact(&mut fixed)?;
        let header_length = Self::check_fixed(&fixed)?;
        let mut header = vec![0u8; header_length];
        file.read_exact(&mut header)?;
        let mut count = [0u8; 4];
        file.read_exact(&mut count)?;
        let entries = u32::from_le_bytes(count) as usize;
        let length = file.metadata()?.len();
        if (entries as u64) * DIRECTORY_ENTRY as u64 > length {
            return Err(malformed("directory larger than the file"));
        }
        let mut directory = vec![0u8; entries * DIRECTORY_ENTRY];
        file.read_exact(&mut directory)?;
        Self::assemble(&header, &directory, length, Source::File(Mutex::new(file)))
    }

    /// A pack held in memory, for tests and fixtures.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, PackError> {
        let fixed = bytes.get(..12).ok_or_else(|| malformed("too short"))?;
        let header_length = Self::check_fixed(fixed)?;
        let header_end = 12 + header_length;
        let count = bytes
            .get(header_end..header_end + 4)
            .ok_or_else(|| malformed("too short"))?;
        let entries = u32::from_le_bytes(count.try_into().unwrap_or_default()) as usize;
        let directory_end = header_end + 4 + entries.saturating_mul(DIRECTORY_ENTRY);
        let header = bytes[12..header_end].to_vec();
        let directory = bytes
            .get(header_end + 4..directory_end)
            .ok_or_else(|| malformed("directory ends early"))?
            .to_vec();
        let length = bytes.len() as u64;
        Self::assemble(&header, &directory, length, Source::Bytes(bytes))
    }

    fn check_fixed(fixed: &[u8]) -> Result<usize, PackError> {
        if &fixed[..4] != MAGIC {
            return Err(malformed("not a location pack"));
        }
        let version = u16::from_le_bytes([fixed[4], fixed[5]]);
        if version != FORMAT_VERSION {
            return Err(malformed(format!(
                "format version {version} is not readable here"
            )));
        }
        Ok(u32::from_le_bytes([fixed[8], fixed[9], fixed[10], fixed[11]]) as usize)
    }

    fn assemble(
        header: &[u8],
        directory: &[u8],
        length: u64,
        source: Source,
    ) -> Result<Self, PackError> {
        let header: Header =
            serde_json::from_slice(header).map_err(|error| malformed(error.to_string()))?;
        let kind = match header.kind.as_str() {
            "venues" => PackKind::Venues,
            "containment" => PackKind::Containment,
            other => return Err(malformed(format!("unknown pack kind {other}"))),
        };
        let mut entries = Vec::with_capacity(directory.len() / DIRECTORY_ENTRY);
        for entry in directory.chunks_exact(DIRECTORY_ENTRY) {
            let key = u64::from_le_bytes(entry[0..8].try_into().unwrap_or_default());
            let offset = u64::from_le_bytes(entry[8..16].try_into().unwrap_or_default());
            let size = u32::from_le_bytes(entry[16..20].try_into().unwrap_or_default());
            if offset + u64::from(size) > length {
                return Err(malformed("a block lies past the end of the file"));
            }
            entries.push((key, offset, size));
        }
        if !entries.windows(2).all(|pair| pair[0].0 < pair[1].0) {
            return Err(malformed("tile directory is not sorted"));
        }
        Ok(Self {
            kind,
            header,
            directory: entries,
            source,
            cache: Mutex::new(VecDeque::new()),
        })
    }

    pub fn kind(&self) -> PackKind {
        self.kind
    }

    pub fn version(&self) -> &str {
        &self.header.version
    }

    pub fn licence(&self) -> &str {
        &self.header.licence
    }

    pub fn attribution(&self) -> &str {
        &self.header.attribution
    }

    pub fn notices(&self) -> &[String] {
        &self.header.notices
    }

    /// Tiles that may hold something within the box, at every zoom.
    fn blocks_touching(&self, west: f64, south: f64, east: f64, north: f64) -> Vec<usize> {
        let mut found = Vec::new();
        for z in 0..=MAX_ZOOM {
            let tiles = 1u64 << z;
            let width = 360.0 / tiles as f64;
            let height = 180.0 / tiles as f64;
            let column = |longitude: f64| {
                (((longitude + 180.0) / width).floor().max(0.0) as u64).min(tiles - 1)
            };
            let row = |latitude: f64| {
                (((latitude + 90.0) / height).floor().max(0.0) as u64).min(tiles - 1)
            };
            // An item lies within its tile's eight neighbours, so widen by one.
            for x in column(west - width)..=column(east + width) {
                for y in row(south - height)..=row(north + height) {
                    let key = tile_key(z, x, y);
                    if let Ok(index) = self.directory.binary_search_by_key(&key, |entry| entry.0) {
                        found.push(index);
                    }
                }
            }
        }
        found
    }

    fn read_raw(&self, offset: u64, size: u32) -> Result<Vec<u8>, PackError> {
        let compressed = match &self.source {
            Source::Bytes(bytes) => {
                bytes[offset as usize..(offset + u64::from(size)) as usize].to_vec()
            }
            Source::File(file) => {
                let mut file = file
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                file.seek(SeekFrom::Start(offset))?;
                let mut buffer = vec![0u8; size as usize];
                file.read_exact(&mut buffer)?;
                buffer
            }
        };
        let mut raw = Vec::new();
        lzma_rs::lzma2_decompress(&mut compressed.as_slice(), &mut raw)
            .map_err(|error| malformed(format!("block does not decompress: {error}")))?;
        Ok(raw)
    }

    fn block(&self, index: usize) -> Result<Arc<Block>, PackError> {
        let (_, offset, size) = self.directory[index];
        if let Some((_, block)) = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|(cached, _)| *cached == offset)
        {
            return Ok(Arc::clone(block));
        }
        let raw = self.read_raw(offset, size)?;
        let block = Arc::new(match self.kind {
            PackKind::Venues => Block::Venues(self.parse_venues(&raw)?),
            PackKind::Containment => {
                let heads = self.parse_heads(&raw)?;
                Block::Features { raw, heads }
            }
        });
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.len() >= CACHED_BLOCKS {
            cache.pop_front();
        }
        cache.push_back((offset, Arc::clone(&block)));
        Ok(block)
    }

    fn parse_venues(&self, raw: &[u8]) -> Result<Vec<Venue>, PackError> {
        let mut cursor = Cursor::new(raw);
        let count = cursor.count()?;
        let mut latitudes = Vec::with_capacity(count);
        let mut previous = 0i64;
        for _ in 0..count {
            previous += cursor.zigzag()?;
            latitudes.push(previous);
        }
        let mut longitudes = Vec::with_capacity(count);
        previous = 0;
        for _ in 0..count {
            previous += cursor.zigzag()?;
            longitudes.push(previous);
        }
        let mut categories = Vec::with_capacity(count);
        for _ in 0..count {
            categories.push(cursor.count()?);
        }
        let confidences = cursor.take(count)?;
        let mut venues = Vec::with_capacity(count);
        for index in 0..count {
            let category = self
                .header
                .categories
                .get(categories[index])
                .ok_or_else(|| malformed("unknown category"))?;
            venues.push(Venue {
                name: cursor.text()?,
                category: category.clone(),
                confidence: confidences[index],
                latitude: degrees(latitudes[index]),
                longitude: degrees(longitudes[index]),
            });
        }
        Ok(venues)
    }

    /// Read each outline's class, name and bounds, and step over its
    /// geometry without decoding it.
    fn parse_heads(&self, raw: &[u8]) -> Result<Vec<FeatureHead>, PackError> {
        let mut cursor = Cursor::new(raw);
        let count = cursor.count()?;
        let mut heads = Vec::with_capacity(count);
        for _ in 0..count {
            let class = cursor.count()?;
            if class >= self.header.classes.len() {
                return Err(malformed("unknown class"));
            }
            let name = cursor.text()?;
            let mut bounds = [0.0; 4];
            for value in &mut bounds {
                *value = degrees(cursor.zigzag()?);
            }
            let start = cursor.at;
            for _ in 0..cursor.count()? {
                for _ in 0..cursor.count()? {
                    for _ in 0..cursor.count()? {
                        cursor.varint()?;
                        cursor.varint()?;
                    }
                }
            }
            heads.push(FeatureHead {
                class,
                name,
                bounds,
                geometry: start..cursor.at,
            });
        }
        Ok(heads)
    }

    fn feature(&self, raw: &[u8], head: &FeatureHead) -> Result<Feature, PackError> {
        let class = &self.header.classes[head.class];
        let layer = Layer::parse(&class[0]).ok_or_else(|| malformed("unknown layer"))?;
        let mut cursor = Cursor::new(&raw[head.geometry.clone()]);
        let polygon_count = cursor.count()?;
        let mut polygons = Vec::with_capacity(polygon_count);
        for _ in 0..polygon_count {
            let ring_count = cursor.count()?;
            let mut rings = Vec::with_capacity(ring_count);
            for _ in 0..ring_count {
                let points = cursor.count()?;
                let mut ring = Vec::with_capacity(points);
                let (mut longitude, mut latitude) = (0i64, 0i64);
                for _ in 0..points {
                    longitude += cursor.zigzag()?;
                    latitude += cursor.zigzag()?;
                    ring.push([degrees(longitude), degrees(latitude)]);
                }
                rings.push(ring);
            }
            polygons.push(rings);
        }
        Ok(Feature {
            layer,
            subtype: class[1].clone(),
            class: class[2].clone(),
            name: head.name.clone(),
            bounds: head.bounds,
            polygons,
            tolerance_m: self
                .header
                .tolerance_m
                .get(layer.as_str())
                .copied()
                .unwrap_or(0.0),
        })
    }

    fn query_box(latitude: f64, longitude: f64, radius_m: f64) -> [f64; 4] {
        let dlat = radius_m / METRES_PER_DEGREE;
        let dlon = radius_m / (METRES_PER_DEGREE * latitude.to_radians().cos().max(0.01));
        [
            longitude - dlon,
            latitude - dlat,
            longitude + dlon,
            latitude + dlat,
        ]
    }

    /// Venues within `radius_m` of the point, closest first, with distances.
    pub fn venues_within(
        &self,
        latitude: f64,
        longitude: f64,
        radius_m: f64,
    ) -> Result<Vec<(Venue, f64)>, PackError> {
        if self.kind != PackKind::Venues {
            return Ok(Vec::new());
        }
        let [west, south, east, north] = Self::query_box(latitude, longitude, radius_m);
        let mut found = Vec::new();
        for index in self.blocks_touching(west, south, east, north) {
            let block = self.block(index)?;
            let Block::Venues(venues) = block.as_ref() else {
                continue;
            };
            for venue in venues {
                let distance = distance_m(latitude, longitude, venue.latitude, venue.longitude);
                if distance <= radius_m {
                    found.push((venue.clone(), distance));
                }
            }
        }
        found.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.name.cmp(&b.0.name)));
        Ok(found)
    }

    /// Outlines that hold the whole disc of `radius_m` around the point,
    /// smallest first.
    pub fn features_containing(
        &self,
        latitude: f64,
        longitude: f64,
        radius_m: f64,
    ) -> Result<Vec<Feature>, PackError> {
        if self.kind != PackKind::Containment {
            return Ok(Vec::new());
        }
        let [west, south, east, north] = Self::query_box(latitude, longitude, radius_m);
        let mut found: Vec<Feature> = Vec::new();
        for index in self.blocks_touching(west, south, east, north) {
            let block = self.block(index)?;
            let Block::Features { raw, heads } = block.as_ref() else {
                continue;
            };
            for head in heads {
                let [f_west, f_south, f_east, f_north] = head.bounds;
                if west < f_west || east > f_east || south < f_south || north > f_north {
                    continue;
                }
                let feature = self.feature(raw, head)?;
                if feature.contains_disc(latitude, longitude, radius_m) {
                    found.push(feature);
                }
            }
        }
        found.sort_by(|a, b| {
            a.extent()
                .total_cmp(&b.extent())
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(found)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const LAT: f64 = 39.7527;
    const LON: f64 = -105.0003;

    pub(crate) fn fixture(name: &str) -> Pack {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/accuracy")
            .join(name);
        Pack::open(&path).unwrap()
    }

    fn offset(east_m: f64, north_m: f64) -> (f64, f64) {
        (
            LAT + north_m / METRES_PER_DEGREE,
            LON + east_m / (METRES_PER_DEGREE * LAT.to_radians().cos()),
        )
    }

    #[test]
    fn venues_within_a_margin_come_closest_first() {
        for file in ["venues.slpk", "unmerged/venues.slpk"] {
            let pack = fixture(file);
            assert_eq!(pack.kind(), PackKind::Venues);
            let found = pack.venues_within(LAT, LON, 50.0).unwrap();
            let names: Vec<&str> = found.iter().map(|(venue, _)| venue.name.as_str()).collect();
            assert_eq!(names, ["Invented Coffee", "Invented Diner"], "{file}");
            assert!((found[0].1 - 30.0).abs() < 1.0);
            assert_eq!(found[0].0.category, "coffee_shop");
            assert_eq!(found[0].0.confidence, 93);
            assert_eq!(pack.venues_within(LAT, LON, 100.0).unwrap().len(), 4);
        }
    }

    #[test]
    fn containment_needs_the_whole_disc_inside() {
        for file in ["containment.slpk", "unmerged/containment.slpk"] {
            let pack = fixture(file);
            let names = |lat: f64, lon: f64, radius: f64| -> Vec<String> {
                pack.features_containing(lat, lon, radius)
                    .unwrap()
                    .into_iter()
                    .map(|feature| feature.name)
                    .collect()
            };
            // The station is 160 m by 80 m with a 20 m by 10 m courtyard hole
            // in the middle, so a disc centred on the hole is not inside it.
            assert_eq!(
                names(LAT, LON, 25.0),
                ["Invented Plaza", "Lower Invented", "Inventedville"],
                "{file}"
            );
            let (lat, lon) = offset(-35.0, 0.0);
            assert_eq!(
                names(lat, lon, 20.0),
                [
                    "Invented Station",
                    "Invented Plaza",
                    "Lower Invented",
                    "Inventedville"
                ],
                "{file}"
            );
            // The same spot with a wider margin reaches the courtyard, 25 m
            // away, so it is no longer wholly inside the station.
            assert_eq!(
                names(lat, lon, 30.0),
                ["Invented Plaza", "Lower Invented", "Inventedville"]
            );
            // Inside the ring park's hole is not inside the park.
            let (lat, lon) = offset(1200.0, 1200.0);
            assert!(!names(lat, lon, 25.0).contains(&"Ring Park".to_owned()));
            let (lat, lon) = offset(1060.0, 1060.0);
            assert!(names(lat, lon, 25.0).contains(&"Ring Park".to_owned()));
            // The second polygon of a multipolygon counts.
            let (lat, lon) = offset(25_500.0, 20_500.0);
            assert_eq!(names(lat, lon, 100.0), ["Invented International Airport"]);
        }
    }

    #[test]
    fn simplification_tolerance_widens_the_margin() {
        let pack = fixture("containment.slpk");
        let feature = pack
            .features_containing(LAT, LON, 25.0)
            .unwrap()
            .into_iter()
            .find(|feature| feature.name == "Invented Plaza")
            .unwrap();
        assert_eq!(feature.layer, Layer::LandUse);
        assert_eq!(feature.tolerance_m, 5.0);
        // The plaza's nearest edge is 150 m away: 145 m plus 5 m of
        // tolerance just fits, 146 m does not.
        assert!(feature.contains_disc(LAT, LON, 144.0));
        assert!(!feature.contains_disc(LAT, LON, 146.0));
    }

    #[test]
    fn a_damaged_pack_is_refused_or_read_without_panicking() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/accuracy/venues.slpk");
        let bytes = std::fs::read(path).unwrap();
        let mut wrong_magic = bytes.clone();
        wrong_magic[0] = b'X';
        assert!(Pack::from_bytes(wrong_magic).is_err());
        let truncated = bytes[..bytes.len() - 10].to_vec();
        assert!(Pack::from_bytes(truncated).is_err(), "a block past the end");
        // Blocks carry no checksum (the download verifies the whole file), so
        // a flipped byte may decode to other names, but never panics.
        for position in (bytes.len() - 60)..bytes.len() {
            let mut flipped = bytes.clone();
            flipped[position] ^= 0xff;
            let pack = Pack::from_bytes(flipped).unwrap();
            let _ = pack.venues_within(LAT, LON, 50.0);
        }
    }
}
