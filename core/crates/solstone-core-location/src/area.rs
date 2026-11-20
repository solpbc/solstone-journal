// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The bundled area tier: towns from GeoNames cities500, country and state
//! boundaries from Natural Earth, read from one file that ships with every
//! journal. Naming a reading is a lookup in memory; nothing is asked of any
//! service. `scripts/build_location_area_tier.py` documents the file format.

use solstone_core_window_catalog::{ENGLISH, strings};
use std::fmt;
use std::path::Path;

/// A town is used for a reading only within this distance of it.
pub const TOWN_RADIUS_M: f64 = 25_000.0;
const EARTH_RADIUS_M: f64 = 6_371_008.8;
const METRES_PER_DEGREE: f64 = 111_195.0;
const FORMAT_VERSION: u16 = 1;

/// The area tier could not be read.
#[derive(Debug)]
pub enum AreaTierError {
    /// The file is not an area tier, or is a version this build cannot read.
    Format(String),
    /// The file could not be found or read.
    Asset(crate::asset::AssetError),
    /// The process could not load the bundled file; the detail says why.
    Unavailable(String),
}

impl fmt::Display for AreaTierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(detail) => write!(formatter, "area tier file is malformed: {detail}"),
            Self::Asset(error) => write!(formatter, "{error}"),
            Self::Unavailable(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for AreaTierError {}

/// A longitude/latitude box in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

impl Bounds {
    fn intersects(&self, other: &Bounds) -> bool {
        self.west <= other.east
            && other.west <= self.east
            && self.south <= other.north
            && other.south <= self.north
    }

    fn contains(&self, longitude: f64, latitude: f64) -> bool {
        longitude >= self.west
            && longitude <= self.east
            && latitude >= self.south
            && latitude <= self.north
    }

    /// Whether the straight segment from `a` to `b` passes through this box
    /// (Liang–Barsky). A long border edge can cross a small view with both of
    /// its ends far outside it.
    fn crossed_by(&self, a: [f64; 2], b: [f64; 2]) -> bool {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let mut low = 0.0_f64;
        let mut high = 1.0_f64;
        for (p, q) in [
            (-dx, a[0] - self.west),
            (dx, self.east - a[0]),
            (-dy, a[1] - self.south),
            (dy, self.north - a[1]),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    return false;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    low = low.max(t);
                } else {
                    high = high.min(t);
                }
                if low > high {
                    return false;
                }
            }
        }
        true
    }
}

/// The town a reading is named after.
#[derive(Debug, Clone, PartialEq)]
pub struct Town {
    pub name: String,
    /// The ASCII spelling, when it differs from `name`.
    pub ascii_name: Option<String>,
    pub region: Option<String>,
    pub country: String,
    pub country_code: String,
    pub distance_m: f64,
}

/// The area-level name for one point.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AreaName {
    /// The town the point is near, within [`TOWN_RADIUS_M`].
    pub town: Option<Town>,
    /// The state or province containing the point, when no town is near.
    pub region: Option<String>,
    /// The country containing the point, when no town is near.
    pub country: Option<String>,
    /// The neighbourhood and town boundaries from the location-accuracy
    /// download that hold the reading's whole margin. Only then is "in" said.
    pub within: Option<crate::accuracy::Within>,
}

impl AreaName {
    /// The state or province, else the country, that qualifies a name.
    fn qualifier(&self) -> Option<&str> {
        match &self.town {
            Some(town) => Some(town.region.as_deref().unwrap_or(town.country.as_str())),
            None => self.region.as_deref().or(self.country.as_deref()),
        }
    }

    /// The short owner-facing label: "near Golden, Colorado". A point with no
    /// town nearby is labelled by the region it falls in, without a
    /// preposition, and a point outside every boundary has no label. A
    /// reading whose whole margin lies inside a neighbourhood or town
    /// boundary is "in" it: "in Montmartre, Paris".
    pub fn label(&self) -> Option<String> {
        self.label_in(ENGLISH)
    }

