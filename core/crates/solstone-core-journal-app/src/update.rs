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
use velopack::sources::{FileSource, UpdateSource};
use velopack::{
    Error, UpdateCheck, UpdateInfo, UpdateManager, UpdateOptions, VelopackAsset, VelopackAssetFeed,
    download,
};

use crate::prefs::{self, CheckInterval, Prefs};

/// The journal's Windows release feed on sol pbc's own update origin.
pub const FEED_URL: &str = "https://updates.solstone.app/solstone-journal/release/windows";
const CHANNEL: &str = "win";

/// The first-party feed: one GET for the release list, then the package file
/// by name from the same place. Nothing else is sent.
#[derive(Clone)]
struct FirstPartyFeed {
    base: String,
}

impl FirstPartyFeed {
    fn asset_url(&self, file_name: &str) -> Result<String, Error> {
        if !asset_name_is_plain(file_name) {
            return Err(Error::Other(format!(
                "the update feed names a file outside itself: {file_name}"
            )));
        }
        Ok(format!("{}/{file_name}", self.base.trim_end_matches('/')))
    }
}

/// A package file the feed names must sit beside the feed itself.
fn asset_name_is_plain(file_name: &str) -> bool {
    !file_name.is_empty()
        && !file_name.starts_with('.')
        && !file_name.contains(['/', '\\', '?', '#', ':', '%'])
}

impl UpdateSource for FirstPartyFeed {
    fn get_release_feed(
        &self,
        channel: &str,
        _app: &Manifest,
        _staged_user_id: &str,
    ) -> Result<VelopackAssetFeed, Error> {
        let url = format!(
            "{}/releases.{channel}.json",
            self.base.trim_end_matches('/')
        );
        let json = download::download_url_as_string(&url)?;
        serde_json::from_str(&json).map_err(Error::from)
    }

    fn download_release_entry(
        &self,
        asset: &VelopackAsset,
        local_file: &Path,
        progress_sender: Option<Sender<i16>>,
    ) -> Result<(), Error> {
        let url = self.asset_url(&asset.FileName)?;
        download::download_url_to_file(&url, local_file, move |percent| {
            if let Some(sender) = &progress_sender {
                let _ = sender.send(percent);
            }
        })
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

fn build_manager(feed: Option<&str>) -> Option<UpdateManager> {
    keep_staging_id_empty();
    let options = UpdateOptions {
        ExplicitChannel: Some(CHANNEL.to_owned()),
        ..Default::default()
    };
    let result = match feed {
        // A folder holding a staged release list and its packages: how an
        // update is proven without touching the feed owners update from.
        Some(feed) if Path::new(feed).is_dir() => {
            UpdateManager::new(FileSource::new(feed), Some(options), None)
        }
        Some(feed) => UpdateManager::new(
            FirstPartyFeed {
                base: feed.to_owned(),
            },
            Some(options),
            None,
        ),
        None => UpdateManager::new(
            FirstPartyFeed {
                base: FEED_URL.to_owned(),
            },
            Some(options),
            None,
        ),
    };
    result.ok()
}

impl Updater {
    pub fn new(feed: Option<&str>, notify: impl Fn(Value) + Send + Sync + 'static) -> Self {
        let manager = build_manager(feed);
        let phase = if manager.is_some() {
            Phase::Idle
        } else {
            Phase::Unavailable
        };
        Self {
            inner: Arc::new(Mutex::new(Inner {
                manager,
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
                Some(Ok(UpdateCheck::UpdateAvailable(info))) => {
                    inner.info = Some(*info);
                    inner.up_to_date = false;
                    inner.phase = Phase::Available;
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
        let info = {
            let inner = self.inner.lock().expect("update state");
            if !matches!(inner.phase, Phase::Available | Phase::Failed(_)) {
                return;
            }
            inner.info.clone()
        };
        let Some(info) = info else { return };
        self.set_phase(Phase::Downloading(0));
        let (sender, receiver) = channel::<i16>();
        let progress = self.clone();
        let reporter = std::thread::spawn(move || {
            for percent in receiver {
                progress.set_phase(Phase::Downloading(percent));
            }
        });
        let result = {
            let inner = self.inner.lock().expect("update state");
            inner
                .manager
                .as_ref()
                .map(|manager| manager.download_updates(&info, Some(sender)))
        };
        let _ = reporter.join();
        self.set_phase(match result {
            Some(Ok(())) => Phase::Ready,
            Some(Err(error)) => Phase::Failed(error.to_string()),
            None => Phase::Unavailable,
        });
    }

    /// Hand off to the installer. On success this process exits here and the
    /// installer opens the app again once the update is in place.
    pub fn install(&self) {
        let (asset, ready) = {
            let inner = self.inner.lock().expect("update state");
            (
                inner
                    .info
                    .as_ref()
                    .map(|info| info.TargetFullRelease.clone()),
                matches!(inner.phase, Phase::Ready),
            )
        };
        let Some(asset) = asset.filter(|_| ready) else {
            return;
        };
        self.set_phase(Phase::Installing);
        let result = {
            let inner = self.inner.lock().expect("update state");
            inner.manager.as_ref().map(|manager| {
                manager.apply_updates_and_restart_with_args(&asset, ["--after-update"])
            })
        };
        if let Some(Err(error)) = result {
            self.set_phase(Phase::Failed(error.to_string()));
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
}
