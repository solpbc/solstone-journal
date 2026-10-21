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
const CHANNEL: &str = "win";

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
            FeedStore::Http(feed) => {
                package_admission::download_http_package(feed, asset, local_file, progress_sender)
            }
            FeedStore::Dir(dir) => {
                package_admission::download_dir_package(dir, asset, local_file, progress_sender)
            }
        }
    }
}

#[derive(Clone, Debug)]
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

struct Inner {
    manager: Option<UpdateManager>,
    packages_dir: Option<PathBuf>,
    phase: Phase,
    info: Option<UpdateInfo>,
    up_to_date: bool,
    prefs: Prefs,
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

fn build_manager(feed: Option<&str>) -> (Option<UpdateManager>, Option<PathBuf>) {
    keep_staging_id_empty();
    let options = UpdateOptions {
        ExplicitChannel: Some(CHANNEL.to_owned()),
        ..Default::default()
    };
    let packages_dir = match velopack::locator::auto_locate_app_manifest(
        velopack::locator::LocationContext::FromCurrentExe,
    ) {
        Ok(locator) => Some(locator.get_packages_dir()),
        Err(_) => None,
    };
    let Some(packages_dir) = packages_dir else {
        return (None, None);
    };

    let source = match feed {
        Some(feed) if Path::new(feed).is_dir() => SignedFeed {
            store: FeedStore::Dir(PathBuf::from(feed)),
        },
        Some(feed) => SignedFeed {
            store: FeedStore::Http(FirstPartyFeed {
                base: feed.to_owned(),
            }),
        },
        None => SignedFeed {
            store: FeedStore::Http(FirstPartyFeed {
                base: FEED_URL.to_owned(),
            }),
        },
    };
    let manager = UpdateManager::new(source, Some(options), None).ok();
    if manager.is_some() {
        (manager, Some(packages_dir))
    } else {
        (None, None)
    }
}

impl Updater {
    pub fn new(feed: Option<&str>, notify: impl Fn(Value) + Send + Sync + 'static) -> Self {
        let (manager, packages_dir) = build_manager(feed);
        let phase = if manager.is_some() {
            Phase::Idle
        } else {
            Phase::Unavailable
        };
        Self {
            inner: Arc::new(Mutex::new(Inner {
                manager,
                packages_dir,
                phase,
                info: None,
                up_to_date: false,
                prefs: prefs::load(),
            })),
            notify: Arc::new(notify),
        }
    }

    #[cfg(test)]
    pub fn new_with_locator(
        feed: Option<&str>,
        locator: velopack::locator::VelopackLocatorConfig,
        notify: impl Fn(Value) + Send + Sync + 'static,
    ) -> Self {
        keep_staging_id_empty();
        let packages_dir = locator.PackagesDir.clone();
        let options = UpdateOptions {
            ExplicitChannel: Some(CHANNEL.to_owned()),
            ..Default::default()
        };
        let source = match feed {
            Some(feed) if Path::new(feed).is_dir() => SignedFeed {
                store: FeedStore::Dir(PathBuf::from(feed)),
            },
            Some(feed) => SignedFeed {
                store: FeedStore::Http(FirstPartyFeed {
                    base: feed.to_owned(),
                }),
            },
            None => SignedFeed {
                store: FeedStore::Http(FirstPartyFeed {
                    base: FEED_URL.to_owned(),
                }),
            },
        };
        let manager = UpdateManager::new(source, Some(options), Some(locator)).ok();
        let phase = if manager.is_some() {
            Phase::Idle
        } else {
            Phase::Unavailable
        };
        Self {
            inner: Arc::new(Mutex::new(Inner {
                manager,
                packages_dir: Some(packages_dir),
                phase,
                info: None,
                up_to_date: false,
                prefs: prefs::load(),
            })),
            notify: Arc::new(notify),
        }
    }

    pub fn view(&self) -> Value {
        let inner = self.inner.lock().expect("update state");
        let version = inner
            .info
            .as_ref()
            .map(|info| info.TargetFullRelease.Version.clone());
        let (phase, percent, message) = match &inner.phase {
            Phase::Unavailable => ("unavailable", None, None),
            Phase::Idle => ("idle", None, None),
            Phase::Checking => ("checking", None, None),
            Phase::Available => ("available", None, None),
            Phase::Downloading(percent) => ("downloading", Some(*percent), None),
            Phase::Ready => ("ready", None, None),
            Phase::Installing => ("installing", None, None),
            Phase::Failed(message) => ("failed", None, Some(message.clone())),
        };
        json!({
            "phase": phase,
            "version": version,
            "percent": percent,
            "message": message,
            "current": inner.manager.as_ref().map(UpdateManager::get_current_version_as_string),
            "up_to_date": inner.up_to_date,
            "last_checked_at": inner.prefs.last_checked_at,
            "auto_check": inner.prefs.auto_check,
            "auto_download": inner.prefs.auto_download,
            "interval": inner.prefs.interval,
        })
    }