    pub fn label_in(&self, locale: &str) -> Option<String> {
        if let Some(within) = &self.within {
            let qualifier = self.qualifier();
            let named = match (&within.neighbourhood, &within.locality) {
                (Some(neighbourhood), Some(locality)) => Some(strings(
                    locale,
                    "journal.window.map.area.label.inside.qualified",
                    [("place", neighbourhood), ("qualifier", locality)],
                )),
                (None, Some(place)) | (Some(place), None) => Some(match qualifier {
                    Some(qualifier) if qualifier != place => strings(
                        locale,
                        "journal.window.map.area.label.inside.qualified",
                        [("place", place), ("qualifier", qualifier)],
                    ),
                    _ => strings(
                        locale,
                        "journal.window.map.area.label.inside",
                        [("place", place)],
                    ),
                }),
                (None, None) => None,
            };
            if named.is_some() {
                return named;
            }
        }
        if let Some(town) = &self.town {
            let qualifier = match &town.region {
                Some(region) if region != &town.name => region.as_str(),
                _ => town.country.as_str(),
            };
            return Some(strings(
                locale,
                "journal.window.map.area.label.near",
                [("place", &town.name), ("qualifier", qualifier)],
            ));
        }
        match (&self.region, &self.country) {
            (Some(region), Some(country)) if region != country => Some(strings(
                locale,
                "journal.window.map.area.label.qualified",
                [("region", region), ("country", country)],
            )),
            (Some(region), _) => Some(region.clone()),
            (None, Some(country)) => Some(country.clone()),
            (None, None) => None,
        }
    }

    /// Every name this area earns, for search: the town, its ASCII spelling,
    /// its region and its country. Nothing nearby that the point is not named
    /// after.
    pub fn words(&self) -> Vec<String> {
        let mut words = Vec::new();
        let mut push = |value: &str| {
            if !value.is_empty() && !words.iter().any(|word: &String| word == value) {
                words.push(value.to_owned());
            }
        };
        if let Some(within) = &self.within {
            for name in [&within.neighbourhood, &within.locality]
                .into_iter()
                .flatten()
            {
                push(name);
            }
        }
        if let Some(town) = &self.town {
            push(&town.name);
            if let Some(ascii) = &town.ascii_name {
                push(ascii);
            }
            if let Some(region) = &town.region {
                push(region);
            }
            push(&town.country);
        } else {
            if let Some(region) = &self.region {
                push(region);
            }
            if let Some(country) = &self.country {
                push(country);
            }
        }
        words
    }
}

/// A town drawn on the map.
#[derive(Debug, Clone, PartialEq)]
pub struct TownPoint {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub population: u32,
}

/// One boundary drawn on the map: lines of `[longitude, latitude]` pairs.
#[derive(Debug, Clone, PartialEq)]
pub struct Boundary {
    pub name: String,
    pub rings: Vec<Vec<[f64; 2]>>,
}

struct Country {
    code: String,
    name: String,
}

struct Admin1 {
    name: String,
}

struct TownRow {
    lat_e5: i32,
    lon_e5: i32,
    country: u16,
    admin1: u32,
    population: u32,
    name: (u32, u32),
    ascii: (u32, u32),
}

struct Region {
    name: String,
    code: String,
    bounds: Bounds,
    rings: Vec<Vec<(i32, i32)>>,
}

impl Region {
    fn contains(&self, longitude: f64, latitude: f64) -> bool {
        if !self.bounds.contains(longitude, latitude) {
            return false;
        }
        let x = (longitude * 1e4).round() as i64;
        let y = (latitude * 1e4).round() as i64;
        let mut inside = false;
        for ring in &self.rings {
            let mut previous = ring.len() - 1;
            for current in 0..ring.len() {
                let (xi, yi) = (i64::from(ring[current].0), i64::from(ring[current].1));
                let (xj, yj) = (i64::from(ring[previous].0), i64::from(ring[previous].1));
                if (yi > y) != (yj > y) {
                    let cross = (x - xi) * (yj - yi) - (xj - xi) * (y - yi);
                    if (cross < 0) != (yj < yi) {
                        inside = !inside;
                    }
                }
                previous = current;
            }
        }
        inside
    }

    /// The parts of this region's outline that cross `bounds`, as lines.
    /// Each line keeps one point beyond the box on either side, so it runs to
    /// the edge of the view instead of stopping short of it.
    fn lines_within(&self, bounds: &Bounds) -> Boundary {
        let mut lines = Vec::new();
        for ring in &self.rings {
            let points: Vec<[f64; 2]> = ring
                .iter()
                .map(|(x, y)| [f64::from(*x) / 1e4, f64::from(*y) / 1e4])
                .collect();
            let mut current: Vec<[f64; 2]> = Vec::new();
            for index in 0..points.len().saturating_sub(1) {
                if bounds.crossed_by(points[index], points[index + 1]) {
                    if current.is_empty() {
                        current.push(points[index]);
                    }
                    current.push(points[index + 1]);
                } else if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
            }
            if current.len() > 1 {
                lines.push(current);
            }
        }
        Boundary {
            name: self.name.clone(),
            rings: lines,
        }
    }
}

