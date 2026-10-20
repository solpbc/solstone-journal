// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Naming from the location-accuracy packs: "at X", "in X" and "near X".
//!
//! The words carry the confidence. Every rule here is about the map and the
//! phone's own margin of error, never about the owner, and every place is
//! treated the same way:
//!
//! - **margin**: the accuracy the phone reported, floored at 25 m. Past 100 m,
//!   or with no accuracy, there are no venue words at all, which covers
//!   approximate location by the same rule.
//! - **at X**: the whole margin lies inside X's outline (a named building, a
//!   named area such as a park or campus, or an airport). The smallest such
//!   outline is X; the next enclosing one is context.
//! - **in X**: the whole margin lies inside a neighbourhood or town boundary.
//! - **near X**: X is a named place within the margin. At most the three
//!   closest, each with its category. It describes the map around the
//!   position, so it is true by construction.
//!
//! "at" and "near" belong to stops (a visit the phone reported). Readings
//! taken on the move get "in" at most.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::pack::{Feature, Layer, Pack, PackKind};

/// The smallest margin a position is given, in metres.
pub const MARGIN_FLOOR_M: f64 = 25.0;
/// Past this reported accuracy, a position gets no venue words.
pub const MARGIN_CAP_M: f64 = 100.0;
/// "near" names at most this many places.
pub const NEAR_LIMIT: usize = 3;

/// The margin a position's words rely on, or none past the cap.
pub fn margin_m(accuracy_m: Option<f64>) -> Option<f64> {
    let accuracy = accuracy_m.filter(|value| value.is_finite() && *value >= 0.0)?;
    (accuracy <= MARGIN_CAP_M).then(|| accuracy.max(MARGIN_FLOOR_M))
}

/// The neighbourhood and town boundaries that hold a whole margin.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Within {
    pub neighbourhood: Option<String>,
    pub locality: Option<String>,
}

impl Within {
    pub fn is_empty(&self) -> bool {
        self.neighbourhood.is_none() && self.locality.is_none()
    }
}

/// The outline a stop was inside.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AtPlace {
    pub name: String,
    /// `building`, `land_use` or `airport`.
    pub layer: &'static str,
    /// The source's class, e.g. `train_station` or `park`; may be empty.
    pub class: String,
    /// The next named outline around it, when there is one.
    pub inside: Option<String>,
    /// Its outer rings as `[longitude, latitude]`, for the map.
    #[serde(skip)]
    pub outline: Vec<Vec<[f64; 2]>>,
}

/// A named place within a stop's margin.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NearPlace {
    pub name: String,
    /// The source's category, e.g. `coffee_shop`; may be empty.
    pub category: String,
    pub distance_m: f64,
}

/// What the packs say about one stop.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StopWords {
    pub margin_m: f64,
    pub at: Option<AtPlace>,
    pub near: Vec<NearPlace>,
}

impl StopWords {
    pub fn is_empty(&self) -> bool {
        self.at.is_none() && self.near.is_empty()
    }
}

/// The installed packs, either of which may be absent.
pub struct Packs {
    venues: Option<Pack>,
    containment: Option<Pack>,
    identity: String,
    pub load_failure: Option<String>,
    _generation_hold: Option<solstone_core_journal_io::FileLock>,
}

impl std::fmt::Debug for Packs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.identity)
    }
}

fn divisions(features: &[Feature]) -> Within {
    let pick = |subtypes: &[&str]| {
        features
            .iter()
            .filter(|feature| feature.layer == Layer::Division)
            .find(|feature| subtypes.contains(&feature.subtype.as_str()))
            .map(|feature| feature.name.clone())
    };
    let locality = pick(&["locality"]).or_else(|| pick(&["localadmin"]));
    let neighbourhood = pick(&["microhood", "neighborhood", "macrohood"])
        .filter(|name| Some(name) != locality.as_ref());
    Within {
        neighbourhood,
        locality,
    }
}

impl Packs {
    pub fn new(venues: Option<Pack>, containment: Option<Pack>) -> Self {
        let venues = venues.filter(|pack| pack.kind() == PackKind::Venues);
        let containment = containment.filter(|pack| pack.kind() == PackKind::Containment);
        let identity = format!(
            "venues:{};containment:{}",
            venues.as_ref().map_or("-", |pack| pack.version()),
            containment.as_ref().map_or("-", |pack| pack.version())
        );
        Self {
            venues,
            containment,
            identity,
            load_failure: None,
            _generation_hold: None,
        }
    }