    fn push(&self) {
        (self.notify)(self.view());
    }

    fn set_phase(&self, phase: Phase) {
        self.inner.lock().expect("update state").phase = phase;
        self.push();
    }

    fn busy(&self) -> bool {
        matches!(
            self.inner.lock().expect("update state").phase,
            Phase::Unavailable | Phase::Checking | Phase::Downloading(_) | Phase::Installing
        )
    }

    /// Check the feed. Runs on the caller's thread; call it off the window's.
    pub fn check(&self) {
        if self.busy() {
            return;
        }
        self.set_phase(Phase::Checking);
        let result = {
            let inner = self.inner.lock().expect("update state");
            inner.manager.as_ref().map(UpdateManager::check_for_updates)
        };
        let auto_download = {
            let mut inner = self.inner.lock().expect("update state");
            inner.prefs.last_checked_at = Some(now());
            prefs::save(&inner.prefs);
            match result {
                Some(Ok(UpdateCheck::UpdateAvailable(mut info))) => {
                    if !info.TargetFullRelease.Type.eq_ignore_ascii_case("Full") {
                        inner.info = None;
                        inner.up_to_date = false;
                        inner.phase = Phase::Failed("only full updates are supported".to_owned());
                    } else {
                        info.BaseRelease = None;
                        info.DeltasToTarget.clear();
                        inner.info = Some(*info);
                        inner.up_to_date = false;
                        inner.phase = Phase::Available;
                    }
                }
                Some(Ok(UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty)) => {
                    inner.info = None;
                    inner.up_to_date = true;
                    inner.phase = Phase::Idle;
                }
                Some(Err(error)) => inner.phase = Phase::Failed(error.to_string()),
                None => inner.phase = Phase::Unavailable,
            }
            matches!(inner.phase, Phase::Available) && inner.prefs.auto_download
        };
        self.push();
        if auto_download {
            self.download();
        }
    }

    /// Download the update the last check found, reporting progress.
    pub fn download(&self) {
        let (info, packages_dir) = {
            let inner = self.inner.lock().expect("update state");
            if !matches!(inner.phase, Phase::Available | Phase::Failed(_)) {
                return;
            }
            (inner.info.clone(), inner.packages_dir.clone())
        };
        let Some(info) = info else { return };
        let Some(packages_dir) = packages_dir else {
            self.set_phase(Phase::Unavailable);
            return;
        };
        self.set_phase(Phase::Downloading(0));
        let (sender, receiver) = channel::<i16>();
        let progress = self.clone();
        let reporter = std::thread::spawn(move || {
            for percent in receiver {
                progress.set_phase(Phase::Downloading(percent));
            }
        });
        let result: Result<(), velopack::Error> = {
            let inner = self.inner.lock().expect("update state");
            match preflight_target_package(&packages_dir, &info.TargetFullRelease) {
                Ok(()) => {
                    if let Some(manager) = inner.manager.as_ref() {
                        manager.download_updates(&info, Some(sender))
                    } else {
                        drop(sender);
                        Err(velopack::Error::Other("manager unavailable".into()))
                    }
                }
                Err(error) => {
                    drop(sender);
                    Err(error)
                }
            }
        };
        let _ = reporter.join();
        self.set_phase(match result {
            Ok(()) => Phase::Ready,
            Err(error) => Phase::Failed(error.to_string()),
        });
    }

    /// Hand off to the installer. On success this process exits here and the
    /// installer opens the app again once the update is in place.
    pub fn install(&self) {
        let (asset, packages_dir, ready) = {
            let inner = self.inner.lock().expect("update state");
            (
                inner
                    .info
                    .as_ref()
                    .map(|info| info.TargetFullRelease.clone()),
                inner.packages_dir.clone(),
                matches!(inner.phase, Phase::Ready),
            )
        };
        let Some((asset, packages_dir)) = asset.zip(packages_dir).filter(|_| ready) else {
            return;
        };
        self.set_phase(Phase::Installing);
        let result = {
            let inner = self.inner.lock().expect("update state");
            inner
                .manager
                .as_ref()
                .map(|manager| spawn_verified_helper(manager, &packages_dir, &asset))
        };
        match result {
            Some(Ok(())) => {
                std::process::exit(0);
            }
            Some(Err(error)) => {
                self.set_phase(Phase::Failed(error.to_string()));
            }
            None => {
                self.set_phase(Phase::Unavailable);
            }
        }
    }