/// The parsed area tier.
pub struct AreaTier {
    countries: Vec<Country>,
    admin1: Vec<Admin1>,
    towns: Vec<TownRow>,
    text: String,
    cells: Vec<(u32, u32)>,
    regions: Vec<Region>,
    nations: Vec<Region>,
}

impl fmt::Debug for AreaTier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AreaTier")
            .field("towns", &self.towns.len())
            .field("regions", &self.regions.len())
            .field("countries", &self.nations.len())
            .finish()
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], AreaTierError> {
        let end = self
            .at
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| AreaTierError::Format(format!("truncated at byte {}", self.at)))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn varint(&mut self) -> Result<u64, AreaTierError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.take(1)?[0];
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(AreaTierError::Format(format!(
            "overlong varint at byte {}",
            self.at
        )))
    }

    fn count(&mut self) -> Result<usize, AreaTierError> {
        let value = self.varint()?;
        usize::try_from(value)
            .ok()
            .filter(|count| *count <= self.bytes.len())
            .ok_or_else(|| AreaTierError::Format(format!("implausible count {value}")))
    }

    fn zigzag(&mut self) -> Result<i64, AreaTierError> {
        let value = self.varint()?;
        Ok(((value >> 1) as i64) ^ -((value & 1) as i64))
    }

    fn text(&mut self) -> Result<&'a str, AreaTierError> {
        let length = self.count()?;
        std::str::from_utf8(self.take(length)?)
            .map_err(|_| AreaTierError::Format(format!("invalid UTF-8 near byte {}", self.at)))
    }

    fn code(&mut self) -> Result<String, AreaTierError> {
        let bytes = self.take(2)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| AreaTierError::Format("invalid country code".to_owned()))
    }
}

fn index_error(what: &str, index: u64) -> AreaTierError {
    AreaTierError::Format(format!("{what} index {index} is out of range"))
}

fn read_regions(reader: &mut Reader<'_>) -> Result<Vec<Region>, AreaTierError> {
    let count = reader.count()?;
    let mut regions = Vec::with_capacity(count);
    for _ in 0..count {
        let name = reader.text()?.to_owned();
        let code = reader.code()?;
        let ring_count = reader.count()?;
        let mut rings = Vec::with_capacity(ring_count);
        let mut bounds = Bounds {
            west: f64::MAX,
            south: f64::MAX,
            east: f64::MIN,
            north: f64::MIN,
        };
        for _ in 0..ring_count {
            let point_count = reader.count()?;
            let mut ring = Vec::with_capacity(point_count);
            let (mut x, mut y) = (0i64, 0i64);
            for _ in 0..point_count {
                x += reader.zigzag()?;
                y += reader.zigzag()?;
                let point = (
                    i32::try_from(x).map_err(|_| AreaTierError::Format("x".into()))?,
                    i32::try_from(y).map_err(|_| AreaTierError::Format("y".into()))?,
                );
                let (longitude, latitude) = (f64::from(point.0) / 1e4, f64::from(point.1) / 1e4);
                bounds.west = bounds.west.min(longitude);
                bounds.east = bounds.east.max(longitude);
                bounds.south = bounds.south.min(latitude);
                bounds.north = bounds.north.max(latitude);
                ring.push(point);
            }
            if !ring.is_empty() {
                rings.push(ring);
            }
        }
        if !rings.is_empty() {
            regions.push(Region {
                name,
                code,
                bounds,
                rings,
            });
        }
    }
    Ok(regions)
}

fn cell_of(latitude: f64, longitude: f64) -> u32 {
    let row = (latitude + 90.0).floor().clamp(0.0, 179.0) as u32;
    let column = (longitude + 180.0).floor().clamp(0.0, 359.0) as u32;
    row * 360 + column
}

fn distance_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (phi1, phi2) = (lat1.to_radians(), lat2.to_radians());
    let half_dphi = (phi2 - phi1) / 2.0;
    let half_dlambda = (lon2 - lon1).to_radians() / 2.0;
    let a = half_dphi.sin().powi(2) + phi1.cos() * phi2.cos() * half_dlambda.sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().min(1.0).asin()
}