    /// Which pack versions these are, for build records.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn has_venues(&self) -> bool {
        self.venues.is_some()
    }

    pub fn has_containment(&self) -> bool {
        self.containment.is_some()
    }

    /// Licence notices for what is installed, to show beside the names.
    pub fn attributions(&self) -> Vec<String> {
        [&self.venues, &self.containment]
            .into_iter()
            .flatten()
            .map(|pack| pack.attribution().to_owned())
            .filter(|text| !text.is_empty())
            .collect()
    }

    /// The full licence notices for what is installed.
    pub fn notices(&self) -> Vec<String> {
        [&self.venues, &self.containment]
            .into_iter()
            .flatten()
            .flat_map(|pack| pack.notices().iter().cloned())
            .collect()
    }

    fn containing(&self, latitude: f64, longitude: f64, margin: f64) -> Vec<Feature> {
        self.containment
            .as_ref()
            .and_then(|pack| pack.features_containing(latitude, longitude, margin).ok())
            .unwrap_or_default()
    }

    /// The boundaries that hold a reading's whole margin.
    pub fn within(&self, latitude: f64, longitude: f64, accuracy_m: Option<f64>) -> Option<Within> {
        let margin = margin_m(accuracy_m)?;
        let within = divisions(&self.containing(latitude, longitude, margin));
        (!within.is_empty()).then_some(within)
    }

    /// Everything the packs say about a stop: its boundaries, the outline it
    /// was inside, and the places within its margin.
    pub fn stop(
        &self,
        latitude: f64,
        longitude: f64,
        accuracy_m: Option<f64>,
    ) -> Option<(Option<Within>, StopWords)> {
        let margin = margin_m(accuracy_m)?;
        let features = self.containing(latitude, longitude, margin);
        let within = Some(divisions(&features)).filter(|within| !within.is_empty());
        let mut outlines = features
            .iter()
            .filter(|feature| feature.layer != Layer::Division);
        let at = outlines.next().map(|feature| AtPlace {
            name: feature.name.clone(),
            layer: feature.layer.as_str(),
            class: feature.class.clone(),
            inside: outlines
                .find(|outer| outer.name != feature.name)
                .map(|outer| outer.name.clone()),
            outline: feature
                .polygons
                .iter()
                .filter_map(|polygon| polygon.first().cloned())
                .collect(),
        });
        let mut near: Vec<NearPlace> = Vec::new();
        if let Some(pack) = &self.venues {
            for (venue, distance) in pack
                .venues_within(latitude, longitude, margin)
                .unwrap_or_default()
            {
                // One name once: a chain's two records at one corner, or the
                // venue that shares the outline's name.
                if near.iter().any(|place| place.name == venue.name)
                    || at.as_ref().is_some_and(|at| at.name == venue.name)
                {
                    continue;
                }
                near.push(NearPlace {
                    name: venue.name,
                    category: venue.category,
                    distance_m: distance,
                });
                if near.len() == NEAR_LIMIT {
                    break;
                }
            }
        }
        Some((
            within,
            StopWords {
                margin_m: margin,
                at,
                near,
            },
        ))
    }
}

/// Where the download lives in a journal: under `cache/`, which backups skip.
pub fn accuracy_root(journal: &Path) -> PathBuf {
    journal.join("cache").join("location").join("accuracy")
}

pub(crate) fn asset_root(journal: &Path) -> PathBuf {
    journal.join("cache/location/accuracy-assets")
}

/// The record of what is installed, written last by an install.
pub const INSTALLED_RECORD: &str = "installed.json";

/// One installed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledFile {
    pub file: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// What `installed.json` says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    pub version: String,
    #[serde(default)]
    pub folder: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_generation: Option<String>,
    pub files: Vec<InstalledFile>,
}

impl Installed {
    pub(crate) fn valid_paths(&self) -> bool {
        let segment = |name: &str| {
            !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':'])
        };
        crate::cache::inventory(self).is_ok()
            && segment(&self.version)
            && (self.folder.is_empty() || segment(&self.folder))
            && self.files.iter().all(|f| segment(&f.file))
            && self
                .cache_generation
                .as_ref()
                .is_none_or(|key| crate::cache::inventory(self).is_ok_and(|g| key == &g.key()))
    }

    pub fn directory(&self, journal: &Path) -> PathBuf {
        if let Some(key) = &self.cache_generation {
            return asset_root(journal).join("generations").join(key);
        }
        let name = if !self.folder.is_empty() {
            &self.folder
        } else {
            &self.version
        };
        accuracy_root(journal).join(name)
    }

    pub fn size_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size_bytes).sum()
    }
}

