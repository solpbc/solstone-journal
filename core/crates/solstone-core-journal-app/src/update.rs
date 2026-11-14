// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The journal app updates itself and the journal together: they are one
//! signed Velopack package on the journal's own feed. An update stops every
//! process in the install; the package's after-update step then starts the
//! journal again only if the owner had it running, so a stop the owner chose
//! holds across an update.
//!
//! The feed request carries nothing about this PC: a plain GET of the
//! release list with no query string, and the per-install staging id Velopack
//! would mint is kept empty on disk.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use velopack::bundle::Manifest;
use velopack::sources::UpdateSource;
use velopack::{
    Error, UpdateCheck, UpdateInfo, UpdateManager, UpdateOptions, VelopackAsset, VelopackAssetFeed,
};

use crate::prefs::{self, CheckInterval, Prefs};

#[path = "package_admission.rs"]
mod package_admission;

/// The journal's Windows release feed on sol pbc's own update origin.
pub const FEED_URL: &str = "https://updates.solstone.app/solstone-journal/release/windows";
const CHANNEL: &str =
    package_identity(solstone_core_installed_payload::compiled_windows_architecture()).channel;

#[derive(Clone, Copy)]
struct PackageIdentity {
    channel: &'static str,
    rid: &'static str,
    machine_architecture: &'static str,
}

const fn package_identity(
    architecture: solstone_core_installed_payload::WindowsArchitecture,
) -> PackageIdentity {
    use solstone_core_installed_payload::WindowsArchitecture;
    match architecture {
        WindowsArchitecture::X86_64 => PackageIdentity {
            channel: "win",
            rid: "win-x64",
            machine_architecture: "x64",
        },
        WindowsArchitecture::Aarch64 => PackageIdentity {
            channel: "win-arm64",
            rid: "win-arm64",
            machine_architecture: "arm64",
        },
    }
}

/// The first-party feed: one GET for the release list, then the package file
/// by name from the same place. Nothing else is sent.
#[derive(Clone)]
pub(crate) struct FirstPartyFeed {
    pub(crate) base: String,
}

// A feed entry may name only a sibling file, and neither request follows
// redirects. The updater must not contact a different host before verifying
// the downloaded package: that would already disclose an installation.
pub(crate) fn request_without_redirects(
    url: &str,
) -> Result<ureq::http::Response<ureq::Body>, Error> {
    let response = ureq::agent()
        .get(url)
        .config()
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|error| Error::Other(error.to_string()))?;
    if !response.status().is_success() {
        return Err(Error::Other(format!(
            "the update origin returned HTTP {}",
            response.status()
        )));
    }
    Ok(response)
}

impl FirstPartyFeed {
    pub(crate) fn asset_url(&self, file_name: &str) -> Result<String, Error> {
        if !asset_name_is_plain(file_name) {
            return Err(Error::Other(format!(
                "the update feed names a file outside itself: {file_name}"
            )));
        }
        Ok(format!("{}/{file_name}", self.base.trim_end_matches('/')))
    }

    pub(crate) fn get_release_feed(
        &self,
        channel: &str,
        _app: &Manifest,
        _staged_user_id: &str,
    ) -> Result<VelopackAssetFeed, Error> {
        let url = format!(
            "{}/releases.{channel}.json",
            self.base.trim_end_matches('/')
        );
        let json = request_without_redirects(&url)?
            .body_mut()
            .read_to_string()
            .map_err(|error| Error::Other(error.to_string()))?;
        serde_json::from_str(&json).map_err(Error::from)
    }
}

/// A package file the feed names must sit beside the feed itself.
pub(crate) fn asset_name_is_plain(file_name: &str) -> bool {
    !file_name.is_empty()
        && !file_name.starts_with('.')
        && !file_name.contains(['/', '\\', '?', '#', ':', '%'])
}

#[derive(Clone)]
enum FeedStore {
    Http(FirstPartyFeed),
    Dir(PathBuf),
}

#[derive(Clone)]
struct SignedFeed {
    store: FeedStore,
}

impl UpdateSource for SignedFeed {
    fn open_admitted_release_entry(
        &self,
        asset: &VelopackAsset,
        local_file: &Path,
    ) -> Result<Option<std::fs::File>, Error> {
        let sidecar = local_file.with_file_name(format!("{}.minisig", asset.FileName));
        package_admission::admit_existing_package(local_file, &sidecar, asset).map(Some)
    }

    fn get_release_feed(
        &self,
        channel: &str,
        app: &Manifest,
        staged_user_id: &str,
    ) -> Result<VelopackAssetFeed, Error> {
        match &self.store {
            FeedStore::Http(feed) => feed.get_release_feed(channel, app, staged_user_id),
            FeedStore::Dir(dir) => {
                let release_file = dir.join(format!("releases.{channel}.json"));
                let content = std::fs::read_to_string(&release_file)
                    .map_err(|e| Error::Other(format!("could not read release feed: {e}")))?;
                serde_json::from_str(&content)
                    .map_err(|e| Error::Other(format!("could not parse release feed: {e}")))
            }
        }
    }