impl AreaTier {
    /// Parse an area tier file's bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, AreaTierError> {
        let mut reader = Reader { bytes, at: 0 };
        if reader.take(4)? != b"SLAT" {
            return Err(AreaTierError::Format("missing SLAT magic".to_owned()));
        }
        let version = u16::from_le_bytes([reader.take(1)?[0], reader.take(1)?[0]]);
        if version != FORMAT_VERSION {
            return Err(AreaTierError::Format(format!(
                "version {version} is not {FORMAT_VERSION}"
            )));
        }
        let country_count = reader.count()?;
        let mut countries = Vec::with_capacity(country_count);
        for _ in 0..country_count {
            let code = reader.code()?;
            let name = reader.text()?.to_owned();
            countries.push(Country { code, name });
        }
        let admin1_count = reader.count()?;
        let mut admin1 = Vec::with_capacity(admin1_count);
        for _ in 0..admin1_count {
            let country = reader.varint()?;
            if country as usize >= countries.len() {
                return Err(index_error("country", country));
            }
            admin1.push(Admin1 {
                name: reader.text()?.to_owned(),
            });
        }
        let town_count = reader.count()?;
        let mut towns = Vec::with_capacity(town_count);
        let mut text = String::new();
        let mut cells: Vec<(u32, u32)> = Vec::new();
        let (mut lat, mut lon) = (0i64, 0i64);
        let push_text = |value: &str, text: &mut String| -> Result<(u32, u32), AreaTierError> {
            let start = u32::try_from(text.len())
                .map_err(|_| AreaTierError::Format("names too large".to_owned()))?;
            text.push_str(value);
            Ok((start, value.len() as u32))
        };
        for index in 0..town_count {
            lat += reader.zigzag()?;
            lon += reader.zigzag()?;
            let country = reader.varint()?;
            if country as usize >= countries.len() {
                return Err(index_error("country", country));
            }
            let admin = reader.varint()?;
            if admin as usize > admin1.len() {
                return Err(index_error("admin-1", admin));
            }
            let population = u32::try_from(reader.varint()?).unwrap_or(u32::MAX);
            let name = push_text(reader.text()?, &mut text)?;
            let ascii = push_text(reader.text()?, &mut text)?;
            let row = TownRow {
                lat_e5: i32::try_from(lat).map_err(|_| AreaTierError::Format("lat".into()))?,
                lon_e5: i32::try_from(lon).map_err(|_| AreaTierError::Format("lon".into()))?,
                country: country as u16,
                admin1: admin as u32,
                population,
                name,
                ascii,
            };
            let cell = cell_of(f64::from(row.lat_e5) / 1e5, f64::from(row.lon_e5) / 1e5);
            match cells.last() {
                Some((last, _)) if *last == cell => {}
                Some((last, _)) if *last > cell => {
                    return Err(AreaTierError::Format(format!(
                        "towns are not sorted by cell at town {index}"
                    )));
                }
                _ => cells.push((cell, index as u32)),
            }
            towns.push(row);
        }
        let regions = read_regions(&mut reader)?;
        let nations = read_regions(&mut reader)?;
        if reader.at != bytes.len() {
            return Err(AreaTierError::Format(format!(
                "{} trailing bytes",
                bytes.len() - reader.at
            )));
        }
        Ok(Self {
            countries,
            admin1,
            towns,
            text,
            cells,
            regions,
            nations,
        })
    }

    /// Read and parse an area tier file.
    pub fn load(path: &Path) -> Result<Self, AreaTierError> {
        let bytes = std::fs::read(path).map_err(|source| {
            AreaTierError::Asset(crate::asset::AssetError::Unreadable {
                path: path.to_path_buf(),
                source,
            })
        })?;
        Self::parse(&bytes)
    }

    fn town_text(&self, (start, length): (u32, u32)) -> &str {
        &self.text[start as usize..(start + length) as usize]
    }

    fn cell_range(&self, cell: u32) -> std::ops::Range<usize> {
        match self.cells.binary_search_by_key(&cell, |(id, _)| *id) {
            Ok(position) => {
                let start = self.cells[position].1 as usize;
                let end = self
                    .cells
                    .get(position + 1)
                    .map_or(self.towns.len(), |(_, next)| *next as usize);
                start..end
            }
            Err(_) => 0..0,
        }
    }

    fn rows_near(&self, latitude: f64, longitude: f64, radius_m: f64) -> Vec<usize> {
        let lat_span = radius_m / METRES_PER_DEGREE;
        let cos = latitude.to_radians().cos().max(0.01);
        let lon_span = (lat_span / cos).min(180.0);
        let first_row = (latitude - lat_span + 90.0).floor().max(0.0) as i64;
        let last_row = (latitude + lat_span + 90.0).floor().min(179.0) as i64;
        let first_column = (longitude - lon_span + 180.0).floor() as i64;
        let last_column = (longitude + lon_span + 180.0).floor() as i64;
        let mut columns: Vec<u32> = (first_column..=last_column)
            .map(|column| column.rem_euclid(360) as u32)
            .collect();
        columns.sort_unstable();
        columns.dedup();
        let mut rows = Vec::new();
        for row in first_row..=last_row {
            for column in &columns {
                rows.extend(self.cell_range(row as u32 * 360 + column));
            }
        }
        rows
    }

    /// Name a point at area level. The town is the one that best stands for
    /// the area within [`TOWN_RADIUS_M`]: a larger town a little further away
    /// wins over a hamlet next door, the way people say where they were.
    pub fn name_point(&self, latitude: f64, longitude: f64) -> AreaName {
        if !latitude.is_finite()
            || !longitude.is_finite()
            || !(-90.0..=90.0).contains(&latitude)
            || !(-180.0..=180.0).contains(&longitude)
        {
            return AreaName::default();
        }
        let mut best: Option<(f64, f64, usize)> = None;
        for index in self.rows_near(latitude, longitude, TOWN_RADIUS_M) {
            let row = &self.towns[index];
            let distance = distance_m(
                latitude,
                longitude,
                f64::from(row.lat_e5) / 1e5,
                f64::from(row.lon_e5) / 1e5,
            );
            if distance > TOWN_RADIUS_M {
                continue;
            }
            let kilometres = (distance / 1000.0).max(1.0);
            let score = f64::from(row.population.max(500)) / (kilometres * kilometres);
            if best.is_none_or(|(best_score, best_distance, _)| {
                score > best_score || (score == best_score && distance < best_distance)
            }) {
                best = Some((score, distance, index));
            }
        }
        if let Some((_, distance, index)) = best {
            let row = &self.towns[index];
            let country = &self.countries[row.country as usize];
            let ascii = self.town_text(row.ascii);
            return AreaName {
                town: Some(Town {
                    name: self.town_text(row.name).to_owned(),
                    ascii_name: (!ascii.is_empty()).then(|| ascii.to_owned()),
                    region: (row.admin1 > 0)
                        .then(|| self.admin1[row.admin1 as usize - 1].name.clone()),
                    country: country.name.clone(),
                    country_code: country.code.clone(),
                    distance_m: distance,
                }),
                region: None,
                country: None,
                within: None,
            };
        }
        let region = self
            .regions
            .iter()
            .find(|region| region.contains(longitude, latitude));
        let nation_code = region
            .map(|region| region.code.clone())
            .filter(|code| code != "??")
            .or_else(|| {
                self.nations
                    .iter()
                    .find(|nation| nation.contains(longitude, latitude))
                    .map(|nation| nation.code.clone())
            });
        let country = nation_code.and_then(|code| {
            self.countries
                .iter()
                .find(|country| country.code == code)
                .map(|country| country.name.clone())
        });
        AreaName {
            town: None,
            region: region.map(|region| region.name.clone()),
            country,
            within: None,
        }
    }

    /// The most populous towns inside `bounds`, for map labels.
    pub fn towns_in(&self, bounds: Bounds, limit: usize) -> Vec<TownPoint> {
        let mut found: Vec<&TownRow> = Vec::new();
        let first_row = (bounds.south + 90.0).floor().max(0.0) as u32;
        let last_row = (bounds.north + 90.0).floor().min(179.0) as u32;
        let first_column = (bounds.west + 180.0).floor().max(0.0) as u32;
        let last_column = (bounds.east + 180.0).floor().min(359.0) as u32;
        if (last_row - first_row + 1) * (last_column - first_column + 1) > 4_000 {
            // A continent-wide view labels only the largest towns, so a full
            // scan is cheaper than walking every cell.
            found.extend(self.towns.iter().filter(|row| {
                bounds.contains(f64::from(row.lon_e5) / 1e5, f64::from(row.lat_e5) / 1e5)
            }));
        } else {
            for row in first_row..=last_row {
                for column in first_column..=last_column {
                    found.extend(
                        self.cell_range(row * 360 + column)
                            .map(|index| &self.towns[index])
                            .filter(|town| {
                                bounds.contains(
                                    f64::from(town.lon_e5) / 1e5,
                                    f64::from(town.lat_e5) / 1e5,
                                )
                            }),
                    );
                }
            }
        }
        found.sort_by(|a, b| {
            b.population
                .cmp(&a.population)
                .then_with(|| self.town_text(a.name).cmp(self.town_text(b.name)))
        });
        found
            .into_iter()
            .take(limit)
            .map(|row| TownPoint {
                name: self.town_text(row.name).to_owned(),
                latitude: f64::from(row.lat_e5) / 1e5,
                longitude: f64::from(row.lon_e5) / 1e5,
                population: row.population,
            })
            .collect()
    }

    /// State and province outlines inside `bounds`.
    pub fn region_boundaries_in(&self, bounds: Bounds) -> Vec<Boundary> {
        Self::lines_in(&self.regions, &bounds)
    }

    /// Country outlines inside `bounds`.
    pub fn country_boundaries_in(&self, bounds: Bounds) -> Vec<Boundary> {
        Self::lines_in(&self.nations, &bounds)
    }

    fn lines_in(regions: &[Region], bounds: &Bounds) -> Vec<Boundary> {
        regions
            .iter()
            .filter(|region| region.bounds.intersects(bounds))
            .map(|region| region.lines_within(bounds))
            .filter(|boundary| !boundary.rings.is_empty())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier() -> &'static AreaTier {
        crate::asset::area_tier().expect("bundled area tier loads from the checkout")
    }

    fn label(latitude: f64, longitude: f64) -> Option<String> {
        tier().name_point(latitude, longitude).label()
    }

    #[test]
    fn public_landmarks_are_named_after_the_town_people_would_say() {
        assert_eq!(
            label(39.7527, -105.0001).as_deref(),
            Some("near Denver, Colorado")
        );
        assert_eq!(
            label(39.7555, -105.2211).as_deref(),
            Some("near Golden, Colorado")
        );
        assert_eq!(
            label(48.8584, 2.2945).as_deref(),
            Some("near Paris, Île-de-France")
        );
        assert_eq!(
            label(38.7075, -9.1364).as_deref(),
            Some("near Lisbon, Portugal")
        );
    }

    #[test]
    fn a_point_far_from_any_town_is_named_by_its_region_without_a_preposition() {
        let name = tier().name_point(43.0, -107.5);
        assert!(name.town.is_none());
        assert_eq!(name.label().as_deref(), Some("Wyoming, United States"));
        assert_eq!(tier().name_point(30.0, -40.0).label(), None);
    }

    #[test]
    fn search_words_are_only_the_names_the_area_earns() {
        let words = tier().name_point(39.7555, -105.2211).words();
        assert_eq!(words, ["Golden", "Colorado", "United States"]);
    }

    #[test]
    fn invalid_coordinates_have_no_name() {
        assert_eq!(tier().name_point(f64::NAN, 0.0), AreaName::default());
        assert_eq!(tier().name_point(91.0, 0.0), AreaName::default());
    }

    #[test]
    fn the_map_gets_towns_and_the_boundary_lines_that_cross_a_view() {
        let around_denver = Bounds {
            west: -105.4,
            south: 39.5,
            east: -104.7,
            north: 40.0,
        };
        let towns = tier().towns_in(around_denver, 5);
        assert_eq!(towns.first().map(|town| town.name.as_str()), Some("Denver"));
        assert!(tier().region_boundaries_in(around_denver).is_empty());

        let across_the_wyoming_line = Bounds {
            west: -105.5,
            south: 40.8,
            east: -104.5,
            north: 41.2,
        };
        let regions = tier().region_boundaries_in(across_the_wyoming_line);
        let names: Vec<&str> = regions.iter().map(|region| region.name.as_str()).collect();
        assert!(
            names.contains(&"Colorado") && names.contains(&"Wyoming"),
            "{names:?}"
        );
        for region in &regions {
            for line in &region.rings {
                assert!(line.len() >= 2);
            }
        }
        let canada = Bounds {
            west: -123.5,
            south: 48.8,
            east: -122.5,
            north: 49.2,
        };
        assert!(
            tier()
                .country_boundaries_in(canada)
                .iter()
                .any(|boundary| boundary.name == "Canada")
        );
    }

    #[test]
    fn a_truncated_file_is_refused() {
        assert!(matches!(
            AreaTier::parse(b"SLAT\x01\x00\x05"),
            Err(AreaTierError::Format(_))
        ));
        assert!(matches!(
            AreaTier::parse(b"NOPE"),
            Err(AreaTierError::Format(_))
        ));
    }
}