/// Read `installed.json`, if the journal has a complete install.
pub fn read_installed(journal: &Path) -> Option<Installed> {
    let bytes = std::fs::read(accuracy_root(journal).join(INSTALLED_RECORD)).ok()?;
    let record: Installed = serde_json::from_slice(&bytes).ok()?;
    record.valid_paths().then_some(record)
}

/// Read the install record for a fingerprint.
///
/// A missing file is no install. Other IO and invalid JSON are errors.
pub fn read_installed_strict(journal: &Path) -> std::io::Result<Option<Installed>> {
    match std::fs::read(accuracy_root(journal).join(INSTALLED_RECORD)) {
        Ok(bytes) => {
            let record: Installed = serde_json::from_slice(&bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            if !record.valid_paths() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid installed accuracy paths",
                ));
            }
            Ok(Some(record))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn open_installed(journal: &Path, installed: &Installed) -> Packs {
    let directory = installed.directory(journal);
    let Ok(hold) = crate::cache::hold_use(journal, installed) else {
        return Packs::new(None, None);
    };
    let mut venues = None;
    let mut containment = None;
    let mut failure = None;
    for file in &installed.files {
        let path = directory.join(&file.file);
        match Pack::open_verified(&path, &file.sha256, file.size_bytes) {
            Ok(pack) if pack.kind() == PackKind::Venues => venues = Some(pack),
            Ok(pack) if pack.kind() == PackKind::Containment => containment = Some(pack),
            Err(error) => failure = Some(error.to_string()),
            _ => failure = Some("installed accuracy pack has the wrong type".into()),
        }
    }
    let mut packs = Packs::new(venues, containment);
    packs._generation_hold = Some(hold);
    packs.load_failure = failure;
    packs
}

type LoadedPacks = Option<(PathBuf, Installed, Arc<Packs>)>;
static LOADED: Mutex<LoadedPacks> = Mutex::new(None);

/// The packs installed in this journal, opened once and reopened when the
/// install changes. `None` when the owner has not downloaded them.
pub fn installed_packs(journal: &Path) -> Option<Arc<Packs>> {
    installed_snapshot(journal).and_then(|(_, packs)| packs)
}

/// Observe one installed record and its open packs under publication exclusion.
/// This lock is separate from daily admission because talent readers may already
/// hold daily admission. Writers acquire daily admission before this short lock.
pub fn installed_snapshot(journal: &Path) -> Option<(Installed, Option<Arc<Packs>>)> {
    read_installed_snapshot(journal, || {})
}

fn read_installed_snapshot(
    journal: &Path,
    after_read: impl FnOnce(),
) -> Option<(Installed, Option<Arc<Packs>>)> {
    let _publication = hold_publication(journal).ok()?;
    let installed = read_installed(journal);
    after_read();
    let mut loaded = LOADED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(installed) = installed else {
        *loaded = None;
        return None;
    };
    if let Some((path, current, packs)) = loaded.as_ref()
        && path == journal
        && current == &installed
    {
        return Some((installed, Some(Arc::clone(packs))));
    }
    let packs = Arc::new(open_installed(journal, &installed));

    *loaded = Some((journal.to_path_buf(), installed.clone(), Arc::clone(&packs)));
    Some((installed, Some(packs)))
}

pub(crate) fn hold_publication(
    journal: &Path,
) -> Result<solstone_core_journal_io::FileLock, solstone_core_journal_io::LockError> {
    solstone_core_journal_io::hold_lock(
        accuracy_root(journal).join(".publication.lock"),
        solstone_core_journal_io::LockOptions::default(),
    )
}

/// Close whatever this process has open, before files are removed.
pub fn release_loaded() {
    *LOADED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::pack::tests::fixture;
    use sha2::{Digest, Sha256};

    const LAT: f64 = 39.7527;
    const LON: f64 = -105.0003;

    pub(crate) fn fixture_packs() -> Packs {
        Packs::new(
            Some(fixture("venues.slpk")),
            Some(fixture("containment.slpk")),
        )
    }

    fn offset(east_m: f64, north_m: f64) -> (f64, f64) {
        (
            LAT + north_m / 111_195.0,
            LON + east_m / (111_195.0 * LAT.to_radians().cos()),
        )
    }

    #[test]
    fn the_margin_is_floored_and_capped() {
        assert_eq!(margin_m(Some(5.0)), Some(25.0));
        assert_eq!(margin_m(Some(60.0)), Some(60.0));
        assert_eq!(margin_m(Some(100.0)), Some(100.0));
        assert_eq!(margin_m(Some(100.5)), None);
        assert_eq!(margin_m(Some(2000.0)), None);
        assert_eq!(margin_m(None), None);
        assert_eq!(margin_m(Some(f64::NAN)), None);
    }

    #[test]
    fn a_stop_inside_a_building_is_at_it_and_near_what_is_within_its_margin() {
        let packs = fixture_packs();
        let (lat, lon) = offset(-45.0, 0.0);
        let (within, words) = packs.stop(lat, lon, Some(12.0)).unwrap();
        assert_eq!(words.margin_m, 25.0);
        let at = words.at.unwrap();
        assert_eq!(at.name, "Invented Station");
        assert_eq!(at.inside.as_deref(), Some("Invented Plaza"));
        assert_eq!(
            within.unwrap(),
            Within {
                neighbourhood: Some("Lower Invented".to_owned()),
                locality: Some("Inventedville".to_owned())
            }
        );
        // The closest invented place is over 60 m away, past the 25 m margin.
        assert!(words.near.is_empty());
    }

    #[test]
    fn near_lists_at_most_three_closest_with_categories() {
        let packs = fixture_packs();
        let (_, words) = packs.stop(LAT, LON, Some(95.0)).unwrap();
        let near: Vec<(&str, &str)> = words
            .near
            .iter()
            .map(|place| (place.name.as_str(), place.category.as_str()))
            .collect();
        assert_eq!(
            near,
            [
                ("Invented Coffee", "coffee_shop"),
                ("Invented Diner", "restaurant"),
                ("Invented Books", "bookstore")
            ]
        );
        // The plaza reaches 150 m each way, so a 95 m margin is inside it.
        assert_eq!(words.at.unwrap().name, "Invented Plaza");
    }

    #[test]
    fn past_the_cap_there_are_no_venue_words() {
        let packs = fixture_packs();
        assert!(packs.stop(LAT, LON, Some(150.0)).is_none());
        assert!(packs.stop(LAT, LON, None).is_none());
        assert!(packs.within(LAT, LON, Some(2000.0)).is_none());
    }

    #[test]
    fn a_reading_gets_only_its_boundaries() {
        let packs = fixture_packs();
        let within = packs.within(LAT, LON, Some(10.0)).unwrap();
        assert_eq!(within.locality.as_deref(), Some("Inventedville"));
        assert_eq!(within.neighbourhood.as_deref(), Some("Lower Invented"));
    }

    #[test]
    fn identity_names_both_versions() {
        assert_eq!(
            fixture_packs().identity(),
            "venues:fixture-1;containment:fixture-1"
        );
        let venues_only = Packs::new(Some(fixture("venues.slpk")), None);
        assert_eq!(venues_only.identity(), "venues:fixture-1;containment:-");
        assert!(
            venues_only
                .stop(LAT, LON, Some(40.0))
                .unwrap()
                .1
                .at
                .is_none()
        );
    }

    #[test]
    fn an_installed_record_opens_its_files_and_follows_changes() {
        let journal = tempfile::tempdir().unwrap();
        assert!(installed_packs(journal.path()).is_none());
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/accuracy");
        let installed = Installed {
            version: "fixture-1".to_owned(),
            folder: String::new(),
            cache_generation: None,
            files: ["venues.slpk", "containment.slpk"]
                .iter()
                .map(|name| InstalledFile {
                    file: (*name).to_owned(),
                    sha256: format!(
                        "{:x}",
                        Sha256::digest(std::fs::read(source.join(name)).unwrap())
                    ),
                    size_bytes: std::fs::metadata(source.join(name)).unwrap().len(),
                })
                .collect(),
        };
        let directory = installed.directory(journal.path());
        std::fs::create_dir_all(&directory).unwrap();
        for file in &installed.files {
            std::fs::copy(source.join(&file.file), directory.join(&file.file)).unwrap();
        }
        let record = accuracy_root(journal.path()).join(INSTALLED_RECORD);
        std::fs::write(&record, serde_json::to_vec(&installed).unwrap()).unwrap();
        let packs = installed_packs(journal.path()).unwrap();
        assert!(packs.has_venues() && packs.has_containment());
        std::fs::remove_file(&record).unwrap();
        assert!(installed_packs(journal.path()).is_none());
    }

    #[test]
    fn same_length_corruption_is_refused_before_both_native_parser_entries() {
        let journal = tempfile::tempdir().unwrap();
        let bytes = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/accuracy/venues.slpk"),
        )
        .unwrap();
        let installed = Installed {
            version: "verified".into(),
            folder: String::new(),
            cache_generation: None,
            files: vec![InstalledFile {
                file: "venues.slpk".into(),
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                size_bytes: bytes.len() as u64,
            }],
        };
        let directory = installed.directory(journal.path());
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("venues.slpk");
        std::fs::write(&path, &bytes).unwrap();
        crate::install::publish_installed_record(journal.path(), &installed).unwrap();
        let before = crate::pack::PACK_OPEN_CALLS.with(|c| c.get());
        assert!(installed_packs(journal.path()).unwrap().has_venues());
        assert_eq!(crate::pack::PACK_OPEN_CALLS.with(|c| c.get()), before + 1);
        release_loaded();
        let mut corrupt = bytes.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        std::fs::write(&path, &corrupt).unwrap();
        let before = crate::pack::PACK_OPEN_CALLS.with(|c| c.get());
        let packs = installed_packs(journal.path()).unwrap();
        assert!(!packs.has_venues());
        assert!(packs.load_failure.as_deref().unwrap().contains("digest"));
        assert!(
            crate::fingerprint::daily_location_fingerprint(journal.path(), "20261001").is_err()
        );
        assert_eq!(crate::pack::PACK_OPEN_CALLS.with(|c| c.get()), before);
        drop(packs);
        release_loaded();
        std::fs::write(&path, &bytes).unwrap();
        assert!(crate::fingerprint::daily_location_fingerprint(journal.path(), "20261001").is_ok());
        // Fingerprinting admits the pack and then renders the day through the
        // normal loaded-pack path. Both use the verified native entry.
        assert!(crate::pack::PACK_OPEN_CALLS.with(|c| c.get()) > before);
    }

    #[test]
    fn installed_deserialization_handles_missing_or_custom_folder() {
        let journal = Path::new("/test/journal");
        let without_folder_json = r#"{"version":"v1","files":[]}"#;
        let installed: Installed = serde_json::from_str(without_folder_json).unwrap();
        assert_eq!(installed.folder, "");
        assert_eq!(
            installed.directory(journal),
            accuracy_root(journal).join("v1")
        );

        let with_folder_json = r#"{"version":"v1","folder":"v1.42","files":[]}"#;
        let installed_with_folder: Installed = serde_json::from_str(with_folder_json).unwrap();
        assert_eq!(installed_with_folder.folder, "v1.42");
        assert_eq!(
            installed_with_folder.directory(journal),
            accuracy_root(journal).join("v1.42")
        );
    }
    #[cfg(feature = "full-tests")]
    #[test]
    fn record_and_pack_open_stay_together_while_a_replacement_publishes() {
        use crate::install::publish_installed_record;
        use std::{fs, sync::mpsc, thread, time::Duration};
        let journal = tempfile::tempdir().unwrap();
        let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/accuracy");
        let make = |version: &str| {
            let directory = accuracy_root(journal.path()).join(version);
            fs::create_dir_all(&directory).unwrap();
            let files = ["venues.slpk", "containment.slpk"]
                .into_iter()
                .map(|name| {
                    let bytes = fs::read(fixture_root.join(name)).unwrap();
                    fs::write(directory.join(name), &bytes).unwrap();
                    InstalledFile {
                        file: name.to_owned(),
                        sha256: format!("{:x}", Sha256::digest(&bytes)),
                        size_bytes: bytes.len() as u64,
                    }
                })
                .collect();
            Installed {
                version: version.to_owned(),
                folder: String::new(),
                cache_generation: None,
                files,
            }
        };
        let old = make("old");
        publish_installed_record(journal.path(), &old).unwrap();
        let new = make("new");
        let (read_tx, read_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let path = journal.path().to_path_buf();
        let reader = thread::spawn(move || {
            read_installed_snapshot(&path, || {
                read_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
            .unwrap()
        });
        read_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let path = journal.path().to_path_buf();
        let writer = thread::spawn(move || {
            publish_installed_record(&path, &new).unwrap();
            done_tx.send(()).unwrap();
        });
        let premature = done_rx.recv_timeout(Duration::from_millis(100));
        release_tx.send(()).unwrap();
        let (record, packs) = reader.join().unwrap();
        writer.join().unwrap();
        assert!(
            matches!(premature, Err(mpsc::RecvTimeoutError::Timeout)),
            "publication must wait for the admitted reader"
        );
        assert_eq!(record, old);
        let packs = packs.unwrap();
        assert!(packs.has_venues() && packs.has_containment());
        assert!(
            !packs
                .stop(39.7527, -105.0003, Some(95.0))
                .unwrap()
                .1
                .near
                .is_empty()
        );
        let (current, packs) = installed_snapshot(journal.path()).unwrap();
        assert_eq!(current.version, "new");
        let packs = packs.unwrap();
        assert!(packs.has_venues() && packs.has_containment());
    }
}