    fn download_release_entry(
        &self,
        asset: &VelopackAsset,
        local_file: &Path,
        progress_sender: Option<Sender<i16>>,
    ) -> Result<(), Error> {
        match &self.store {
            FeedStore::Http(feed) => package_admission::download_http_package(
                feed,
                asset,
                local_file,
                progress_sender,
                || true,
                true,
            ),
            FeedStore::Dir(dir) => package_admission::download_dir_package(
                dir,
                asset,
                local_file,
                progress_sender,
                || true,
                true,
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionReport {
    pub known: bool,
    pub unrestricted: bool,
    pub roaming: bool,
    pub over_data_limit: bool,
    pub background_restricted: bool,
}

pub fn background_download_allowed(report: ConnectionReport) -> bool {
    report.known
        && report.unrestricted
        && !report.roaming
        && !report.over_data_limit
        && !report.background_restricted
}

pub fn deferral_reason(report: ConnectionReport) -> Option<&'static str> {
    if !report.known {
        Some("unknown")
    } else if report.roaming {
        Some("roaming")
    } else if !report.unrestricted {
        Some("metered")
    } else if report.over_data_limit || report.background_restricted {
        Some("restricted")
    } else {
        None
    }
}

#[cfg(windows)]
pub fn probe_connection() -> ConnectionReport {
    use windows::Networking::Connectivity::{
        NetworkConnectivityLevel, NetworkCostType, NetworkInformation,
    };

    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::System::Com::CoIncrementMTAUsage;
    use windows::Win32::System::WinRT::{
        RO_INIT_MULTITHREADED, RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize,
    };

    let unknown = || ConnectionReport {
        known: false,
        unrestricted: false,
        roaming: false,
        over_data_limit: false,
        background_restricted: false,
    };
    // windows-rs caches agile activation factories for the process. Keep one
    // MTA lease for that same lifetime, including between short-lived workers.
    // Releasing the last apartment per probe can invalidate the cached factory.
    static MTA_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*MTA_AVAILABLE.get_or_init(|| unsafe { CoIncrementMTAUsage() }.is_ok()) {
        return unknown();
    }
    let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.or_else(|error| {
        if error.code() == RPC_E_CHANGED_MODE {
            unsafe { RoInitialize(RO_INIT_SINGLETHREADED) }
        } else {
            Err(error)
        }
    });
    if initialized.is_err() {
        return unknown();
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { RoUninitialize() };
        }
    }
    let _apartment = Apartment;

    let profile = match NetworkInformation::GetInternetConnectionProfile() {
        Ok(p) => p,
        Err(_) => {
            return ConnectionReport {
                known: false,
                unrestricted: false,
                roaming: false,
                over_data_limit: false,
                background_restricted: false,
            };
        }
    };

    let connected = match profile.GetNetworkConnectivityLevel() {
        Ok(level) => level == NetworkConnectivityLevel::InternetAccess,
        Err(_) => false,
    };
    if !connected {
        return ConnectionReport {
            known: false,
            unrestricted: false,
            roaming: false,
            over_data_limit: false,
            background_restricted: false,
        };
    }

    let cost_obj = match profile.GetConnectionCost() {
        Ok(c) => c,
        Err(_) => {
            return ConnectionReport {
                known: false,
                unrestricted: false,
                roaming: false,
                over_data_limit: false,
                background_restricted: false,
            };
        }
    };

    let (Ok(roaming), Ok(over_data_limit), Ok(background_restricted), Ok(kind)) = (
        cost_obj.Roaming(),
        cost_obj.OverDataLimit(),
        cost_obj.BackgroundDataUsageRestricted(),
        cost_obj.NetworkCostType(),
    ) else {
        return unknown();
    };
    let unrestricted = match kind {
        NetworkCostType::Unrestricted => true,
        NetworkCostType::Fixed | NetworkCostType::Variable => false,
        _ => return unknown(),
    };

    ConnectionReport {
        known: true,
        unrestricted,
        roaming,
        over_data_limit,
        background_restricted,
    }
}

#[cfg(not(windows))]
pub fn probe_connection() -> ConnectionReport {
    ConnectionReport {
        known: false,
        unrestricted: false,
        roaming: false,
        over_data_limit: false,
        background_restricted: false,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchKind {
    Plain,
    SignIn,
    AfterUpdate,
}

pub type SpawnHelperFn = Box<dyn FnOnce(&Path, &VelopackAsset) -> Result<(), String> + Send>;

pub struct ApplyHooks {
    pub stop_journal: Box<dyn FnOnce() -> Result<(), String> + Send>,
    pub resume_journal: Box<dyn FnOnce() -> Result<(), String> + Send>,
    pub spawn_helper: SpawnHelperFn,
    pub exit_process: Box<dyn FnOnce(i32) + Send>,
}

fn production_stop_journal() -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::journal::stop_for_update()
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

fn production_resume_journal() -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::journal::restore_update_intent()
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

pub fn production_apply_hooks(feed: Option<&str>) -> ApplyHooks {
    let feed_str = feed.map(|s| s.to_string());
    ApplyHooks {
        stop_journal: Box::new(production_stop_journal),
        resume_journal: Box::new(production_resume_journal),
        spawn_helper: Box::new(move |packages_dir, asset| {
            let (manager, _, _, _) = build_manager(feed_str.as_deref());
            let Some(manager) = manager else {
                return Err("update manager unavailable".into());
            };
            spawn_verified_helper(&manager, packages_dir, asset).map_err(|e| e.to_string())
        }),
        exit_process: Box::new(|code| {
            std::process::exit(code);
        }),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartApply {
    NotCandidate,
    AppliedExiting,
    Deferred { failure: &'static str },
}

pub fn is_newer_version(candidate: &str, baseline: &str) -> bool {
    match (
        semver::Version::parse(candidate),
        semver::Version::parse(baseline),
    ) {
        (Ok(cand), Ok(base)) => cand > base,
        _ => false,
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PendingRecord {
    pub file: String,
}

pub fn read_pending_record() -> Option<PendingRecord> {
    let path = prefs::pending_update_path();
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn write_pending_record(record: &PendingRecord) -> Result<(), std::io::Error> {
    let path = prefs::pending_update_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = serde_json::to_string(record).map_err(std::io::Error::other)?;
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, content)?;
    std::fs::rename(&temp, &path)?;
    Ok(())
}

fn owned_package_version(name: &str) -> Option<String> {
    let version = name
        .strip_prefix("SolstoneJournal-")?
        .strip_suffix("-full.nupkg")?;
    let parsed = semver::Version::parse(version).ok()?;
    (parsed.to_string() == version).then(|| version.to_owned())
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn cache_operation_lock(directory: &Path) -> Result<velopack::lockfile::LockFile, Error> {
    std::fs::create_dir_all(directory)?;
    Ok(velopack::lockfile::LockFile::try_acquire_lock(
        directory.join(".solstone_update_lock"),
    )?)
}

fn retire_other_pending(
    directory: &Path,
    current: &str,
    keep: Option<&str>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let base = name.strip_suffix(".minisig").unwrap_or(name);
        if keep == Some(base) {
            continue;
        }
        if owned_package_version(base).is_some_and(|version| is_newer_version(&version, current)) {
            remove_if_present(&entry.path())?;
        }
    }
    Ok(())
}

// Cache recovery mutates only our canonical package namespace, under the same
// lock used by transfer and apply. Installed-version cache files remain intact.
fn hydrate_pending(directory: &Path, manager: &UpdateManager) -> Option<String> {
    let result = (|| -> Result<Option<String>, Error> {
        let _guard = cache_operation_lock(directory)?;
        let current = manager.get_current_version_as_string();
        let record = read_pending_record();
        let admitted = record.as_ref().and_then(|record| {
            owned_package_version(&record.file)?;
            package_admission::admit_cached_package(
                &directory.join(&record.file),
                &directory.join(format!("{}.minisig", record.file)),
            )
            .ok()
            .filter(|admitted| is_newer_version(&admitted.version, &current))
        });
        let keep = admitted
            .as_ref()
            .and(record.as_ref())
            .map(|record| record.file.as_str());
        retire_other_pending(directory, &current, keep)?;
        if admitted.is_none() {
            remove_if_present(&prefs::pending_update_path())?;
        }
        Ok(admitted.map(|admitted| admitted.version))
    })();
    match result {
        Ok(version) => version,
        Err(error) => {
            log::warn!("pending update recovery failed: {error}");
            None
        }
    }
}

fn apply_on_start_packages(
    packages_dir: &Path,
    current_version: &str,
    hooks: ApplyHooks,
) -> StartApply {
    let _guard = match cache_operation_lock(packages_dir) {
        Ok(guard) => guard,
        Err(error) => {
            log::warn!("update cache unavailable: {error}");
            return StartApply::Deferred { failure: "install" };
        }
    };
    let pending_path = prefs::pending_update_path();
    let Ok(content) = std::fs::read_to_string(&pending_path) else {
        return StartApply::NotCandidate;
    };
    let Ok(record) = serde_json::from_str::<PendingRecord>(&content) else {
        let _ = std::fs::remove_file(&pending_path);
        return StartApply::NotCandidate;
    };
    if owned_package_version(&record.file).is_none() {
        let _ = std::fs::remove_file(&pending_path);
        return StartApply::NotCandidate;
    }
    let package_path = packages_dir.join(&record.file);
    let sidecar_path = packages_dir.join(format!("{}.minisig", record.file));
    if !package_path.is_file() || !sidecar_path.is_file() {
        let _ = std::fs::remove_file(&pending_path);
        return StartApply::NotCandidate;
    }

    let admitted = match package_admission::admit_cached_package(&package_path, &sidecar_path) {
        Ok(admitted) => admitted,
        Err(e) => {
            log::warn!("cached package failed admission: {e}");
            let _ = std::fs::remove_file(&pending_path);
            let _ = std::fs::remove_file(&package_path);
            let _ = std::fs::remove_file(&sidecar_path);
            return StartApply::NotCandidate;
        }
    };

    if !is_newer_version(&admitted.version, current_version) {
        let _ = remove_if_present(&pending_path);
        return StartApply::NotCandidate;
    }

    let Ok(meta) = std::fs::metadata(&package_path) else {
        return StartApply::NotCandidate;
    };

    if let Err(e) = (hooks.stop_journal)() {
        log::warn!("stop_journal failed: {e}");
        return StartApply::Deferred { failure: "stop" };
    }

    let asset = VelopackAsset {
        FileName: record.file.clone(),
        PackageId: "SolstoneJournal".into(),
        Version: admitted.version.clone(),
        Type: "Full".into(),
        Size: meta.len(),
        ..Default::default()
    };

    match (hooks.spawn_helper)(packages_dir, &asset) {
        Ok(()) => {
            (hooks.exit_process)(0);
            StartApply::AppliedExiting
        }
        Err(e) => {
            log::warn!("spawn_helper failed: {e}");
            let _ = (hooks.resume_journal)();
            StartApply::Deferred { failure: "install" }
        }
    }
}

pub fn apply_on_start(acquired: bool, launch: LaunchKind, hooks: ApplyHooks) -> StartApply {
    if !acquired || launch == LaunchKind::AfterUpdate {
        return StartApply::NotCandidate;
    }
    let loc = match velopack::locator::auto_locate_app_manifest(
        velopack::locator::LocationContext::FromCurrentExe,
    ) {
        Ok(loc) => loc,
        Err(_) => return StartApply::NotCandidate,
    };
    let current_version = loc.get_manifest_version().to_string();
    let packages_dir = loc.get_packages_dir();
    apply_on_start_packages(&packages_dir, &current_version, hooks)
}

#[cfg(test)]
pub fn apply_on_start_test(
    acquired: bool,
    launch: LaunchKind,
    packages_dir: &Path,
    current_version: &str,
    hooks: ApplyHooks,
) -> StartApply {
    if !acquired || launch == LaunchKind::AfterUpdate {
        return StartApply::NotCandidate;
    }
    apply_on_start_packages(packages_dir, current_version, hooks)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Unavailable,
    Idle,
    Checking,
    Available,
    Downloading(i16),
    Ready,
    Installing,
    Failed(String),
}

fn phase_is_busy(phase: &Phase) -> bool {
    matches!(
        phase,
        Phase::Unavailable | Phase::Checking | Phase::Downloading(_) | Phase::Installing
    )
}

struct Inner {
    manager: Option<UpdateManager>,
    packages_dir: Option<PathBuf>,
    update_path: Option<PathBuf>,
    feed_store: FeedStore,
    phase: Phase,
    info: Option<UpdateInfo>,
    up_to_date: bool,
    prefs: Prefs,
    downloaded_version: Option<String>,
    failure: Option<&'static str>,
    deferral: Option<&'static str>,
    last_download_failure_at: Option<u64>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    probe: Arc<dyn Fn() -> ConnectionReport + Send + Sync>,
}

impl Inner {
    fn now(&self) -> u64 {
        (self.clock)()
    }

    fn probe(&self) -> ConnectionReport {
        (self.probe)()
    }
}

#[derive(Clone)]
pub struct Updater {
    inner: Arc<Mutex<Inner>>,
    notify: Arc<dyn Fn(Value) + Send + Sync>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// `<root>\current\bin\journal-app.exe` → `<root>`.
fn install_root() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
}

/// Velopack reads a per-install staging id from `packages\.betaId` and mints
/// a random one when the file is absent. An empty file means none is minted
/// and none is kept.
fn keep_staging_id_empty() {
    if let Some(root) = install_root() {
        let packages = root.join("packages");
        let _ = std::fs::create_dir_all(&packages);
        let _ = std::fs::write(packages.join(".betaId"), "");
    }
}

fn build_manager(
    feed: Option<&str>,
) -> (
    Option<UpdateManager>,
    Option<PathBuf>,
    FeedStore,
    Option<PathBuf>,
) {
    keep_staging_id_empty();
    let options = UpdateOptions {
        ExplicitChannel: Some(CHANNEL.to_owned()),
        MaximumDeltasBeforeFallback: -1,
        ..Default::default()
    };
    let located = velopack::locator::auto_locate_app_manifest(
        velopack::locator::LocationContext::FromCurrentExe,
    )
    .ok();
    let packages_dir = located.as_ref().map(|locator| locator.get_packages_dir());
    let update_path = located.as_ref().map(|locator| locator.get_update_path());
    let feed_store = match feed {
        Some(feed) if Path::new(feed).is_dir() => FeedStore::Dir(PathBuf::from(feed)),
        Some(feed) => FeedStore::Http(FirstPartyFeed {
            base: feed.to_owned(),
        }),
        None => FeedStore::Http(FirstPartyFeed {
            base: FEED_URL.to_owned(),
        }),
    };
    let Some(packages_dir) = packages_dir else {
        return (None, None, feed_store, None);
    };

    let source = SignedFeed {
        store: feed_store.clone(),
    };
    let manager = UpdateManager::new(source, Some(options), None).ok();
    if manager.is_some() {
        (manager, Some(packages_dir), feed_store, update_path)
    } else {
        (None, None, feed_store, None)
    }
}

impl Updater {
    pub fn new(feed: Option<&str>, notify: impl Fn(Value) + Send + Sync + 'static) -> Self {
        prefs::initialize();
        let (manager, packages_dir, feed_store, update_path) = build_manager(feed);
        let mut downloaded_version = None;
        let mut phase = if manager.is_some() {
            Phase::Idle
        } else {
            Phase::Unavailable
        };

        if let (Some(dir), Some(manager)) = (&packages_dir, &manager) {
            downloaded_version = hydrate_pending(dir, manager);
            if downloaded_version.is_some() {
                phase = Phase::Ready;
            }
        }

        Self {
            inner: Arc::new(Mutex::new(Inner {
                manager,
                packages_dir,
                update_path,
                feed_store,
                phase,
                info: None,
                up_to_date: false,
                prefs: prefs::load(),
                downloaded_version,
                failure: None,
                deferral: None,
                last_download_failure_at: None,
                clock: Arc::new(now),
                probe: Arc::new(probe_connection),
            })),
            notify: Arc::new(notify),
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    pub fn new_with_locator(
        feed: Option<&str>,
        locator: velopack::locator::VelopackLocatorConfig,
        notify: impl Fn(Value) + Send + Sync + 'static,
    ) -> Self {
        prefs::initialize();
        keep_staging_id_empty();
        let packages_dir = locator.PackagesDir.clone();
        let update_path = locator.UpdateExePath.clone();
        let options = UpdateOptions {
            ExplicitChannel: Some(CHANNEL.to_owned()),
            MaximumDeltasBeforeFallback: -1,
            ..Default::default()
        };
        let feed_store = match feed {
            Some(feed) if Path::new(feed).is_dir() => FeedStore::Dir(PathBuf::from(feed)),
            Some(feed) => FeedStore::Http(FirstPartyFeed {
                base: feed.to_owned(),
            }),
            None => FeedStore::Http(FirstPartyFeed {
                base: FEED_URL.to_owned(),
            }),
        };
        let source = SignedFeed {
            store: feed_store.clone(),
        };
        let manager = UpdateManager::new(source, Some(options), Some(locator)).ok();
        let mut downloaded_version = None;
        let mut phase = if manager.is_some() {
            Phase::Idle
        } else {
            Phase::Unavailable
        };

        if let Some(manager) = &manager {
            downloaded_version = hydrate_pending(&packages_dir, manager);
            if downloaded_version.is_some() {
                phase = Phase::Ready;
            }
        }

        Self {
            inner: Arc::new(Mutex::new(Inner {
                manager,
                packages_dir: Some(packages_dir),
                update_path: Some(update_path),
                feed_store,
                phase,
                info: None,
                up_to_date: false,
                prefs: prefs::load(),
                downloaded_version,
                failure: None,
                deferral: None,
                last_download_failure_at: None,
                clock: Arc::new(now),
                // Transport/state fixtures choose their cost explicitly. Real WinRT
                // lifetime is covered by the native probe regression and live app.
                probe: Arc::new(|| ConnectionReport {
                    known: false,
                    unrestricted: false,
                    roaming: false,
                    over_data_limit: false,
                    background_restricted: false,
                }),
            })),
            notify: Arc::new(notify),
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    pub fn set_clock(&self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) {
        self.inner.lock().expect("update state").clock = clock;
    }

    #[cfg(all(test, feature = "full-tests"))]
    pub fn set_probe(&self, probe: Arc<dyn Fn() -> ConnectionReport + Send + Sync>) {
        self.inner.lock().expect("update state").probe = probe;
    }

    pub fn now(&self) -> u64 {
        self.inner.lock().expect("update state").now()
    }

    pub fn probe(&self) -> ConnectionReport {
        self.inner.lock().expect("update state").probe()
    }

    pub fn view(&self) -> Value {
        let inner = self.inner.lock().expect("update state");
        let (phase_str, percent) = match &inner.phase {
            Phase::Unavailable => ("unavailable", None),
            Phase::Idle => ("idle", None),
            Phase::Checking => ("checking", None),
            Phase::Available => ("available", None),
            Phase::Downloading(pct) => ("downloading", Some(*pct)),
            Phase::Ready => ("ready", None),
            Phase::Installing => ("installing", None),
            Phase::Failed(_) => ("failed", None),
        };
        let current = inner
            .manager
            .as_ref()
            .map(UpdateManager::get_current_version_as_string);
        let next_start = match (&inner.downloaded_version, &current) {
            (Some(downloaded), Some(cur)) => is_newer_version(downloaded, cur),
            _ => false,
        };
        let target_version = inner.info.as_ref().and_then(|info| {
            let feed_ver = &info.TargetFullRelease.Version;
            match &inner.downloaded_version {
                Some(downloaded) if !is_newer_version(feed_ver, downloaded) => None,
                _ => Some(feed_ver.clone()),
            }
        });
        let up_to_date = inner.up_to_date && inner.downloaded_version.is_none();
        json!({
            "phase": phase_str,
            "percent": percent,
            "current": current,
            "last_checked_at": inner.prefs.last_checked_at,
            "auto_check": inner.prefs.auto_check,
            "auto_download": inner.prefs.auto_download,
            "interval": inner.prefs.interval,
            "downloaded_version": inner.downloaded_version,
            "target_version": target_version,
            "deferral": if inner.prefs.auto_download { inner.deferral } else { None },
            "failure": inner.failure,
            "next_start": next_start,
            "up_to_date": up_to_date,
        })
    }

    fn push(&self) {
        (self.notify)(self.view());
    }

    fn set_phase(&self, phase: Phase) {
        self.inner.lock().expect("update state").phase = phase;
        self.push();
    }

    fn set_download_percent(&self, percent: i16) {
        let mut inner = self.inner.lock().expect("update state");
        if matches!(inner.phase, Phase::Downloading(_)) {
            inner.phase = Phase::Downloading(percent);
        }
        drop(inner);
        self.push();
    }

    /// Check the feed. Runs on the caller's thread; call it off the window's.
    pub fn check(&self) {
        {
            let mut inner = self.inner.lock().expect("update state");
            if phase_is_busy(&inner.phase) {
                return;
            }
            inner.phase = Phase::Checking;
        }
        self.push();
        let result = {
            let inner = self.inner.lock().expect("update state");
            inner.manager.as_ref().map(UpdateManager::check_for_updates)
        };
        let now_ts = self.now();
        let auto_download = {
            let mut inner = self.inner.lock().expect("update state");
            match result {
                Some(Ok(UpdateCheck::UpdateAvailable(mut info))) => {
                    prefs::update_last_checked_at(now_ts);
                    inner.prefs.last_checked_at = Some(now_ts);

                    if !info.TargetFullRelease.Type.eq_ignore_ascii_case("Full") {
                        inner.info = None;
                        inner.up_to_date = false;
                        inner.failure = Some("check");
                        inner.phase = Phase::Failed("only full updates are supported".to_owned());
                        false
                    } else {
                        let target_v = &info.TargetFullRelease.Version;
                        let is_newer = match &inner.downloaded_version {
                            Some(d) => is_newer_version(target_v, d),
                            None => {
                                let cur = inner
                                    .manager
                                    .as_ref()
                                    .map(UpdateManager::get_current_version_as_string);
                                match cur {
                                    Some(c) => is_newer_version(target_v, &c),
                                    None => true,
                                }
                            }
                        };
                        if is_newer {
                            info.BaseRelease = None;
                            info.DeltasToTarget.clear();
                            inner.info = Some(*info);
                            inner.up_to_date = false;
                            inner.failure = None;
                            inner.phase = Phase::Available;
                            inner.prefs.auto_download
                        } else {
                            inner.info = None;
                            inner.failure = None;
                            if inner.downloaded_version.is_some() {
                                inner.phase = Phase::Ready;
                                inner.up_to_date = false;
                            } else {
                                inner.phase = Phase::Idle;
                                inner.up_to_date = true;
                            }
                            false
                        }
                    }
                }
                Some(Ok(UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty)) => {
                    prefs::update_last_checked_at(now_ts);
                    inner.prefs.last_checked_at = Some(now_ts);
                    inner.info = None;
                    inner.failure = None;
                    if inner.downloaded_version.is_some() {
                        inner.phase = Phase::Ready;
                        inner.up_to_date = false;
                    } else {
                        inner.phase = Phase::Idle;
                        inner.up_to_date = true;
                    }
                    false
                }
                Some(Err(error)) => {
                    prefs::update_last_checked_at(now_ts);
                    inner.prefs.last_checked_at = Some(now_ts);
                    log::warn!("update check failed: {error}");
                    inner.failure = Some("check");
                    inner.phase = Phase::Failed("the check failed".to_owned());
                    inner.up_to_date = false;
                    false
                }
                None => {
                    inner.phase = Phase::Unavailable;
                    inner.up_to_date = false;
                    false
                }
            }
        };
        self.push();
        if auto_download {
            self.download_background();
        }
    }

    /// Download the update the last check found, reporting progress.
    pub fn download(&self) {
        self.download_impl(false);
    }

    pub fn download_background(&self) {
        {
            let mut inner = self.inner.lock().expect("update state");
            if let Some(disk_prefs) = prefs::load_if_readable() {
                inner.prefs = disk_prefs;
            }
            if !inner.prefs.auto_download {
                return;
            }
        }
        self.download_impl(true);
    }

    fn download_impl(&self, is_background: bool) {
        let (info, packages_dir, feed_store, downloaded_version, update_path) = {
            let mut inner = self.inner.lock().expect("update state");
            if !matches!(inner.phase, Phase::Available | Phase::Failed(_))
                || inner.info.is_none()
                || inner.packages_dir.is_none()
            {
                return;
            }
            inner.phase = Phase::Downloading(0);
            inner.deferral = None;
            inner.failure = None;
            (
                inner.info.clone(),
                inner.packages_dir.clone(),
                inner.feed_store.clone(),
                inner.downloaded_version.clone(),
                inner.update_path.clone(),
            )
        };
        let Some(info) = info else { return };
        let Some(packages_dir) = packages_dir else {
            self.set_phase(Phase::Unavailable);
            return;
        };

        if downloaded_version
            .as_deref()
            .is_some_and(|d| !is_newer_version(&info.TargetFullRelease.Version, d))
        {
            self.set_phase(Phase::Ready);
            return;
        }

        if is_background {
            let probe = self.probe();
            if !background_download_allowed(probe) {
                let mut inner = self.inner.lock().expect("update state");
                inner.deferral = deferral_reason(probe);
                inner.last_download_failure_at = None;
                inner.failure = None;
                inner.phase = Phase::Available;
                drop(inner);
                self.push();
                return;
            }
        }

        let asset = info.TargetFullRelease;
        if owned_package_version(&asset.FileName).as_deref() != Some(&asset.Version) {
            let mut inner = self.inner.lock().expect("update state");
            inner.last_download_failure_at = Some(inner.now());
            inner.failure = Some("download");
            inner.phase = Phase::Failed("target package name is not plain".into());
            drop(inner);
            self.push();
            return;
        }

        let final_nupkg = packages_dir.join(&asset.FileName);
        let final_sidecar = packages_dir.join(format!("{}.minisig", asset.FileName));
        let temp_nupkg = packages_dir.join(&asset.FileName).with_extension("partial");
        let temp_sidecar = packages_dir.join(format!("{}.minisig.partial", asset.FileName));

        if final_nupkg.is_dir() {
            let mut inner = self.inner.lock().expect("update state");
            inner.last_download_failure_at = Some(inner.now());
            inner.failure = Some("download");
            inner.phase = Phase::Failed("target package path is a directory".into());
            drop(inner);
            self.push();
            return;
        }

        let _cache_guard = match cache_operation_lock(&packages_dir) {
            Ok(guard) => guard,
            Err(error) => {
                self.fail_download(error.to_string());
                return;
            }
        };
        self.push();
        let (sender, receiver) = channel::<i16>();
        let progress = self.clone();
        let reporter = std::thread::spawn(move || {
            for percent in receiver {
                progress.set_download_percent(percent);
            }
        });

        let updater_for_loop = self.clone();
        let is_allowed = move || {
            if is_background {
                background_download_allowed(updater_for_loop.probe())
            } else {
                true
            }
        };

        let dl_result = match feed_store {
            FeedStore::Http(feed) => package_admission::download_http_package(
                &feed,
                &asset,
                &temp_nupkg,
                Some(sender),
                is_allowed,
                false,
            ),
            FeedStore::Dir(dir) => package_admission::download_dir_package(
                &dir,
                &asset,
                &temp_nupkg,
                Some(sender),
                is_allowed,
                false,
            ),
        };

        let _ = reporter.join();

        if let Err(e) = dl_result {
            let _ = std::fs::remove_file(&temp_nupkg);
            let _ = std::fs::remove_file(&temp_sidecar);
            if package_admission::is_deferral_error(&e) {
                let probe = self.probe();
                let mut inner = self.inner.lock().expect("update state");
                inner.deferral = deferral_reason(probe);
                inner.last_download_failure_at = None;
                inner.failure = None;
                inner.phase = Phase::Available;
                drop(inner);
                self.push();
                return;
            } else {
                let mut inner = self.inner.lock().expect("update state");
                inner.deferral = None;
                inner.last_download_failure_at = Some(inner.now());
                inner.failure = Some("download");
                inner.phase = Phase::Failed("the download did not finish".into());
                drop(inner);
                self.push();
                return;
            }
        }

        let prepare_result = prepare_verified_helper(&temp_nupkg, &temp_sidecar, |package| {
            #[cfg(windows)]
            {
                let path = update_path
                    .as_deref()
                    .ok_or_else(|| Error::Other("update helper path unavailable".into()))?;
                extract_helper_if_needed(path, package)
            }
            #[cfg(not(windows))]
            {
                let _ = (update_path, package);
                Ok(())
            }
        });
        if let Err(error) = prepare_result {
            let _ = std::fs::remove_file(&temp_nupkg);
            let _ = std::fs::remove_file(&temp_sidecar);
            self.fail_download(error.to_string());
            return;
        }

        let post_delete_step = (|| -> Result<String, velopack::Error> {
            let current = self
                .inner
                .lock()
                .expect("update state")
                .manager
                .as_ref()
                .expect("installed update manager")
                .get_current_version_as_string();
            // Refuse promotion if any older pending package cannot be retired.
            retire_other_pending(&packages_dir, &current, Some(&asset.FileName))
                .map_err(|error| Error::Other(error.to_string()))?;
            std::fs::rename(&temp_nupkg, &final_nupkg)
                .map_err(|e| velopack::Error::Other(e.to_string()))?;
            std::fs::rename(&temp_sidecar, &final_sidecar)
                .map_err(|e| velopack::Error::Other(e.to_string()))?;
            write_pending_record(&PendingRecord {
                file: asset.FileName.clone(),
            })
            .map_err(|e| velopack::Error::Other(e.to_string()))?;

            // 4. admit_cached_package on final pair
            let admitted = package_admission::admit_cached_package(&final_nupkg, &final_sidecar)?;
            Ok(admitted.version)
        })();

        match post_delete_step {
            Ok(version) => {
                let mut inner = self.inner.lock().expect("update state");
                inner.downloaded_version = Some(version);
                inner.failure = None;
                inner.deferral = None;
                inner.phase = Phase::Ready;
                drop(inner);
                self.push();
            }
            Err(e) => {
                log::warn!("post-delete commit step failed: {e}");
                let _ = std::fs::remove_file(&temp_nupkg);
                let _ = std::fs::remove_file(&temp_sidecar);
                let _ = std::fs::remove_file(&final_nupkg);
                let _ = std::fs::remove_file(&final_sidecar);

                let remaining_version = read_pending_record().and_then(|rec| {
                    if asset_name_is_plain(&rec.file) {
                        let pkg = packages_dir.join(&rec.file);
                        let sig = packages_dir.join(format!("{}.minisig", rec.file));
                        package_admission::admit_cached_package(&pkg, &sig)
                            .ok()
                            .map(|a| a.version)
                    } else {
                        None
                    }
                });

                if remaining_version.is_none() {
                    let _ = std::fs::remove_file(prefs::pending_update_path());
                }

                let mut inner = self.inner.lock().expect("update state");
                inner.downloaded_version = remaining_version;
                inner.deferral = None;
                inner.last_download_failure_at = Some(inner.now());
                inner.failure = Some("download");
                inner.phase = Phase::Failed("the download did not finish".into());
                drop(inner);
                self.push();
            }
        }
    }

    fn fail_download(&self, error: String) {
        log::warn!("update download failed: {error}");
        let mut inner = self.inner.lock().expect("update state");
        inner.deferral = None;
        inner.last_download_failure_at = Some(inner.now());
        inner.failure = Some("download");
        inner.phase = Phase::Failed("the download did not finish".into());
        drop(inner);
        self.push();
    }

    /// Hand off to the installer. On success this process exits here and the
    /// installer opens the app again once the update is in place.
    pub fn install(&self) {
        let (packages_dir, current_version, manager) = {
            let mut inner = self.inner.lock().expect("update state");
            if phase_is_busy(&inner.phase) || inner.downloaded_version.is_none() {
                return;
            }
            inner.phase = Phase::Installing;
            let dir = inner.packages_dir.clone();
            let cur = inner
                .manager
                .as_ref()
                .map(UpdateManager::get_current_version_as_string)
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
            let mgr = inner.manager.clone();
            (dir, cur, mgr)
        };
        let Some(packages_dir) = packages_dir else {
            return;
        };
        let Some(manager) = manager else {
            return;
        };

        self.push();

        let hooks = ApplyHooks {
            stop_journal: Box::new(production_stop_journal),
            resume_journal: Box::new(production_resume_journal),
            spawn_helper: Box::new(move |packages_dir, asset| {
                spawn_verified_helper(&manager, packages_dir, asset).map_err(|e| e.to_string())
            }),
            exit_process: Box::new(|code| {
                std::process::exit(code);
            }),
        };

        let outcome = apply_on_start_packages(&packages_dir, &current_version, hooks);
        self.note_start(outcome);
    }

    #[cfg(all(test, feature = "full-tests"))]
    pub fn install_with_hooks(&self, hooks: ApplyHooks) {
        let (packages_dir, current_version) = {
            let mut inner = self.inner.lock().expect("update state");
            if phase_is_busy(&inner.phase) || inner.downloaded_version.is_none() {
                return;
            }
            inner.phase = Phase::Installing;
            let dir = inner.packages_dir.clone();
            let cur = inner
                .manager
                .as_ref()
                .map(UpdateManager::get_current_version_as_string)
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
            (dir, cur)
        };
        let Some(packages_dir) = packages_dir else {
            return;
        };

        self.push();
        let outcome = apply_on_start_packages(&packages_dir, &current_version, hooks);
        self.note_start(outcome);
    }

    pub fn note_start(&self, outcome: StartApply) {
        match outcome {
            StartApply::Deferred { failure } => {
                let mut inner = self.inner.lock().expect("update state");
                inner.failure = Some(failure);
                inner.phase = Phase::Failed("the update did not finish".into());
                drop(inner);
                self.push();
            }
            StartApply::NotCandidate => {
                let mut inner = self.inner.lock().expect("update state");
                if inner.phase == Phase::Installing {
                    inner.downloaded_version = None;
                    inner.failure = Some("install");
                    inner.phase =
                        Phase::Failed("the downloaded update is no longer available".into());
                    drop(inner);
                    self.push();
                }
            }
            StartApply::AppliedExiting => {}
        }
    }

    pub fn set_prefs(&self, auto_check: bool, auto_download: bool, interval: CheckInterval) {
        let updated = prefs::update_settings(auto_check, auto_download, interval);
        {
            let mut inner = self.inner.lock().expect("update state");
            inner.prefs = updated;
        }
        self.push();
    }

    /// Whether a scheduled check is due now.
    pub fn check_due(&self) -> bool {
        let inner = self.inner.lock().expect("update state");
        if !inner.prefs.auto_check {
            return false;
        }
        if matches!(
            inner.phase,
            Phase::Unavailable | Phase::Checking | Phase::Downloading(_) | Phase::Installing
        ) {
            return false;
        }
        let now = inner.now();
        inner
            .prefs
            .last_checked_at
            .is_none_or(|at| now.saturating_sub(at) >= inner.prefs.interval.seconds())
    }

    pub fn poll_tick(&self) {
        if let Some(disk_prefs) = prefs::load_if_readable() {
            let mut inner = self.inner.lock().expect("update state");
            inner.prefs = disk_prefs;
        }

        if self.check_due() {
            self.check();
            return;
        }

        let should_retry = {
            let mut inner = self.inner.lock().expect("update state");
            if let Some(disk_prefs) = prefs::load_if_readable() {
                inner.prefs = disk_prefs;
            }
            if !inner.prefs.auto_download || inner.info.is_none() {
                false
            } else if inner.deferral.is_some() {
                true
            } else if inner.failure == Some("download") {
                if let Some(fail_ts) = inner.last_download_failure_at {
                    inner.now().saturating_sub(fail_ts) >= 900
                } else {
                    false
                }
            } else {
                false
            }
        };

        if should_retry {
            self.download_background();
        }
    }

    /// Check on the owner's schedule for as long as the app runs.
    pub fn run_schedule(&self) {
        let updater = self.clone();
        std::thread::spawn(move || {
            // Let the window and the journal settle before the first check.
            std::thread::sleep(std::time::Duration::from_secs(30));
            loop {
                updater.poll_tick();
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        });
    }
}

// Keep the authenticated file open through helper extraction. On Windows the
// admission handle excludes writes/replacement, including through hard links.
fn prepare_verified_helper(
    package: &Path,
    sidecar: &Path,
    prepare: impl FnOnce(&Path) -> Result<(), Error>,
) -> Result<(), Error> {
    let admitted = package_admission::admit_cached_package(package, sidecar)?;
    let package_guard = admitted.file;
    prepare(package)?;
    drop(package_guard); // release before renaming the package on Windows
    Ok(())
}

#[cfg(windows)]
fn extract_helper_if_needed(
    update_exe_path: &Path,
    temp_nupkg: &Path,
) -> Result<(), velopack::Error> {
    let bundle = velopack::bundle::load_bundle_from_file(temp_nupkg)?;
    bundle.extract_zip_predicate_to_path(|f| f.ends_with("Squirrel.exe"), update_exe_path)?;
    Ok(())
}

fn spawn_verified_helper(
    manager: &UpdateManager,
    packages_dir: &Path,
    asset: &VelopackAsset,
) -> Result<(), Error> {
    let package_path = packages_dir.join(&asset.FileName);
    let sidecar_path = packages_dir.join(format!("{}.minisig", asset.FileName));
    let verified_file =
        package_admission::admit_existing_package(&package_path, &sidecar_path, asset)?;

    let symlink_meta = std::fs::symlink_metadata(&package_path)
        .map_err(|e| Error::Other(format!("could not re-query metadata before spawn: {e}")))?;
    if symlink_meta.file_type().is_symlink() || !symlink_meta.file_type().is_file() {
        return Err(Error::Other(
            "package is not a regular file immediately before spawn".into(),
        ));
    }
    let pre_spawn_id = package_admission::path_file_identity(&package_path)?;
    let file_id = package_admission::file_handle_identity(&verified_file)?;
    if pre_spawn_id != file_id {
        return Err(Error::Other(
            "package identity changed immediately before spawn".into(),
        ));
    }

    manager.wait_exit_then_apply_updates(asset, false, true, ["--after-update"])?;
    drop(verified_file);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::asset_name_is_plain;

    #[test]
    fn a_package_must_sit_beside_the_feed() {
        assert!(asset_name_is_plain("SolstoneJournal-2.0.28-full.nupkg"));
        for outside in [
            "",
            "../x.nupkg",
            "sub/x.nupkg",
            r"sub\x.nupkg",
            "https://elsewhere/x.nupkg",
            "x.nupkg?id=1",
            ".hidden",
            "x%2Fy.nupkg",
        ] {
            assert!(!asset_name_is_plain(outside), "{outside}");
        }
    }

    #[test]
    fn background_download_allowed_table() {
        use super::{ConnectionReport, background_download_allowed};

        let eligible = ConnectionReport {
            known: true,
            unrestricted: true,
            roaming: false,
            over_data_limit: false,
            background_restricted: false,
        };
        assert!(background_download_allowed(eligible));

        assert!(!background_download_allowed(ConnectionReport {
            known: false,
            ..eligible
        }));
        assert!(!background_download_allowed(ConnectionReport {
            unrestricted: false,
            ..eligible
        }));
        assert!(!background_download_allowed(ConnectionReport {
            roaming: true,
            ..eligible
        }));
        assert!(!background_download_allowed(ConnectionReport {
            over_data_limit: true,
            ..eligible
        }));
        assert!(!background_download_allowed(ConnectionReport {
            background_restricted: true,
            ..eligible
        }));
    }

    #[test]
    fn deferral_reason_priority() {
        use super::{ConnectionReport, deferral_reason};

        let eligible = ConnectionReport {
            known: true,
            unrestricted: true,
            roaming: false,
            over_data_limit: false,
            background_restricted: false,
        };
        assert_eq!(deferral_reason(eligible), None);

        let unknown = ConnectionReport {
            known: false,
            unrestricted: false,
            roaming: true,
            over_data_limit: true,
            background_restricted: true,
        };
        assert_eq!(deferral_reason(unknown), Some("unknown"));

        let roaming = ConnectionReport {
            known: true,
            unrestricted: false,
            roaming: true,
            over_data_limit: true,
            background_restricted: true,
        };
        assert_eq!(deferral_reason(roaming), Some("roaming"));

        let metered = ConnectionReport {
            known: true,
            unrestricted: false,
            roaming: false,
            over_data_limit: true,
            background_restricted: true,
        };
        assert_eq!(deferral_reason(metered), Some("metered"));

        let over_limit = ConnectionReport {
            known: true,
            unrestricted: true,
            roaming: false,
            over_data_limit: true,
            background_restricted: false,
        };
        assert_eq!(deferral_reason(over_limit), Some("restricted"));

        let background_restricted = ConnectionReport {
            known: true,
            unrestricted: true,
            roaming: false,
            over_data_limit: false,
            background_restricted: true,
        };
        assert_eq!(deferral_reason(background_restricted), Some("restricted"));
    }

    #[test]
    fn is_newer_version_table() {
        use super::is_newer_version;

        assert!(is_newer_version("2.0.0", "1.9.9"));
        assert!(is_newer_version("2.0.1", "2.0.0"));
        assert!(!is_newer_version("2.0.0", "2.0.0"));
        assert!(!is_newer_version("1.9.9", "2.0.0"));
        assert!(!is_newer_version("invalid", "2.0.0"));
        assert!(!is_newer_version("2.0.0", "invalid"));
        assert!(!is_newer_version("invalid", "invalid"));
    }

    #[test]
    fn apply_on_start_routine_short_circuits_before_files() {
        use super::{ApplyHooks, LaunchKind, StartApply, apply_on_start_test};
        use std::path::Path;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let stop_calls = std::sync::Arc::new(AtomicUsize::new(0));
        let resume_calls = std::sync::Arc::new(AtomicUsize::new(0));
        let spawn_calls = std::sync::Arc::new(AtomicUsize::new(0));
        let exit_calls = std::sync::Arc::new(AtomicUsize::new(0));

        let make_hooks = || {
            let st = stop_calls.clone();
            let re = resume_calls.clone();
            let sp = spawn_calls.clone();
            let ex = exit_calls.clone();
            ApplyHooks {
                stop_journal: Box::new(move || {
                    st.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
                resume_journal: Box::new(move || {
                    re.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
                spawn_helper: Box::new(move |_, _| {
                    sp.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
                exit_process: Box::new(move |_| {
                    ex.fetch_add(1, Ordering::SeqCst);
                }),
            }
        };

        let dummy_path = Path::new("");

        let res1 = apply_on_start_test(false, LaunchKind::Plain, dummy_path, "1.0.0", make_hooks());
        assert_eq!(res1, StartApply::NotCandidate);
        assert_eq!(stop_calls.load(Ordering::SeqCst), 0);
        assert_eq!(resume_calls.load(Ordering::SeqCst), 0);
        assert_eq!(spawn_calls.load(Ordering::SeqCst), 0);
        assert_eq!(exit_calls.load(Ordering::SeqCst), 0);

        let res2 = apply_on_start_test(
            true,
            LaunchKind::AfterUpdate,
            dummy_path,
            "1.0.0",
            make_hooks(),
        );
        assert_eq!(res2, StartApply::NotCandidate);
        assert_eq!(stop_calls.load(Ordering::SeqCst), 0);
        assert_eq!(resume_calls.load(Ordering::SeqCst), 0);
        assert_eq!(spawn_calls.load(Ordering::SeqCst), 0);
        assert_eq!(exit_calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn redirect_is_refused_before_contacting_its_destination() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::{Duration, Instant};

        let origin = TcpListener::bind("127.0.0.1:0").unwrap();
        let destination = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin_url = format!("http://{}", origin.local_addr().unwrap());
        let destination_url = format!(
            "http://localhost:{}",
            destination.local_addr().unwrap().port()
        );
        origin.set_nonblocking(true).unwrap();
        let origin_server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match origin.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "no origin request arrived");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("origin server: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).unwrap();
            write!(stream, "HTTP/1.1 302 Found\r\nLocation: {destination_url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        destination.set_nonblocking(true).unwrap();
        let trap = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                match destination.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nleak").unwrap();
                        return true;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("redirect trap: {error}"),
                }
            }
            false
        });
        let result = super::request_without_redirects(&origin_url);
        origin_server.join().unwrap();
        let contacted = trap.join().unwrap();
        assert!(result.is_err(), "redirect must refuse");
        assert!(!contacted, "redirect destination received a request");
    }

    #[cfg(all(test, feature = "full-tests"))]
    mod full_tests {
        use std::io::{Cursor, Read, Write};
        use std::net::TcpListener;
        use std::path::{Path, PathBuf};
        use std::sync::{Arc, OnceLock};
        use std::time::{Duration, Instant};

        use velopack::sources::UpdateSource;
        use velopack::{UpdateInfo, UpdateManager, VelopackAsset};
        use zip::ZipWriter;
        use zip::write::SimpleFileOptions;

        use super::super::prefs;
        use super::super::{
            ApplyHooks, CHANNEL, CheckInterval, ConnectionReport, FeedStore, FirstPartyFeed,
            LaunchKind, PackageIdentity, PendingRecord, Phase, SignedFeed, StartApply, Updater,
            apply_on_start_test, is_newer_version, package_identity, read_pending_record,
            retire_other_pending, spawn_verified_helper, write_pending_record,
        };

        struct TestKeys {
            _pin_dir: tempfile::TempDir,
            primary_sec: minisign::SecretKey,
            wrong_sec: minisign::SecretKey,
        }

        static TEST_KEYS: OnceLock<TestKeys> = OnceLock::new();

        fn get_test_keys() -> &'static TestKeys {
            TEST_KEYS.get_or_init(|| {
                let pin_dir = tempfile::tempdir().unwrap();
                let primary = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
                let public_file = pin_dir.path().join("fixture.pub");
                std::fs::write(&public_file, primary.pk.to_box().unwrap().to_string()).unwrap();
                solstone_core_installed_payload::install_test_fixture_pin(&public_file).unwrap();
                let wrong = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
                TestKeys {
                    _pin_dir: pin_dir,
                    primary_sec: primary.sk,
                    wrong_sec: wrong.sk,
                }
            })
        }

        fn sign_with_key(key: &minisign::SecretKey, data: &[u8]) -> Vec<u8> {
            minisign::sign(
                None,
                key,
                Cursor::new(data),
                Some("synthetic update fixture"),
                None,
            )
            .unwrap()
            .to_string()
            .into_bytes()
        }

        fn make_valid_nupkg(id: &str, version: &str, main_exe: &str) -> Vec<u8> {
            make_nupkg_for_architecture(
                id,
                version,
                main_exe,
                solstone_core_installed_payload::compiled_windows_architecture(),
            )
        }

        fn make_nupkg_for_architecture(
            id: &str,
            version: &str,
            main_exe: &str,
            architecture: solstone_core_installed_payload::WindowsArchitecture,
        ) -> Vec<u8> {
            let PackageIdentity {
                channel,
                rid,
                machine_architecture,
            } = package_identity(architecture);
            let nuspec = format!(
                r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://schemas.microsoft.com/packaging/2010/07/nuspec.xsd">
<metadata>
  <id>{id}</id>
  <version>{version}</version>
  <channel>{channel}</channel>
  <os>win</os>
  <rid>{rid}</rid>
  <machineArchitecture>{machine_architecture}</machineArchitecture>
  <mainExe>{main_exe}</mainExe>
</metadata>
</package>"#
            );
            let mut buf = Vec::new();
            let mut zip = ZipWriter::new(Cursor::new(&mut buf));
            zip.start_file(format!("{id}.nuspec"), SimpleFileOptions::default())
                .unwrap();
            zip.write_all(nuspec.as_bytes()).unwrap();
            zip.start_file("lib/net45/Squirrel.exe", SimpleFileOptions::default())
                .unwrap();
            let helper = if architecture
                == solstone_core_installed_payload::compiled_windows_architecture()
            {
                std::fs::read(get_test_recorder_binary()).unwrap()
            } else {
                b"synthetic foreign update helper".to_vec()
            };
            zip.write_all(&helper).unwrap();
            zip.finish().unwrap();
            buf
        }

        fn sha256_hex(data: &[u8]) -> String {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(data);
            format!("{:x}", hasher.finalize())
        }

        fn get_test_recorder_binary() -> &'static Path {
            static RECORDER: OnceLock<PathBuf> = OnceLock::new();
            RECORDER.get_or_init(|| {
                let temp_dir = tempfile::Builder::new()
                    .prefix("recorder")
                    .tempdir()
                    .unwrap()
                    .keep();
                let src = temp_dir.join("recorder.rs");
                let bin = temp_dir.join(if cfg!(windows) {
                    "recorder.exe"
                } else {
                    "recorder"
                });
                std::fs::write(
                    &src,
                    r#"
fn main() {
    let args: Vec<String> = std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect();
    let current_exe = std::env::current_exe().unwrap();
    let argv_file = current_exe.with_extension("argv");
    let pending_file = current_exe.with_extension("argv.pending");
    std::fs::write(&pending_file, args.join("\n")).unwrap();
    // Existence is the parent's completion signal; publish only the closed file.
    std::fs::rename(&pending_file, &argv_file).unwrap();
}
"#,
                )
                .unwrap();
                let status = std::process::Command::new("rustc")
                    .arg(&src)
                    .arg("-o")
                    .arg(&bin)
                    .status()
                    .expect("compile recorder with rustc");
                assert!(status.success());
                bin
            })
        }

        fn create_test_locator(temp_dir: &Path) -> velopack::locator::VelopackLocatorConfig {
            let packages_dir = temp_dir.join("packages");
            std::fs::create_dir_all(&packages_dir).unwrap();
            let current_dir = temp_dir.join("current");
            std::fs::create_dir_all(&current_dir).unwrap();

            let manifest_path = current_dir.join("SolstoneJournal.nuspec");
            std::fs::write(
                &manifest_path,
                r#"<?xml version="1.0" encoding="utf-8"?>
<package>
<metadata>
  <id>SolstoneJournal</id>
  <version>1.0.0</version>
  <mainExe>bin/journal-app.exe</mainExe>
</metadata>
</package>"#,
            )
            .unwrap();

            let update_exe = temp_dir.join(if cfg!(windows) {
                "Update.exe"
            } else {
                "Update"
            });
            let recorder_bin = get_test_recorder_binary();
            std::fs::copy(recorder_bin, &update_exe).unwrap();

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&update_exe).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&update_exe, perms).unwrap();
            }

            velopack::locator::VelopackLocatorConfig {
                RootAppDir: temp_dir.to_path_buf(),
                UpdateExePath: update_exe,
                PackagesDir: packages_dir,
                ManifestPath: manifest_path,
                CurrentBinaryDir: current_dir,
                IsPortable: true,
            }
        }

        #[test]
        fn signed_foreign_packages_cannot_replace_the_update_helper() {
            use solstone_core_installed_payload::{
                WindowsArchitecture, compiled_windows_architecture,
            };
            let keys = get_test_keys();
            let foreign = WindowsArchitecture::ALL
                .into_iter()
                .find(|architecture| *architecture != compiled_windows_architecture())
                .unwrap();
            for architecture in [compiled_windows_architecture(), foreign] {
                let bytes = make_nupkg_for_architecture(
                    "SolstoneJournal",
                    "2.0.28",
                    "bin/journal-app.exe",
                    architecture,
                );
                let signature = sign_with_key(&keys.primary_sec, &bytes);
                for cached in [false, true] {
                    let root = tempfile::tempdir().unwrap();
                    let feed_dir = root.path().join("feed");
                    std::fs::create_dir(&feed_dir).unwrap();
                    let name = "foreign-full.nupkg";
                    std::fs::write(feed_dir.join(name), &bytes).unwrap();
                    std::fs::write(feed_dir.join(format!("{name}.minisig")), &signature).unwrap();
                    let locator = create_test_locator(root.path());
                    let helper_before = std::fs::read(&locator.UpdateExePath).unwrap();
                    if cached {
                        std::fs::write(locator.PackagesDir.join(name), &bytes).unwrap();
                        std::fs::write(
                            locator.PackagesDir.join(format!("{name}.minisig")),
                            &signature,
                        )
                        .unwrap();
                    }
                    let info = velopack::UpdateInfo {
                        TargetFullRelease: VelopackAsset {
                            PackageId: "SolstoneJournal".into(),
                            Version: "2.0.28".into(),
                            Type: "Full".into(),
                            FileName: name.into(),
                            SHA256: sha256_hex(&bytes),
                            Size: bytes.len() as u64,
                            ..Default::default()
                        },
                        BaseRelease: None,
                        DeltasToTarget: vec![],
                        IsDowngrade: false,
                    };
                    let manager = UpdateManager::new(
                        SignedFeed {
                            store: FeedStore::Dir(feed_dir),
                        },
                        None,
                        Some(locator.clone()),
                    )
                    .unwrap();
                    assert_eq!(
                        manager.download_updates(&info, None).is_ok(),
                        architecture == compiled_windows_architecture()
                    );
                    assert_eq!(
                        std::fs::read(&locator.UpdateExePath).unwrap(),
                        helper_before
                    );
                }
            }
        }

        #[test]
        fn a_sibling_package_streams_to_disk_and_reports_progress() {
            let keys = get_test_keys();
            let pkg_bytes = make_valid_nupkg("SolstoneJournal", "2.0.28", "bin/journal-app.exe");
            let sig_bytes = sign_with_key(&keys.primary_sec, &pkg_bytes);

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let server_sig = sig_bytes.clone();
            let server_pkg = pkg_bytes.clone();

            let server = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                // Handle 2 requests: first sidecar, then package
                for expected_path in [
                    &b"GET /package.nupkg.minisig "[..],
                    &b"GET /package.nupkg "[..],
                ] {
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(Instant::now() < deadline, "request timeout");
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            Err(e) => panic!("server error: {e}"),
                        }
                    };
                    stream.set_nonblocking(false).unwrap();
                    let mut req = [0; 4096];
                    let size = stream.read(&mut req).unwrap();
                    assert!(req[..size].starts_with(expected_path));
                    assert!(!req[..size].contains(&b'?' as &u8));

                    let body = if expected_path == b"GET /package.nupkg.minisig " {
                        &server_sig
                    } else {
                        &server_pkg
                    };
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(body).unwrap();
                }
            });

            let feed = SignedFeed {
                store: FeedStore::Http(FirstPartyFeed {
                    base: format!("http://{addr}"),
                }),
            };
            let temp = tempfile::Builder::new().prefix("dl").tempdir().unwrap();
            let path = temp.path().join("package.nupkg.partial");
            let asset = VelopackAsset {
                PackageId: "SolstoneJournal".into(),
                Version: "2.0.28".into(),
                Type: "Full".into(),
                FileName: "package.nupkg".into(),
                SHA256: sha256_hex(&pkg_bytes),
                Size: pkg_bytes.len() as u64,
                ..Default::default()
            };

            let (sender, receiver) = std::sync::mpsc::channel();
            let result = feed.download_release_entry(&asset, &path, Some(sender));
            server.join().unwrap();
            result.expect("download and admission succeed");

            assert_eq!(std::fs::read(&path).unwrap(), pkg_bytes);
            let written_sidecar = temp.path().join("package.nupkg.minisig");
            assert_eq!(std::fs::read(written_sidecar).unwrap(), sig_bytes);
            assert_eq!(receiver.into_iter().last(), Some(100));
        }

        #[test]
        fn rejections_for_unsigned_wrong_key_tampered_and_mismatch() {
            let keys = get_test_keys();
            let pkg_bytes = make_valid_nupkg("SolstoneJournal", "2.0.28", "bin/journal-app.exe");
            let valid_sig = sign_with_key(&keys.primary_sec, &pkg_bytes);
            let wrong_sig = sign_with_key(&keys.wrong_sec, &pkg_bytes);

            let temp = tempfile::Builder::new().prefix("rej").tempdir().unwrap();
            let dir = temp.path().join("feed");
            std::fs::create_dir_all(&dir).unwrap();

            let feed = SignedFeed {
                store: FeedStore::Dir(dir.clone()),
            };

            let asset = VelopackAsset {
                PackageId: "SolstoneJournal".into(),
                Version: "2.0.28".into(),
                Type: "Full".into(),
                FileName: "pkg.nupkg".into(),
                SHA256: sha256_hex(&pkg_bytes),
                Size: pkg_bytes.len() as u64,
                ..Default::default()
            };

            // 1. Unsigned (no sidecar)
            std::fs::write(dir.join("pkg.nupkg"), &pkg_bytes).unwrap();
            let dest = temp.path().join("dest.partial");
            assert!(feed.download_release_entry(&asset, &dest, None).is_err());

            // 2. Malformed sidecar
            std::fs::write(dir.join("pkg.nupkg.minisig"), b"garbage sidecar").unwrap();
            assert!(feed.download_release_entry(&asset, &dest, None).is_err());

            // 3. Wrong key
            std::fs::write(dir.join("pkg.nupkg.minisig"), &wrong_sig).unwrap();
            assert!(feed.download_release_entry(&asset, &dest, None).is_err());

            // 4. Tampered package byte
            let mut tampered = pkg_bytes.clone();
            tampered[10] ^= 0xff;
            std::fs::write(dir.join("pkg.nupkg"), &tampered).unwrap();
            std::fs::write(dir.join("pkg.nupkg.minisig"), &valid_sig).unwrap();
            assert!(feed.download_release_entry(&asset, &dest, None).is_err());

            // 5. Signed id mismatch (id "Other" signed with primary key)
            let other_id_pkg = make_valid_nupkg("Other", "2.0.28", "bin/journal-app.exe");
            let other_id_sig = sign_with_key(&keys.primary_sec, &other_id_pkg);
            let other_asset = VelopackAsset {
                PackageId: "SolstoneJournal".into(),
                Version: "2.0.28".into(),
                Type: "Full".into(),
                FileName: "pkg.nupkg".into(),
                SHA256: sha256_hex(&other_id_pkg),
                Size: other_id_pkg.len() as u64,
                ..Default::default()
            };
            std::fs::write(dir.join("pkg.nupkg"), &other_id_pkg).unwrap();
            std::fs::write(dir.join("pkg.nupkg.minisig"), &other_id_sig).unwrap();
            assert!(
                feed.download_release_entry(&other_asset, &dest, None)
                    .is_err()
            );

            // 6. Signed version mismatch (version "2.0.29" vs asset "2.0.28")
            let other_ver_pkg =
                make_valid_nupkg("SolstoneJournal", "2.0.29", "bin/journal-app.exe");
            let other_ver_sig = sign_with_key(&keys.primary_sec, &other_ver_pkg);
            std::fs::write(dir.join("pkg.nupkg"), &other_ver_pkg).unwrap();
            std::fs::write(dir.join("pkg.nupkg.minisig"), &other_ver_sig).unwrap();
            assert!(feed.download_release_entry(&asset, &dest, None).is_err());
        }

        #[test]
        fn bad_zip_shapes_signed_refuse_through_download() {
            let keys = get_test_keys();
            let temp = tempfile::Builder::new().prefix("badzip").tempdir().unwrap();
            let dir = temp.path().join("feed");
            std::fs::create_dir_all(&dir).unwrap();
            let feed = SignedFeed {
                store: FeedStore::Dir(dir.clone()),
            };
            let dest = temp.path().join("dest.partial");

            // Two nuspec entries
            let nuspec1 = r#"<?xml version="1.0"?><package><metadata><id>SolstoneJournal</id><version>2.0.28</version><channel>win</channel><os>win</os><rid>win-x64</rid><machineArchitecture>x64</machineArchitecture><mainExe>bin/journal-app.exe</mainExe></metadata></package>"#;
            let mut buf = Vec::new();
            let mut zip = ZipWriter::new(Cursor::new(&mut buf));
            zip.start_file("A.nuspec", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(nuspec1.as_bytes()).unwrap();
            zip.start_file("B.nuspec", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(nuspec1.as_bytes()).unwrap();
            zip.finish().unwrap();

            let sig = sign_with_key(&keys.primary_sec, &buf);
            let asset = VelopackAsset {
                PackageId: "SolstoneJournal".into(),
                Version: "2.0.28".into(),
                Type: "Full".into(),
                FileName: "two.nupkg".into(),
                SHA256: sha256_hex(&buf),
                Size: buf.len() as u64,
                ..Default::default()
            };
            std::fs::write(dir.join("two.nupkg"), &buf).unwrap();
            std::fs::write(dir.join("two.nupkg.minisig"), &sig).unwrap();
            assert!(feed.download_release_entry(&asset, &dest, None).is_err());
        }

        struct AfterDownload {
            source: SignedFeed,
            corrupt: bool,
        }

        impl UpdateSource for AfterDownload {
            fn get_release_feed(
                &self,
                channel: &str,
                app: &velopack::bundle::Manifest,
                staged: &str,
            ) -> Result<velopack::VelopackAssetFeed, velopack::Error> {
                self.source.get_release_feed(channel, app, staged)
            }
            fn download_release_entry(
                &self,
                asset: &VelopackAsset,
                path: &Path,
                progress: Option<std::sync::mpsc::Sender<i16>>,
            ) -> Result<(), velopack::Error> {
                self.source.download_release_entry(asset, path, progress)?;
                if self.corrupt {
                    let mut bytes = std::fs::read(path)?;
                    bytes[10] ^= 1;
                    std::fs::write(path, bytes)?;
                }
                Ok(())
            }
            fn open_admitted_release_entry(
                &self,
                asset: &VelopackAsset,
                path: &Path,
            ) -> Result<Option<std::fs::File>, velopack::Error> {
                let guard = self.source.open_admitted_release_entry(asset, path)?;
                #[cfg(windows)]
                assert!(
                    std::fs::OpenOptions::new().write(true).open(path).is_err(),
                    "admitted file must deny writes through extraction"
                );
                Ok(guard)
            }
        }

        #[test]
        fn manager_reauthenticates_before_helper_extraction() {
            let keys = get_test_keys();
            for corrupt in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let locator = create_test_locator(temp.path());
                let helper = std::fs::read(&locator.UpdateExePath).unwrap();
                let dir = temp.path().join("feed");
                std::fs::create_dir(&dir).unwrap();
                let bytes = make_valid_nupkg("SolstoneJournal", "2.0.28", "bin/journal-app.exe");
                let name = "SolstoneJournal-2.0.28-full.nupkg";
                std::fs::write(dir.join(name), &bytes).unwrap();
                std::fs::write(
                    dir.join(format!("{name}.minisig")),
                    sign_with_key(&keys.primary_sec, &bytes),
                )
                .unwrap();
                let mut final_bytes = bytes.clone();
                if corrupt {
                    final_bytes[10] ^= 1;
                }
                let asset = VelopackAsset {
                    PackageId: "SolstoneJournal".into(),
                    Version: "2.0.28".into(),
                    Type: "Full".into(),
                    FileName: name.into(),
                    Size: bytes.len() as u64,
                    SHA256: sha256_hex(&final_bytes),
                    ..Default::default()
                };
                let source = AfterDownload {
                    source: SignedFeed {
                        store: FeedStore::Dir(dir),
                    },
                    corrupt,
                };
                let manager =
                    velopack::UpdateManager::new(source, None, Some(locator.clone())).unwrap();
                let info = velopack::UpdateInfo {
                    TargetFullRelease: asset,
                    BaseRelease: None,
                    DeltasToTarget: vec![],
                    IsDowngrade: false,
                };
                assert_eq!(manager.download_updates(&info, None).is_err(), corrupt);
                assert_eq!(std::fs::read(&locator.UpdateExePath).unwrap(), helper);
            }
        }

        #[test]
        fn partial_links_and_sidecar_failure_preserve_prior_pending() {
            let keys = get_test_keys();
            let temp = tempfile::tempdir().unwrap();
            let locator = create_test_locator(temp.path());
            let dir = temp.path().join("feed");
            std::fs::create_dir(&dir).unwrap();
            let bytes = make_valid_nupkg("SolstoneJournal", "2.0.28", "bin/journal-app.exe");
            let sig = sign_with_key(&keys.primary_sec, &bytes);
            let name = "SolstoneJournal-2.0.28-full.nupkg";
            std::fs::write(dir.join(name), &bytes).unwrap();
            std::fs::write(dir.join(format!("{name}.minisig")), &sig).unwrap();
            let asset = VelopackAsset {
                PackageId: "SolstoneJournal".into(),
                Version: "2.0.28".into(),
                Type: "Full".into(),
                FileName: name.into(),
                Size: bytes.len() as u64,
                SHA256: sha256_hex(&bytes),
                ..Default::default()
            };
            let prior = locator.PackagesDir.join("SolstoneJournal-prior-full.nupkg");
            let prior_sig = prior.with_file_name("SolstoneJournal-prior-full.nupkg.minisig");
            std::fs::write(&prior, &bytes).unwrap();
            std::fs::write(&prior_sig, &sig).unwrap();
            let helper = std::fs::read(&locator.UpdateExePath).unwrap();
            let partial = locator.PackagesDir.join(format!("{name}.partial"));
            let sidecar_partial = locator.PackagesDir.join(format!("{name}.minisig.partial"));
            let feed = SignedFeed {
                store: FeedStore::Dir(dir.clone()),
            };
            for path in [&partial, &sidecar_partial] {
                std::fs::hard_link(&locator.UpdateExePath, path).unwrap();
                assert!(feed.download_release_entry(&asset, &partial, None).is_err());
                assert_eq!(std::fs::read(&locator.UpdateExePath).unwrap(), helper);
                assert!(!path.exists());
            }
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&locator.UpdateExePath, &partial).unwrap();
                assert!(feed.download_release_entry(&asset, &partial, None).is_err());
                assert_eq!(std::fs::read(&locator.UpdateExePath).unwrap(), helper);
            }
            let blocked = locator.PackagesDir.join(format!("{name}.minisig"));
            std::fs::create_dir(&blocked).unwrap();
            let manager = velopack::UpdateManager::new(feed, None, Some(locator.clone())).unwrap();
            let info = velopack::UpdateInfo {
                TargetFullRelease: asset.clone(),
                BaseRelease: None,
                DeltasToTarget: vec![],
                IsDowngrade: false,
            };
            assert!(manager.download_updates(&info, None).is_err());
            assert!(
                super::super::package_admission::admit_existing_package(&prior, &prior_sig, &asset)
                    .is_ok()
            );
            assert_eq!(std::fs::read(&locator.UpdateExePath).unwrap(), helper);
            std::fs::remove_dir(blocked).unwrap();
            assert!(manager.download_updates(&info, None).is_ok());
            assert!(
                super::super::package_admission::admit_existing_package(
                    &locator.PackagesDir.join(name),
                    &locator.PackagesDir.join(format!("{name}.minisig")),
                    &asset
                )
                .is_ok()
            );
        }

        #[test]
        fn manager_check_skips_unsigned_cached_delta_base() {
            let temp = tempfile::tempdir().unwrap();
            let locator = create_test_locator(temp.path());
            let dir = temp.path().join("feed");
            std::fs::create_dir(&dir).unwrap();
            let cache = make_valid_nupkg("SolstoneJournal", "2.0.27", "bin/journal-app.exe");
            std::fs::write(
                locator
                    .PackagesDir
                    .join("SolstoneJournal-2.0.27-full.nupkg"),
                cache,
            )
            .unwrap();
            let full = VelopackAsset {
                PackageId: "SolstoneJournal".into(),
                Version: "2.0.28".into(),
                Type: "Full".into(),
                FileName: "SolstoneJournal-2.0.28-full.nupkg".into(),
                ..Default::default()
            };
            let delta = VelopackAsset {
                Type: "Delta".into(),
                FileName: "SolstoneJournal-2.0.28-delta.nupkg".into(),
                ..full.clone()
            };
            std::fs::write(
                dir.join(format!("releases.{CHANNEL}.json")),
                serde_json::to_vec(&serde_json::json!({"Assets": [full, delta]})).unwrap(),
            )
            .unwrap();
            let source = SignedFeed {
                store: FeedStore::Dir(dir),
            };
            let manager = velopack::UpdateManager::new(
                source,
                Some(velopack::UpdateOptions {
                    ExplicitChannel: Some(CHANNEL.into()),
                    MaximumDeltasBeforeFallback: -1,
                    ..Default::default()
                }),
                Some(locator),
            )
            .unwrap();
            let velopack::UpdateCheck::UpdateAvailable(info) = manager.check_for_updates().unwrap()
            else {
                panic!("new release expected")
            };
            assert!(info.BaseRelease.is_none());
            assert!(info.DeltasToTarget.is_empty());
        }

        #[test]
        fn directory_feed_signed_admits_unsigned_refuses() {
            let keys = get_test_keys();
            let pkg_bytes = make_valid_nupkg("SolstoneJournal", "2.0.28", "bin/journal-app.exe");
            let sig_bytes = sign_with_key(&keys.primary_sec, &pkg_bytes);

            let temp = tempfile::Builder::new()
                .prefix("dirfeed")
                .tempdir()
                .unwrap();
            let dir = temp.path().join("feed");
            std::fs::create_dir_all(&dir).unwrap();

            std::fs::write(dir.join("good.nupkg"), &pkg_bytes).unwrap();
            std::fs::write(dir.join("good.nupkg.minisig"), &sig_bytes).unwrap();
            std::fs::write(dir.join("unsigned.nupkg"), &pkg_bytes).unwrap();

            let feed = SignedFeed {
                store: FeedStore::Dir(dir),
            };

            let good_asset = VelopackAsset {
                PackageId: "SolstoneJournal".into(),
                Version: "2.0.28".into(),
                Type: "Full".into(),
                FileName: "good.nupkg".into(),
                Size: pkg_bytes.len() as u64,
                ..Default::default()
            };
            let unsigned_asset = VelopackAsset {
                PackageId: "SolstoneJournal".into(),
                Version: "2.0.28".into(),
                Type: "Full".into(),
                FileName: "unsigned.nupkg".into(),
                Size: pkg_bytes.len() as u64,
                ..Default::default()
            };

            let dest_good = temp.path().join("good.partial");
            assert!(
                feed.download_release_entry(&good_asset, &dest_good, None)
                    .is_ok()
            );

            let dest_unsigned = temp.path().join("unsigned.partial");
            assert!(
                feed.download_release_entry(&unsigned_asset, &dest_unsigned, None)
                    .is_err()
            );
        }

        #[test]
        fn cached_bad_final_preflight_and_preservation_of_other() {
            let _keys = get_test_keys();
            let good_pkg = make_valid_nupkg("SolstoneJournal", "2.0.28", "bin/journal-app.exe");

            let temp = tempfile::Builder::new().prefix("cached").tempdir().unwrap();
            prefs::with_test_state_dir(temp.path(), || {
                let locator = create_test_locator(temp.path());

                // Initial helper bytes
                let helper_bytes = std::fs::read(&locator.UpdateExePath).unwrap();

                // Place an existing other verified package
                let other_path = locator.PackagesDir.join("Other-1.0.0-full.nupkg");
                std::fs::write(&other_path, b"other package bytes").unwrap();

                // Place a bad cached final package and bad sidecar
                let bad_path = locator
                    .PackagesDir
                    .join("SolstoneJournal-2.0.28-full.nupkg");
                std::fs::write(&bad_path, b"corrupted nupkg").unwrap();
                let bad_sig_path = locator
                    .PackagesDir
                    .join("SolstoneJournal-2.0.28-full.nupkg.minisig");
                std::fs::write(&bad_sig_path, b"bad signature").unwrap();

                // Feed directory serving good pkg but bad sidecar, causing download to fail
                let feed_dir = temp.path().join("feed");
                std::fs::create_dir_all(&feed_dir).unwrap();
                std::fs::write(
                    feed_dir.join("SolstoneJournal-2.0.28-full.nupkg"),
                    &good_pkg,
                )
                .unwrap();
                std::fs::write(
                    feed_dir.join("SolstoneJournal-2.0.28-full.nupkg.minisig"),
                    b"bad feed signature",
                )
                .unwrap();

                let updater = Updater::new_with_locator(
                    Some(feed_dir.to_str().unwrap()),
                    locator.clone(),
                    |_| {},
                );

                let asset = VelopackAsset {
                    PackageId: "SolstoneJournal".into(),
                    Version: "2.0.28".into(),
                    Type: "Full".into(),
                    FileName: "SolstoneJournal-2.0.28-full.nupkg".into(),
                    Size: good_pkg.len() as u64,
                    SHA256: sha256_hex(&good_pkg),
                    ..Default::default()
                };

                {
                    let mut inner = updater.inner.lock().unwrap();
                    inner.info = Some(velopack::UpdateInfo {
                        TargetFullRelease: asset,
                        BaseRelease: None,
                        DeltasToTarget: Vec::new(),
                        IsDowngrade: false,
                    });
                    inner.phase = Phase::Available;
                }

                updater.download();

                // Download failed due to bad sidecar from feed
                let view = updater.view();
                assert_eq!(view["phase"], "failed");

                // Stub helper bytes are unchanged
                assert_eq!(std::fs::read(&locator.UpdateExePath).unwrap(), helper_bytes);

                // Other package remained untouched
                assert_eq!(std::fs::read(&other_path).unwrap(), b"other package bytes");

                // Extension: a directory occupying the target nupkg name
                let _ = std::fs::remove_file(&bad_path);
                std::fs::create_dir_all(&bad_path).unwrap();
                {
                    let mut inner = updater.inner.lock().unwrap();
                    inner.phase = Phase::Available;
                }
                updater.download();

                let view2 = updater.view();
                assert_eq!(view2["phase"], "failed");
                assert!(
                    bad_path.is_dir(),
                    "directory occupying target nupkg must still exist"
                );
                assert_eq!(std::fs::read(&other_path).unwrap(), b"other package bytes");
                std::fs::remove_dir(&bad_path).unwrap();
            });
        }

        #[test]
        fn apply_failure_and_spawn_verified_helper_argv_inspection() {
            let keys = get_test_keys();
            let good_pkg = make_valid_nupkg("SolstoneJournal", "2.0.28", "bin/journal-app.exe");
            let good_sig = sign_with_key(&keys.primary_sec, &good_pkg);

            let temp = tempfile::Builder::new().prefix("apply").tempdir().unwrap();
            prefs::with_test_state_dir(temp.path(), || {
                let locator = create_test_locator(temp.path());

                let feed_dir = temp.path().join("feed");
                std::fs::create_dir_all(&feed_dir).unwrap();
                let pkg_name = "SolstoneJournal-2.0.28-full.nupkg";
                let pkg_path = locator.PackagesDir.join(pkg_name);
                let sig_path = locator.PackagesDir.join(format!("{pkg_name}.minisig"));
                std::fs::write(&pkg_path, &good_pkg).unwrap();
                std::fs::write(&sig_path, &good_sig).unwrap();
                write_pending_record(&PendingRecord {
                    file: pkg_name.into(),
                })
                .unwrap();

                // 1. Test apply failure with install_with_hooks: simulated failure sets Phase::Failed
                let updater = Updater::new_with_locator(
                    Some(feed_dir.to_str().unwrap()),
                    locator.clone(),
                    |_| {},
                );

                let asset = VelopackAsset {
                    PackageId: "SolstoneJournal".into(),
                    Version: "2.0.28".into(),
                    Type: "Full".into(),
                    FileName: pkg_name.into(),
                    Size: good_pkg.len() as u64,
                    SHA256: sha256_hex(&good_pkg),
                    ..Default::default()
                };

                updater.install_with_hooks(ApplyHooks {
                    stop_journal: Box::new(|| Ok(())),
                    resume_journal: Box::new(|| Ok(())),
                    spawn_helper: Box::new(|_, _| Err("simulated apply failure".into())),
                    exit_process: Box::new(|_| {}),
                });
                let view = updater.view();
                assert_eq!(view["phase"], "failed");

                // Recorder argv file must NOT exist
                let argv_file = locator.UpdateExePath.with_extension("argv");
                assert!(!argv_file.exists());

                // 2. Recreate updater, check + download valid files
                std::fs::remove_file(&pkg_path).unwrap();
                std::fs::remove_file(&sig_path).unwrap();
                let _ = std::fs::remove_file(prefs::pending_update_path());
                std::fs::write(feed_dir.join(pkg_name), &good_pkg).unwrap();
                std::fs::write(feed_dir.join(format!("{pkg_name}.minisig")), &good_sig).unwrap();

                let release_feed = serde_json::json!({
                    "Assets": [asset.clone()]
                });
                std::fs::write(
                    feed_dir.join(format!("releases.{CHANNEL}.json")),
                    serde_json::to_string(&release_feed).unwrap(),
                )
                .unwrap();

                let updater2 = Updater::new_with_locator(
                    Some(feed_dir.to_str().unwrap()),
                    locator.clone(),
                    |_| {},
                );

                updater2.set_probe(Arc::new(|| ConnectionReport {
                    known: false,
                    unrestricted: false,
                    roaming: false,
                    over_data_limit: false,
                    background_restricted: false,
                }));
                updater2.check();
                assert_eq!(updater2.view()["phase"], "available");

                updater2.download();
                assert_eq!(updater2.view()["phase"], "ready");

                // Put a higher-version unsigned nupkg in packages dir before spawn_verified_helper
                let higher_unsigned = locator.PackagesDir.join("SolstoneJournal-3.0.0-full.nupkg");
                std::fs::write(&higher_unsigned, b"unsigned higher package").unwrap();

                // Call spawn_verified_helper to inspect argv without exiting
                let inner = updater2.inner.lock().unwrap();
                let manager = inner.manager.as_ref().unwrap();
                let packages_dir = inner.packages_dir.as_ref().unwrap();
                spawn_verified_helper(manager, packages_dir, &asset).expect("spawn succeeds");

                // Wait for helper process to finish writing argv
                let deadline = Instant::now() + Duration::from_secs(5);
                while !argv_file.exists() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(argv_file.exists(), "recorder must have recorded argv");
                let recorded_argv = std::fs::read_to_string(&argv_file).unwrap();
                let args: Vec<&str> = recorded_argv.lines().collect();

                // Argv must contain --package and the verified file path, NOT the unsigned 3.0.0 sibling
                let pkg_flag_idx =
                    args.iter()
                        .position(|&a| a == "--package")
                        .unwrap_or_else(|| {
                            panic!("--package flag missing from recorder argv: {args:?}")
                        });
                let target_arg = args[pkg_flag_idx + 1];
                assert!(target_arg.ends_with("SolstoneJournal-2.0.28-full.nupkg"));
                assert!(!target_arg.contains("SolstoneJournal-3.0.0-full.nupkg"));

                let after_update_idx = args
                    .iter()
                    .position(|&a| a == "--after-update")
                    .expect("--after-update flag");
                assert!(after_update_idx > pkg_flag_idx);
            });
        }

        #[test]
        fn obsolete_pending_and_tampered_manual_install_recover_without_hooks() {
            let keys = get_test_keys();
            let temp = tempfile::tempdir().unwrap();
            prefs::with_test_state_dir(temp.path(), || {
                let locator = create_test_locator(temp.path());
                let current = UpdateManager::new(
                    SignedFeed {
                        store: FeedStore::Dir(temp.path().to_owned()),
                    },
                    None,
                    Some(locator.clone()),
                )
                .unwrap()
                .get_current_version_as_string();
                for version in [&current, "0.0.1", "2.0.28"] {
                    let name = format!("SolstoneJournal-{version}-full.nupkg");
                    let bytes = make_valid_nupkg("SolstoneJournal", version, "bin/journal-app.exe");
                    let package = locator.PackagesDir.join(&name);
                    let sidecar = locator.PackagesDir.join(format!("{name}.minisig"));
                    std::fs::write(&package, &bytes).unwrap();
                    std::fs::write(&sidecar, sign_with_key(&keys.primary_sec, &bytes)).unwrap();
                    write_pending_record(&PendingRecord { file: name }).unwrap();
                    let updater = Updater::new_with_locator(None, locator.clone(), |_| {});
                    if !is_newer_version(version, &current) {
                        assert!(updater.view()["downloaded_version"].is_null());
                        assert_eq!(updater.view()["next_start"], false);
                        assert!(!prefs::pending_update_path().exists());
                    } else {
                        assert_eq!(updater.view()["phase"], "ready");
                        std::fs::write(&package, b"tampered after hydration").unwrap();
                        updater.install_with_hooks(ApplyHooks {
                            stop_journal: Box::new(|| panic!("tampering must precede quiesce")),
                            resume_journal: Box::new(|| panic!("no recovery hook needed")),
                            spawn_helper: Box::new(|_, _| panic!("tampering must precede helper")),
                            exit_process: Box::new(|_| panic!("tampering must not exit")),
                        });
                        assert_eq!(updater.view()["phase"], "failed");
                        assert_eq!(updater.view()["failure"], "install");
                        assert!(updater.view()["downloaded_version"].is_null());
                        assert!(!package.exists());
                        assert!(!sidecar.exists());
                        assert!(!prefs::pending_update_path().exists());
                        assert!(updater.check_due());
                    }
                }
            });
        }

        #[cfg(windows)]
        #[test]
        fn admitted_temp_stays_write_locked_until_helper_extraction_finishes() {
            let keys = get_test_keys();
            let temp = tempfile::tempdir().unwrap();
            let bytes = make_valid_nupkg("SolstoneJournal", "2.0.28", "bin/journal-app.exe");
            let package = temp.path().join("SolstoneJournal-2.0.28-full.partial");
            let sidecar = temp
                .path()
                .join("SolstoneJournal-2.0.28-full.nupkg.minisig.partial");
            let helper = temp.path().join("Update.exe");
            let alias = temp.path().join("same-package.partial");
            std::fs::write(&package, &bytes).unwrap();
            std::fs::write(&sidecar, sign_with_key(&keys.primary_sec, &bytes)).unwrap();
            std::fs::hard_link(&package, &alias).unwrap();
            super::super::prepare_verified_helper(&package, &sidecar, |path| {
                for target in [path, alias.as_path()] {
                    assert!(
                        std::fs::OpenOptions::new()
                            .write(true)
                            .open(target)
                            .is_err()
                    );
                }
                assert!(std::fs::rename(path, temp.path().join("replaced.partial")).is_err());
                super::super::extract_helper_if_needed(&helper, path)
            })
            .unwrap();
            assert_eq!(
                std::fs::read(&helper).unwrap(),
                std::fs::read(get_test_recorder_binary()).unwrap()
            );
            // Positive control: release of the admission handle permits a write.
            assert!(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&package)
                    .is_ok()
            );
        }

        #[cfg(windows)]
        #[test]
        fn connection_probe_survives_successive_and_concurrent_worker_exits() {
            let probe_twice = || {
                let first = super::super::probe_connection();
                let second = super::super::probe_connection();
                eprintln!("native connection probes: {first:?}; {second:?}");
            };
            // The process-static factory outlives each of these MTA workers.
            for _ in 0..3 {
                std::thread::spawn(probe_twice).join().unwrap();
            }
            let workers: Vec<_> = (0..4).map(|_| std::thread::spawn(probe_twice)).collect();
            for worker in workers {
                worker.join().unwrap();
            }
            probe_twice();
        }

        #[test]
        fn eligible_failure_after_cost_deferral_observes_retry_delay() {
            let _keys = get_test_keys();
            let temp = tempfile::tempdir().unwrap();
            prefs::with_test_state_dir(temp.path(), || {
                let locator = create_test_locator(temp.path());
                let updater = Updater::new_with_locator(None, locator, |_| {});
                let clock = Arc::new(std::sync::atomic::AtomicU64::new(100));
                let clock_read = clock.clone();
                updater.set_clock(Arc::new(move || {
                    clock_read.load(std::sync::atomic::Ordering::SeqCst)
                }));
                updater.set_probe(Arc::new(|| ConnectionReport {
                    known: true,
                    unrestricted: true,
                    roaming: false,
                    over_data_limit: false,
                    background_restricted: false,
                }));
                updater.set_prefs(false, true, CheckInterval::Day);
                {
                    let mut inner = updater.inner.lock().unwrap();
                    inner.phase = Phase::Available;
                    inner.deferral = Some("metered");
                    inner.feed_store = FeedStore::Dir(temp.path().join("missing-feed"));
                    inner.info = Some(UpdateInfo {
                        TargetFullRelease: VelopackAsset {
                            FileName: "SolstoneJournal-2.0.28-full.nupkg".into(),
                            Version: "2.0.28".into(),
                            Type: "Full".into(),
                            PackageId: "SolstoneJournal".into(),
                            ..Default::default()
                        },
                        BaseRelease: None,
                        DeltasToTarget: Vec::new(),
                        IsDowngrade: false,
                    });
                }
                updater.poll_tick();
                assert_eq!(updater.view()["failure"], "download");
                assert!(updater.view()["deferral"].is_null());
                assert_eq!(
                    updater.inner.lock().unwrap().last_download_failure_at,
                    Some(100)
                );
                clock.store(999, std::sync::atomic::Ordering::SeqCst);
                updater.poll_tick();
                assert_eq!(
                    updater.inner.lock().unwrap().last_download_failure_at,
                    Some(100)
                );
                clock.store(1000, std::sync::atomic::Ordering::SeqCst);
                updater.poll_tick();
                assert_eq!(
                    updater.inner.lock().unwrap().last_download_failure_at,
                    Some(1000)
                );
            });
        }

        #[test]
        fn failed_retirement_blocks_promotion_and_busy_install_preserves_transfer() {
            let temp = tempfile::tempdir().unwrap();
            let old = temp.path().join("SolstoneJournal-2.0.28-full.nupkg");
            std::fs::create_dir(&old).unwrap();
            assert!(
                retire_other_pending(
                    temp.path(),
                    "2.0.27",
                    Some("SolstoneJournal-2.0.29-full.nupkg")
                )
                .is_err()
            );
            prefs::with_test_state_dir(temp.path(), || {
                let locator = create_test_locator(temp.path());
                let updater = Updater::new_with_locator(None, locator, |_| {});
                {
                    let mut inner = updater.inner.lock().unwrap();
                    inner.phase = Phase::Downloading(42);
                    inner.downloaded_version = Some("2.0.28".into());
                }
                updater.install_with_hooks(ApplyHooks {
                    stop_journal: Box::new(|| panic!("transfer is busy")),
                    resume_journal: Box::new(|| panic!("transfer is busy")),
                    spawn_helper: Box::new(|_, _| panic!("transfer is busy")),
                    exit_process: Box::new(|_| panic!("transfer is busy")),
                });
                assert_eq!(updater.view()["phase"], "downloading");
                assert_eq!(updater.view()["percent"], 42);
            });
        }

        #[test]
        fn scheduled_check_due_intervals_phases_and_settings_reloading() {
            let temp = tempfile::tempdir().unwrap();
            prefs::with_test_state_dir(temp.path(), || {
                let locator = create_test_locator(temp.path());
                let clock = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1000));
                let clock_clone = clock.clone();

                let updater = Updater::new_with_locator(None, locator, |_| {});
                updater.set_clock(std::sync::Arc::new(move || {
                    clock_clone.load(std::sync::atomic::Ordering::SeqCst)
                }));

                prefs::update_last_checked_at(1000);
                updater.poll_tick();

                // Not due one second earlier (elapsed = 86399)
                clock.store(1000 + 86399, std::sync::atomic::Ordering::SeqCst);
                assert!(!updater.check_due());

                // Due when elapsed >= 86400 (including jump)
                clock.store(1000 + 86400, std::sync::atomic::Ordering::SeqCst);
                assert!(updater.check_due());

                // Available and Ready still due
                updater.set_phase(Phase::Available);
                assert!(updater.check_due());
                updater.set_phase(Phase::Ready);
                assert!(updater.check_due());

                // Checking, Downloading, Installing NOT due
                updater.set_phase(Phase::Checking);
                assert!(!updater.check_due());
                updater.set_phase(Phase::Downloading(50));
                assert!(!updater.check_due());
                updater.set_phase(Phase::Installing);
                assert!(!updater.check_due());

                // After test writes new interval or auto_check through update_settings,
                // next poll_tick uses the file, not the values from Updater::new
                updater.set_phase(Phase::Idle);
                prefs::update_settings(false, true, CheckInterval::Day);
                updater.poll_tick();
                assert!(!updater.check_due());

                prefs::update_settings(true, true, CheckInterval::Week);
                updater.poll_tick();
                // At 1000 + 86400, not due because interval is now Week (604800)
                assert!(!updater.check_due());
                clock.store(1000 + 604800, std::sync::atomic::Ordering::SeqCst);
                assert!(updater.check_due());
            });
        }

        #[test]
        fn automatic_download_failure_retry_math_and_deferral() {
            let temp = tempfile::tempdir().unwrap();
            prefs::with_test_state_dir(temp.path(), || {
                let locator = create_test_locator(temp.path());
                let clock = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1000));
                let clock_clone = clock.clone();

                let eligible_probe = std::sync::Arc::new(|| ConnectionReport {
                    known: true,
                    unrestricted: true,
                    roaming: false,
                    over_data_limit: false,
                    background_restricted: false,
                });
                let metered_probe = std::sync::Arc::new(|| ConnectionReport {
                    known: true,
                    unrestricted: false,
                    roaming: false,
                    over_data_limit: false,
                    background_restricted: false,
                });

                let updater = Updater::new_with_locator(None, locator, |_| {});
                updater.set_clock(std::sync::Arc::new(move || {
                    clock_clone.load(std::sync::atomic::Ordering::SeqCst)
                }));
                updater.set_probe(eligible_probe.clone());

                prefs::update_last_checked_at(1000);
                updater.poll_tick();

                let asset = VelopackAsset {
                    PackageId: "SolstoneJournal".into(),
                    Version: "2.0.28".into(),
                    Type: "Full".into(),
                    FileName: "SolstoneJournal-2.0.28-full.nupkg".into(),
                    ..Default::default()
                };
                {
                    let mut inner = updater.inner.lock().unwrap();
                    inner.info = Some(velopack::UpdateInfo {
                        TargetFullRelease: asset,
                        BaseRelease: None,
                        DeltasToTarget: Vec::new(),
                        IsDowngrade: false,
                    });
                    inner.phase = Phase::Failed("dl err".into());
                    inner.failure = Some("download");
                    inner.last_download_failure_at = Some(1000);
                }

                // At 1000 + 899: poll_tick does not start another download
                clock.store(1000 + 899, std::sync::atomic::Ordering::SeqCst);
                updater.poll_tick();
                assert_eq!(updater.view()["phase"], "failed");
                assert_eq!(updater.view()["last_checked_at"], 1000);

                // At 1000 + 900: with probe eligible and auto_download still true, starts one download
                clock.store(1000 + 900, std::sync::atomic::Ordering::SeqCst);
                updater.poll_tick();
                assert_eq!(updater.view()["last_checked_at"], 1000);

                // auto_download false in the file stops that attempt
                {
                    let mut inner = updater.inner.lock().unwrap();
                    inner.phase = Phase::Failed("dl err".into());
                    inner.failure = Some("download");
                    inner.last_download_failure_at = Some(2000);
                }
                prefs::update_settings(true, false, CheckInterval::Day);
                clock.store(2000 + 900, std::sync::atomic::Ordering::SeqCst);
                updater.poll_tick();
                assert_eq!(updater.view()["phase"], "failed");

                // Cost deferral starts on a later tick when probe becomes eligible,
                // without waiting 900 seconds and without a new daily check.
                prefs::update_settings(true, true, CheckInterval::Day);
                updater.set_probe(metered_probe);
                {
                    let mut inner = updater.inner.lock().unwrap();
                    inner.deferral = Some("metered");
                    inner.phase = Phase::Available;
                    inner.failure = None;
                }
                clock.store(3000, std::sync::atomic::Ordering::SeqCst);
                updater.poll_tick();
                assert_eq!(updater.view()["deferral"], "metered");

                // Probe becomes eligible: starts on later tick without waiting 900s
                updater.set_probe(eligible_probe);
                clock.store(3005, std::sync::atomic::Ordering::SeqCst);
                updater.poll_tick();
                assert_eq!(updater.view()["last_checked_at"], 1000);
            });
        }

        #[test]
        fn directory_listing_at_most_one_nupkg_and_restart_admission() {
            let keys = get_test_keys();
            let temp = tempfile::tempdir().unwrap();
            prefs::with_test_state_dir(temp.path(), || {
                let locator = create_test_locator(temp.path());
                let feed_dir = temp.path().join("feed");
                std::fs::create_dir_all(&feed_dir).unwrap();

                let pkg_a = make_valid_nupkg("SolstoneJournal", "2.0.28", "bin/journal-app.exe");
                let sig_a = sign_with_key(&keys.primary_sec, &pkg_a);
                let name_a = "SolstoneJournal-2.0.28-full.nupkg";
                std::fs::write(feed_dir.join(name_a), &pkg_a).unwrap();
                std::fs::write(feed_dir.join(format!("{name_a}.minisig")), &sig_a).unwrap();

                let asset_a = VelopackAsset {
                    PackageId: "SolstoneJournal".into(),
                    Version: "2.0.28".into(),
                    Type: "Full".into(),
                    FileName: name_a.into(),
                    Size: pkg_a.len() as u64,
                    SHA256: sha256_hex(&pkg_a),
                    ..Default::default()
                };
                let feed_json_a = serde_json::json!({ "Assets": [asset_a] });
                std::fs::write(
                    feed_dir.join(format!("releases.{CHANNEL}.json")),
                    serde_json::to_string(&feed_json_a).unwrap(),
                )
                .unwrap();

                let updater = Updater::new_with_locator(
                    Some(feed_dir.to_str().unwrap()),
                    locator.clone(),
                    |_| {},
                );
                updater.check();
                assert_eq!(updater.view()["phase"], "available");
                updater.download();
                assert_eq!(updater.view()["phase"], "ready");
                assert_eq!(updater.view()["downloaded_version"], "2.0.28");

                // Directory listing shows at most one *.nupkg
                let nupkgs: Vec<_> = std::fs::read_dir(&locator.PackagesDir)
                    .unwrap()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|ext| ext == "nupkg"))
                    .collect();
                assert_eq!(nupkgs.len(), 1);
                assert_eq!(nupkgs[0].file_name(), name_a);

                // Feed serves newer package B (2.0.29)
                let pkg_b = make_valid_nupkg("SolstoneJournal", "2.0.29", "bin/journal-app.exe");
                let sig_b = sign_with_key(&keys.primary_sec, &pkg_b);
                let name_b = "SolstoneJournal-2.0.29-full.nupkg";
                std::fs::write(feed_dir.join(name_b), &pkg_b).unwrap();
                std::fs::write(feed_dir.join(format!("{name_b}.minisig")), &sig_b).unwrap();
                let asset_b = VelopackAsset {
                    PackageId: "SolstoneJournal".into(),
                    Version: "2.0.29".into(),
                    Type: "Full".into(),
                    FileName: name_b.into(),
                    Size: pkg_b.len() as u64,
                    SHA256: sha256_hex(&pkg_b),
                    ..Default::default()
                };
                let feed_json_b = serde_json::json!({ "Assets": [asset_b] });
                std::fs::write(
                    feed_dir.join(format!("releases.{CHANNEL}.json")),
                    serde_json::to_string(&feed_json_b).unwrap(),
                )
                .unwrap();

                updater.check();
                updater.download();
                assert_eq!(updater.view()["phase"], "ready");
                assert_eq!(updater.view()["downloaded_version"], "2.0.29");

                // Directory listing still shows at most one *.nupkg
                let nupkgs2: Vec<_> = std::fs::read_dir(&locator.PackagesDir)
                    .unwrap()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|ext| ext == "nupkg"))
                    .collect();
                assert_eq!(nupkgs2.len(), 1);
                assert_eq!(nupkgs2[0].file_name(), name_b);
                assert!(!locator.PackagesDir.join(name_a).exists());

                // Same or older feed target does not replace valid pending package
                let feed_json_old = serde_json::json!({ "Assets": [asset_a] });
                std::fs::write(
                    feed_dir.join(format!("releases.{CHANNEL}.json")),
                    serde_json::to_string(&feed_json_old).unwrap(),
                )
                .unwrap();
                updater.check();
                updater.download();
                assert_eq!(updater.view()["downloaded_version"], "2.0.29");
                assert!(locator.PackagesDir.join(name_b).exists());

                // Restart: new Updater::new_with_locator admits pending file and view().downloaded_version is nuspec version
                let restarted = Updater::new_with_locator(
                    Some(feed_dir.to_str().unwrap()),
                    locator.clone(),
                    |_| {},
                );
                assert_eq!(restarted.view()["downloaded_version"], "2.0.29");
                assert_eq!(restarted.view()["phase"], "ready");

                // Replacement whose new bytes fail admission leaves previous file and downloaded_version still old
                let corrupted_pkg = b"corrupted nupkg bytes";
                let name_c = "SolstoneJournal-2.0.30-full.nupkg";
                std::fs::write(feed_dir.join(name_c), corrupted_pkg).unwrap();
                std::fs::write(feed_dir.join(format!("{name_c}.minisig")), b"corrupted sig")
                    .unwrap();
                let asset_c = VelopackAsset {
                    PackageId: "SolstoneJournal".into(),
                    Version: "2.0.30".into(),
                    Type: "Full".into(),
                    FileName: name_c.into(),
                    Size: corrupted_pkg.len() as u64,
                    SHA256: sha256_hex(corrupted_pkg),
                    ..Default::default()
                };
                let feed_json_c = serde_json::json!({ "Assets": [asset_c] });
                std::fs::write(
                    feed_dir.join(format!("releases.{CHANNEL}.json")),
                    serde_json::to_string(&feed_json_c).unwrap(),
                )
                .unwrap();

                restarted.check();
                restarted.download();
                assert_eq!(restarted.view()["phase"], "failed");
                // Leaves previous file and downloaded_version still old
                assert_eq!(restarted.view()["downloaded_version"], "2.0.29");
                assert!(locator.PackagesDir.join(name_b).exists());
            });
        }