    pub fn set_prefs(&self, auto_check: bool, auto_download: bool, interval: CheckInterval) {
        {
            let mut inner = self.inner.lock().expect("update state");
            inner.prefs.auto_check = auto_check;
            inner.prefs.auto_download = auto_download;
            inner.prefs.interval = interval;
            prefs::save(&inner.prefs);
        }
        self.push();
    }

    /// Whether a scheduled check is due now.
    pub fn check_due(&self) -> bool {
        let inner = self.inner.lock().expect("update state");
        inner.prefs.auto_check
            && matches!(inner.phase, Phase::Idle | Phase::Failed(_))
            && inner
                .prefs
                .last_checked_at
                .is_none_or(|at| now().saturating_sub(at) >= inner.prefs.interval.seconds())
    }

    /// Check on the owner's schedule for as long as the app runs.
    pub fn run_schedule(&self) {
        let updater = self.clone();
        std::thread::spawn(move || {
            // Let the window and the journal settle before the first check.
            std::thread::sleep(std::time::Duration::from_secs(30));
            loop {
                if updater.check_due() {
                    updater.check();
                }
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        });
    }
}

fn preflight_target_package(
    packages_dir: &Path,
    asset: &VelopackAsset,
) -> Result<(), velopack::Error> {
    if !asset.Type.eq_ignore_ascii_case("Full") {
        log::warn!("target release type {} is not Full", asset.Type);
        return Err(velopack::Error::Other(
            "target release type is not Full".into(),
        ));
    }
    if asset.Size == 0 {
        log::warn!("target release size is 0");
        return Err(velopack::Error::Other("target release size is zero".into()));
    }
    let sidecar_name = format!("{}.minisig", asset.FileName);
    if !asset_name_is_plain(&asset.FileName) || !asset_name_is_plain(&sidecar_name) {
        log::warn!(
            "target package or sidecar name is not plain: {}",
            asset.FileName
        );
        return Err(velopack::Error::Other(
            "target package or sidecar name is not plain".into(),
        ));
    }
    let target_nupkg = packages_dir.join(&asset.FileName);
    let target_sidecar = packages_dir.join(&sidecar_name);
    if target_nupkg.parent() != Some(packages_dir) || target_sidecar.parent() != Some(packages_dir)
    {
        log::warn!("target package or sidecar escaped packages directory");
        return Err(velopack::Error::Other(
            "target package or sidecar escaped packages directory".into(),
        ));
    }

    match std::fs::symlink_metadata(&target_nupkg) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            remove_sidecar_if_present(&target_sidecar)?;
            Ok(())
        }
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::remove_file(&target_nupkg).map_err(|e| {
                log::warn!("could not remove symlink at target package path: {e}");
                velopack::Error::Other(format!(
                    "could not remove symlink at target package path: {e}"
                ))
            })?;
            remove_sidecar_if_present(&target_sidecar)?;
            Ok(())
        }
        Ok(meta) if meta.file_type().is_file() => {
            match package_admission::admit_existing_package(&target_nupkg, &target_sidecar, asset) {
                Ok(_) => Ok(()),
                Err(_) => {
                    std::fs::remove_file(&target_nupkg).map_err(|e| {
                        log::warn!("could not remove invalid cached package: {e}");
                        velopack::Error::Other(format!(
                            "could not remove invalid cached package: {e}"
                        ))
                    })?;
                    remove_sidecar_if_present(&target_sidecar)?;
                    Ok(())
                }
            }
        }
        Ok(_) => {
            log::warn!(
                "target package path {} is a directory or non-file",
                target_nupkg.display()
            );
            Err(velopack::Error::Other(format!(
                "target package path {} is a directory or non-file",
                target_nupkg.display()
            )))
        }
        Err(e) => {
            log::warn!(
                "could not inspect package path {}: {e}",
                target_nupkg.display()
            );
            Err(velopack::Error::Other(format!(
                "could not inspect package path: {e}"
            )))
        }
    }
}