        #[test]
        fn automatic_transfer_probe_flip_and_explicit_download() {
            let keys = get_test_keys();
            let temp = tempfile::tempdir().unwrap();
            prefs::with_test_state_dir(temp.path(), || {
                let locator = create_test_locator(temp.path());

                // Payload larger than 64 KiB
                let mut large_pkg =
                    make_valid_nupkg("SolstoneJournal", "2.0.28", "bin/journal-app.exe");
                large_pkg.extend(vec![0u8; 128 * 1024]);
                let large_sig = sign_with_key(&keys.primary_sec, &large_pkg);

                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let addr = listener.local_addr().unwrap();
                listener.set_nonblocking(true).unwrap();

                let s_sig = large_sig.clone();
                let s_pkg = large_pkg.clone();
                let server = std::thread::spawn(move || {
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while Instant::now() < deadline {
                        let Ok((mut stream, _)) = listener.accept() else {
                            std::thread::sleep(Duration::from_millis(10));
                            continue;
                        };
                        stream.set_nonblocking(false).unwrap();
                        let mut req = [0; 4096];
                        let n = stream.read(&mut req).unwrap_or(0);
                        let req_str = String::from_utf8_lossy(&req[..n]);
                        if req_str.contains(".minisig") {
                            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", s_sig.len()).unwrap();
                            stream.write_all(&s_sig).unwrap();
                        } else {
                            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", s_pkg.len()).unwrap();
                            stream.write_all(&s_pkg).unwrap();
                        }
                    }
                });

                let feed_base = format!("http://{addr}");
                let name = "SolstoneJournal-2.0.28-full.nupkg";
                let asset = VelopackAsset {
                    PackageId: "SolstoneJournal".into(),
                    Version: "2.0.28".into(),
                    Type: "Full".into(),
                    FileName: name.into(),
                    Size: large_pkg.len() as u64,
                    SHA256: sha256_hex(&large_pkg),
                    ..Default::default()
                };

                let probe_allowed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
                let p_clone = probe_allowed.clone();

                let updater = Updater::new_with_locator(Some(&feed_base), locator.clone(), |_| {});
                updater.set_probe(std::sync::Arc::new(move || {
                    let allowed = p_clone.load(std::sync::atomic::Ordering::SeqCst);
                    ConnectionReport {
                        known: true,
                        unrestricted: allowed,
                        roaming: false,
                        over_data_limit: false,
                        background_restricted: false,
                    }
                }));

                let prior = locator.PackagesDir.join("prior.nupkg");
                std::fs::write(&prior, b"prior package bytes").unwrap();

                {
                    let mut inner = updater.inner.lock().unwrap();
                    inner.info = Some(velopack::UpdateInfo {
                        TargetFullRelease: asset.clone(),
                        BaseRelease: None,
                        DeltasToTarget: Vec::new(),
                        IsDowngrade: false,
                    });
                    inner.phase = Phase::Available;
                }

                probe_allowed.store(false, std::sync::atomic::Ordering::SeqCst);
                updater.download_background();

                assert!(!locator.PackagesDir.join(format!("{name}.partial")).exists());
                assert!(prior.exists());

                updater.download();
                assert_eq!(updater.view()["phase"], "ready");
                assert_eq!(updater.view()["downloaded_version"], "2.0.28");
                drop(server);
            });
        }

        #[test]
        fn apply_on_start_test_and_install_with_hooks_fakes() {
            let keys = get_test_keys();
            let temp = tempfile::tempdir().unwrap();
            prefs::with_test_state_dir(temp.path(), || {
                let locator = create_test_locator(temp.path());

                let pkg_bytes =
                    make_valid_nupkg("SolstoneJournal", "2.0.28", "bin/journal-app.exe");
                let sig_bytes = sign_with_key(&keys.primary_sec, &pkg_bytes);
                let pkg_name = "SolstoneJournal-2.0.28-full.nupkg";
                let pkg_path = locator.PackagesDir.join(pkg_name);
                let sig_path = locator.PackagesDir.join(format!("{pkg_name}.minisig"));

                // 1. Tampered package or sidecar is deleted and spawn is not called
                std::fs::write(&pkg_path, b"tampered bytes").unwrap();
                std::fs::write(&sig_path, &sig_bytes).unwrap();
                write_pending_record(&PendingRecord {
                    file: pkg_name.into(),
                })
                .unwrap();

                let spawn_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let sp = spawn_called.clone();
                let hooks = ApplyHooks {
                    stop_journal: Box::new(|| Ok(())),
                    resume_journal: Box::new(|| Ok(())),
                    spawn_helper: Box::new(move |_, _| {
                        sp.store(true, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    }),
                    exit_process: Box::new(|_| {}),
                };
                let res = apply_on_start_test(
                    true,
                    LaunchKind::Plain,
                    &locator.PackagesDir,
                    "1.0.0",
                    hooks,
                );
                assert_eq!(res, StartApply::NotCandidate);
                assert!(!spawn_called.load(std::sync::atomic::Ordering::SeqCst));
                assert!(!pkg_path.exists());
                assert!(!sig_path.exists());
                assert!(read_pending_record().is_none());

                // 2. Cached newer package: stop_journal is called once even when journal was not running
                std::fs::write(&pkg_path, &pkg_bytes).unwrap();
                std::fs::write(&sig_path, &sig_bytes).unwrap();
                write_pending_record(&PendingRecord {
                    file: pkg_name.into(),
                })
                .unwrap();

                let stop_called = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let sc = stop_called.clone();
                let exited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let ex = exited.clone();
                let hooks_ok = ApplyHooks {
                    stop_journal: Box::new(move || {
                        sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    }),
                    resume_journal: Box::new(|| Ok(())),
                    spawn_helper: Box::new(|_, _| Ok(())),
                    exit_process: Box::new(move |code| {
                        assert_eq!(code, 0);
                        ex.store(true, std::sync::atomic::Ordering::SeqCst);
                    }),
                };
                let res_ok = apply_on_start_test(
                    true,
                    LaunchKind::Plain,
                    &locator.PackagesDir,
                    "1.0.0",
                    hooks_ok,
                );
                assert_eq!(res_ok, StartApply::AppliedExiting);
                assert_eq!(stop_called.load(std::sync::atomic::Ordering::SeqCst), 1);
                assert!(exited.load(std::sync::atomic::Ordering::SeqCst));

                // 3. Stop failure: spawn 0, resume 0, failure "stop"
                let stop_called2 = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let sc2 = stop_called2.clone();
                let resume_called = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let rc = resume_called.clone();
                let spawn_called2 = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let sp2 = spawn_called2.clone();
                let hooks_stop_fail = ApplyHooks {
                    stop_journal: Box::new(move || {
                        sc2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Err("stop err".into())
                    }),
                    resume_journal: Box::new(move || {
                        rc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    }),
                    spawn_helper: Box::new(move |_, _| {
                        sp2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        panic!("spawn must not be called")
                    }),
                    exit_process: Box::new(|_| {}),
                };
                let res_stop_fail = apply_on_start_test(
                    true,
                    LaunchKind::Plain,
                    &locator.PackagesDir,
                    "1.0.0",
                    hooks_stop_fail,
                );
                assert_eq!(res_stop_fail, StartApply::Deferred { failure: "stop" });
                assert_eq!(stop_called2.load(std::sync::atomic::Ordering::SeqCst), 1);
                assert_eq!(spawn_called2.load(std::sync::atomic::Ordering::SeqCst), 0);
                assert_eq!(resume_called.load(std::sync::atomic::Ordering::SeqCst), 0);

                // 4. Helper failure: resume (restore) is called once whether or not journal was running
                let resume_called2 = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let rc2 = resume_called2.clone();
                let hooks_helper_fail_running = ApplyHooks {
                    stop_journal: Box::new(|| Ok(())),
                    resume_journal: Box::new(move || {
                        rc2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    }),
                    spawn_helper: Box::new(|_, _| Err("helper failed".into())),
                    exit_process: Box::new(|_| {}),
                };
                let res_hf = apply_on_start_test(
                    true,
                    LaunchKind::Plain,
                    &locator.PackagesDir,
                    "1.0.0",
                    hooks_helper_fail_running,
                );
                assert_eq!(res_hf, StartApply::Deferred { failure: "install" });
                assert_eq!(resume_called2.load(std::sync::atomic::Ordering::SeqCst), 1);

                // 5. Helper failure without journal running still calls resume once
                let resume_called3 = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let rc3 = resume_called3.clone();
                let hooks_helper_fail_not_running = ApplyHooks {
                    stop_journal: Box::new(|| Ok(())),
                    resume_journal: Box::new(move || {
                        rc3.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    }),
                    spawn_helper: Box::new(|_, _| Err("helper failed".into())),
                    exit_process: Box::new(|_| {}),
                };
                let res_hf_nr = apply_on_start_test(
                    true,
                    LaunchKind::Plain,
                    &locator.PackagesDir,
                    "1.0.0",
                    hooks_helper_fail_not_running,
                );
                assert_eq!(res_hf_nr, StartApply::Deferred { failure: "install" });
                assert_eq!(resume_called3.load(std::sync::atomic::Ordering::SeqCst), 1);

                // 6. install_with_hooks hits those same fakes for one of those cases
                let resume_called4 = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let rc4 = resume_called4.clone();
                let hooks_for_install = ApplyHooks {
                    stop_journal: Box::new(|| Ok(())),
                    resume_journal: Box::new(move || {
                        rc4.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    }),
                    spawn_helper: Box::new(|_, _| Err("helper failed".into())),
                    exit_process: Box::new(|_| {}),
                };
                let updater = Updater::new_with_locator(None, locator.clone(), |_| {});
                updater.install_with_hooks(hooks_for_install);
                assert_eq!(updater.view()["phase"], "failed");
                assert_eq!(updater.view()["failure"], "install");
                assert_eq!(resume_called4.load(std::sync::atomic::Ordering::SeqCst), 1);
            });
        }

        #[test]
        fn delta_rejections_and_plan_clearing() {
            let keys = get_test_keys();
            let pkg_bytes = make_valid_nupkg("SolstoneJournal", "2.0.28", "bin/journal-app.exe");
            let sig_bytes = sign_with_key(&keys.primary_sec, &pkg_bytes);

            let temp = tempfile::Builder::new().prefix("delta").tempdir().unwrap();
            let locator = create_test_locator(temp.path());

            let feed_dir = temp.path().join("feed");
            std::fs::create_dir_all(&feed_dir).unwrap();
            let pkg_name = "SolstoneJournal-2.0.28-full.nupkg";
            std::fs::write(feed_dir.join(pkg_name), &pkg_bytes).unwrap();
            std::fs::write(feed_dir.join(format!("{pkg_name}.minisig")), &sig_bytes).unwrap();

            let full_asset = VelopackAsset {
                PackageId: "SolstoneJournal".into(),
                Version: "2.0.28".into(),
                Type: "Full".into(),
                FileName: pkg_name.into(),
                Size: pkg_bytes.len() as u64,
                SHA256: sha256_hex(&pkg_bytes),
                ..Default::default()
            };

            let delta_asset = VelopackAsset {
                PackageId: "SolstoneJournal".into(),
                Version: "2.0.28".into(),
                Type: "Delta".into(),
                FileName: "SolstoneJournal-2.0.28-delta.nupkg".into(),
                Size: 100,
                SHA256: "abc".into(),
                ..Default::default()
            };

            let release_feed = serde_json::json!({
                "Assets": [full_asset, delta_asset]
            });
            std::fs::write(
                feed_dir.join(format!("releases.{CHANNEL}.json")),
                serde_json::to_string(&release_feed).unwrap(),
            )
            .unwrap();

            let updater = Updater::new_with_locator(
                Some(feed_dir.to_str().unwrap()),
                locator.clone(),
                |_| {},
            );

            updater.check();
            // Stored info must have BaseRelease == None and DeltasToTarget empty
            {
                let inner = updater.inner.lock().unwrap();
                let info = inner.info.as_ref().unwrap();
                assert!(info.BaseRelease.is_none());
                assert!(info.DeltasToTarget.is_empty());
            }

            // Direct Type: Delta download_release_entry returns Err
            let feed = SignedFeed {
                store: FeedStore::Dir(feed_dir),
            };
            let dummy_dest = temp.path().join("dummy.partial");
            assert!(
                feed.download_release_entry(&delta_asset, &dummy_dest, None)
                    .is_err()
            );
        }

        #[test]
        fn non_plain_filename_does_not_delete_outside_packages_dir() {
            let _keys = get_test_keys();
            let temp = tempfile::Builder::new()
                .prefix("outside")
                .tempdir()
                .unwrap();
            let locator = create_test_locator(temp.path());

            // A file outside packages_dir (in the temp root)
            let outside_file = temp.path().join("victim.txt");
            std::fs::write(&outside_file, b"victim content").unwrap();

            let initial_helper_bytes = std::fs::read(&locator.UpdateExePath).unwrap();

            let updater = Updater::new_with_locator(None, locator.clone(), |_| {});

            let traversal_name =
                format!("../{}", outside_file.file_name().unwrap().to_str().unwrap());
            let asset = VelopackAsset {
                PackageId: "SolstoneJournal".into(),
                Version: "2.0.28".into(),
                Type: "Full".into(),
                FileName: traversal_name,
                Size: 100,
                SHA256: "0".repeat(64),
                ..Default::default()
            };

            {
                let mut inner = updater.inner.lock().unwrap();
                inner.info = Some(velopack::UpdateInfo {
                    TargetFullRelease: asset,
                    BaseRelease: None,
                    DeltasToTarget: Vec::new(),
                    IsDowngrade: false,
                });
                inner.phase = Phase::Available;
            }

            updater.download();

            // Download must fail
            assert_eq!(updater.view()["phase"], "failed");

            // Outside file must still exist with unchanged content
            assert!(outside_file.exists());
            assert_eq!(std::fs::read(&outside_file).unwrap(), b"victim content");

            // Helper must not be replaced, and no argv file
            assert_eq!(
                std::fs::read(&locator.UpdateExePath).unwrap(),
                initial_helper_bytes
            );
            let argv_file = locator.UpdateExePath.with_extension("argv");
            assert!(!argv_file.exists());
        }
    }
}