fn remove_sidecar_if_present(sidecar_path: &Path) -> Result<(), velopack::Error> {
    match std::fs::symlink_metadata(sidecar_path) {
        Ok(meta) => {
            if meta.file_type().is_file() || meta.file_type().is_symlink() {
                std::fs::remove_file(sidecar_path).map_err(|e| {
                    log::warn!("could not remove sidecar {}: {e}", sidecar_path.display());
                    velopack::Error::Other(format!(
                        "could not remove sidecar {}: {e}",
                        sidecar_path.display()
                    ))
                })
            } else {
                log::warn!("sidecar path {} is a directory", sidecar_path.display());
                Err(velopack::Error::Other(format!(
                    "sidecar path {} is a directory",
                    sidecar_path.display()
                )))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            log::warn!(
                "could not query sidecar metadata {}: {e}",
                sidecar_path.display()
            );
            Err(velopack::Error::Other(format!(
                "could not query sidecar metadata: {e}"
            )))
        }
    }
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
        use std::sync::OnceLock;
        use std::time::{Duration, Instant};

        use velopack::VelopackAsset;
        use velopack::sources::UpdateSource;
        use zip::ZipWriter;
        use zip::write::SimpleFileOptions;

        use super::super::{
            FeedStore, FirstPartyFeed, Phase, SignedFeed, Updater, spawn_verified_helper,
        };

        struct TestKeys {
            primary_pub: PathBuf,
            primary_sec: PathBuf,
            wrong_sec: PathBuf,
        }

        static TEST_KEYS: OnceLock<TestKeys> = OnceLock::new();

        fn get_test_keys() -> &'static TestKeys {
            TEST_KEYS.get_or_init(|| {
                let temp = tempfile::Builder::new()
                    .prefix("keys")
                    .tempdir()
                    .unwrap()
                    .keep();
                let primary_pub = temp.join("primary.pub");
                let primary_sec = temp.join("primary.key");
                let status = std::process::Command::new("minisign")
                    .args([
                        "-G",
                        "-W",
                        "-p",
                        primary_pub.to_str().unwrap(),
                        "-s",
                        primary_sec.to_str().unwrap(),
                    ])
                    .status()
                    .expect("minisign generate primary key");
                assert!(status.success());

                solstone_core_installed_payload::install_test_fixture_pin(&primary_pub)
                    .expect("install test fixture pin");

                let wrong_pub = temp.join("wrong.pub");
                let wrong_sec = temp.join("wrong.key");
                let status2 = std::process::Command::new("minisign")
                    .args([
                        "-G",
                        "-W",
                        "-p",
                        wrong_pub.to_str().unwrap(),
                        "-s",
                        wrong_sec.to_str().unwrap(),
                    ])
                    .status()
                    .expect("minisign generate wrong key");
                assert!(status2.success());

                TestKeys {
                    primary_pub,
                    primary_sec,
                    wrong_sec,
                }
            })
        }

        fn sign_with_key(key_path: &Path, data: &[u8]) -> Vec<u8> {
            let temp = tempfile::Builder::new().prefix("msign").tempdir().unwrap();
            let data_file = temp.path().join("data.bin");
            let sig_file = temp.path().join("data.bin.minisig");
            std::fs::write(&data_file, data).unwrap();
            let status = std::process::Command::new("minisign")
                .args([
                    "-S",
                    "-W",
                    "-s",
                    key_path.to_str().unwrap(),
                    "-m",
                    data_file.to_str().unwrap(),
                    "-x",
                    sig_file.to_str().unwrap(),
                ])
                .status()
                .expect("minisign sign");
            assert!(status.success());
            std::fs::read(sig_file).unwrap()
        }

        fn make_valid_nupkg(id: &str, version: &str, main_exe: &str) -> Vec<u8> {
            let nuspec = format!(
                r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://schemas.microsoft.com/packaging/2010/07/nuspec.xsd">
<metadata>
  <id>{id}</id>
  <version>{version}</version>
  <channel>win</channel>
  <os>win</os>
  <rid>win-x64</rid>
  <machineArchitecture>x64</machineArchitecture>
  <mainExe>{main_exe}</mainExe>
</metadata>
</package>"#
            );
            let mut buf = Vec::new();
            let mut zip = ZipWriter::new(Cursor::new(&mut buf));
            zip.start_file(format!("{id}.nuspec"), SimpleFileOptions::default())
                .unwrap();
            zip.write_all(nuspec.as_bytes()).unwrap();
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
    std::fs::write(&argv_file, args.join("\n")).unwrap();
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
  <mainExe>journal-app.exe</mainExe>
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
        fn a_sibling_package_streams_to_disk_and_reports_progress() {
            let keys = get_test_keys();
            let pkg_bytes = make_valid_nupkg("SolstoneJournal", "2.0.28", "journal-app.exe");
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
            let pkg_bytes = make_valid_nupkg("SolstoneJournal", "2.0.28", "journal-app.exe");
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
            let other_id_pkg = make_valid_nupkg("Other", "2.0.28", "journal-app.exe");
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
            let other_ver_pkg = make_valid_nupkg("SolstoneJournal", "2.0.29", "journal-app.exe");
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
            let nuspec1 = r#"<?xml version="1.0"?><package><metadata><id>SolstoneJournal</id><version>2.0.28</version><channel>win</channel><os>win</os><rid>win-x64</rid><machineArchitecture>x64</machineArchitecture><mainExe>journal-app.exe</mainExe></metadata></package>"#;
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

        #[test]
        fn directory_feed_signed_admits_unsigned_refuses() {
            let keys = get_test_keys();
            let pkg_bytes = make_valid_nupkg("SolstoneJournal", "2.0.28", "journal-app.exe");
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
            let good_pkg = make_valid_nupkg("SolstoneJournal", "2.0.28", "journal-app.exe");

            let temp = tempfile::Builder::new().prefix("cached").tempdir().unwrap();
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

            // Feed directory serving good pkg but bad sidecar, causing download_updates to return Err
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
        }

        #[test]
        fn apply_failure_and_spawn_verified_helper_argv_inspection() {
            let keys = get_test_keys();
            let good_pkg = make_valid_nupkg("SolstoneJournal", "2.0.28", "journal-app.exe");
            let good_sig = sign_with_key(&keys.primary_sec, &good_pkg);

            let temp = tempfile::Builder::new().prefix("apply").tempdir().unwrap();
            let locator = create_test_locator(temp.path());

            let feed_dir = temp.path().join("feed");
            std::fs::create_dir_all(&feed_dir).unwrap();
            let pkg_name = "SolstoneJournal-2.0.28-full.nupkg";
            std::fs::write(feed_dir.join(pkg_name), &good_pkg).unwrap();
            std::fs::write(feed_dir.join(format!("{pkg_name}.minisig")), &good_sig).unwrap();

            // 1. Test apply failure: corrupted package causes install() to fail without exiting
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

            {
                let mut inner = updater.inner.lock().unwrap();
                inner.info = Some(velopack::UpdateInfo {
                    TargetFullRelease: asset.clone(),
                    BaseRelease: None,
                    DeltasToTarget: Vec::new(),
                    IsDowngrade: false,
                });
                inner.phase = Phase::Ready;
            }

            // Write corrupted files in packages dir
            std::fs::write(locator.PackagesDir.join(pkg_name), b"corrupted bytes").unwrap();
            std::fs::write(
                locator.PackagesDir.join(format!("{pkg_name}.minisig")),
                &good_sig,
            )
            .unwrap();

            // Call install(): should fail validation, set Phase::Failed, and NOT exit
            updater.install();
            let view = updater.view();
            assert_eq!(view["phase"], "failed");

            // Recorder argv file must NOT exist
            let argv_file = locator.UpdateExePath.with_extension("argv");
            assert!(!argv_file.exists());

            // 2. Recreate updater, check + download valid files
            std::fs::remove_file(locator.PackagesDir.join(pkg_name)).unwrap();
            std::fs::remove_file(locator.PackagesDir.join(format!("{pkg_name}.minisig"))).unwrap();

            // Write feed release json
            let release_feed = serde_json::json!({
                "Assets": [asset]
            });
            std::fs::write(
                feed_dir.join("releases.win.json"),
                serde_json::to_string(&release_feed).unwrap(),
            )
            .unwrap();

            let updater2 = Updater::new_with_locator(
                Some(feed_dir.to_str().unwrap()),
                locator.clone(),
                |_| {},
            );

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
            let pkg_flag_idx = args
                .iter()
                .position(|&a| a == "--package")
                .expect("--package flag");
            let target_arg = args[pkg_flag_idx + 1];
            assert!(target_arg.ends_with("SolstoneJournal-2.0.28-full.nupkg"));
            assert!(!target_arg.contains("SolstoneJournal-3.0.0-full.nupkg"));

            let after_update_idx = args
                .iter()
                .position(|&a| a == "--after-update")
                .expect("--after-update flag");
            assert!(after_update_idx > pkg_flag_idx);
        }

        #[test]
        fn delta_rejections_and_plan_clearing() {
            let keys = get_test_keys();
            let pkg_bytes = make_valid_nupkg("SolstoneJournal", "2.0.28", "journal-app.exe");
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
                feed_dir.join("releases.win.json"),
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
