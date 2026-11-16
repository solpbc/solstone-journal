// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Automatic updates for a Linux tree install, the `curl … | sh` route.
//!
//! A systemd user timer runs `solstone update --unattended` once a day. That
//! run reads the signed catalogue, fetches the installer copy published beside
//! it, checks the copy against the digest the signed catalogue names, and runs
//! it pinned to that catalogue. It never passes `--yes` and never asks
//! anything. It never starts, stops or restarts the journal: the new version
//! is installed beside the running one and starts the next time the journal
//! does. Two failed runs in a row pause it until the owner tries again.
//!
//! Every request is a bare GET to the update origin. The schedule's random
//! delay is drawn again each day. What this keeps is the owner's own, on this
//! computer: the choice, and the last run's result.

use std::cmp::Ordering;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::catalogue::{CatalogueListing, InstallerCopy, PlatformCatalogue};
use crate::delegate::{InstallerRunner, StdinMode, validate_safe_arg};
use crate::fetch::{CopyFetchError, download_installer_copy, update_origin};
use crate::hold::{HoldState, read_hold_state};
use crate::ownership::{
    OwnershipResult, PackageProbe, PositiveOwnership, RouteKind, ServicePolicy, classify_ownership,
};
use crate::{compare_platform_versions, compare_semver_3part};

/// The owner's choice: `on` or `off`. Missing means never asked.
pub const CHOICE_FILENAME: &str = "automatic-update";
/// The last run's result, kept for the journal to show.
pub const STATE_FILENAME: &str = "automatic-update-state.json";
pub const TIMER_UNIT: &str = "solstone-update.timer";
pub const SERVICE_UNIT: &str = "solstone-update.service";
/// Set for the installer an unattended run starts, and read by the journal's
/// setup: an automatic update never starts or restarts the journal.
pub const UNATTENDED_ENV: &str = "SOLSTONE_UPDATE_UNATTENDED";
/// Failed runs in a row before automatic updates pause.
pub const PAUSE_AFTER_FAILURES: u32 = 2;

const TIMER: &str = "[Unit]\n\
Description=solstone daily update check\n\
\n\
[Timer]\n\
OnCalendar=daily\n\
RandomizedDelaySec=4h\n\
Persistent=true\n\
\n\
[Install]\n\
WantedBy=timers.target\n";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    On,
    Off,
    Unasked,
}

/// What the last unattended run found, kept between runs.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct State {
    #[serde(default)]
    pub last_check_unix: Option<u64>,
    #[serde(default)]
    pub last_result: Option<String>,
    /// The catalogue coordinate the last successful run installed.
    #[serde(default)]
    pub last_installed: Option<String>,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub paused: bool,
    /// The newest catalogue coordinate any run has accepted. A catalogue below
    /// it is refused: the release rail never moves a coordinate back.
    #[serde(default)]
    pub highest_platform_version: Option<String>,
}

/// What the journal shows about automatic updates.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AutomaticStatus {
    pub choice: Choice,
    /// Whether this install can update itself (a tree install with a journal).
    pub eligible: bool,
    /// Whether the solstone app updates with it.
    pub with_app: bool,
    pub paused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_check_unix: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_installed: Option<String>,
}

/// One `systemctl --user` call. `Ok(false)` is a nonzero exit.
pub trait Systemctl {
    fn user(&self, args: &[&str]) -> io::Result<bool>;
}

pub struct RealSystemctl;

impl Systemctl for RealSystemctl {
    fn user(&self, args: &[&str]) -> io::Result<bool> {
        std::process::Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
    }
}

/// Whether this computer's network connection is metered. `None` when
/// nothing says, which is not metered.
pub trait Meter {
    fn metered(&self) -> Option<bool>;
}

/// NetworkManager's own answer, where it runs.
pub struct NetworkManagerMeter;

impl Meter for NetworkManagerMeter {
    fn metered(&self) -> Option<bool> {
        let output = std::process::Command::new("busctl")
            .args([
                "--system",
                "get-property",
                "org.freedesktop.NetworkManager",
                "/org/freedesktop/NetworkManager",
                "org.freedesktop.NetworkManager",
                "Metered",
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        parse_metered(&String::from_utf8_lossy(&output.stdout))
    }
}

/// NetworkManager's `Metered` is `u 1` (yes) or `u 3` (guess yes) when the
/// connection is metered.
fn parse_metered(text: &str) -> Option<bool> {
    match text.trim().strip_prefix("u ")?.trim() {
        "1" | "3" => Some(true),
        "0" | "2" | "4" => Some(false),
        _ => None,
    }
}

/// The installed journal's security floor on Linux, from the signed advisory
/// this computer last verified, if any.
pub trait SecurityFloor {
    fn linux_floor(&self) -> Option<String>;
}

/// Reads the advisory the journal keeps in its own folder.
pub struct StoredAdvisoryFloor {
    pub journal: PathBuf,
    pub now_unix: u64,
}

impl SecurityFloor for StoredAdvisoryFloor {
    fn linux_floor(&self) -> Option<String> {
        #[derive(Deserialize)]
        struct Stored {
            document: String,
            signature: String,
        }
        let bytes = fs::read(self.journal.join("health/security-advisory.json")).ok()?;
        let stored: Stored = serde_json::from_slice(&bytes).ok()?;
        let key = crate::advisory::platform_key().ok()?;
        let advisory = crate::advisory::verify(
            stored.document.as_bytes(),
            stored.signature.as_bytes(),
            &key,
            self.now_unix,
            None,
        )
        .ok()?;
        advisory
            .journal
            .get("linux")
            .map(|floor| floor.fixed_in.clone())
    }
}

/// Fetches the installer copy a verified catalogue names.
pub trait InstallerCopyFetch {
    fn fetch_copy(
        &self,
        lane: &str,
        coordinate: &str,
        copy: &InstallerCopy,
    ) -> Result<PathBuf, CopyFetchError>;
    fn cleanup(&self, path: &Path) {
        let _ = fs::remove_file(path);
    }
}

pub struct RealInstallerCopyFetch;

impl InstallerCopyFetch for RealInstallerCopyFetch {
    fn fetch_copy(
        &self,
        lane: &str,
        coordinate: &str,
        copy: &InstallerCopy,
    ) -> Result<PathBuf, CopyFetchError> {
        let (origin, host) = update_origin();
        download_installer_copy(
            &format!("{origin}/solstone/{lane}/{coordinate}/{}", copy.filename),
            host,
            &copy.sha256,
        )
    }
}

/// The journal's tree prefix, from the tree receipt, when every recorded
/// component is on the tree route and one is the journal. This is what the
/// journal's own switch reads, including while a newer journal waits for its
/// next start; each unattended run still checks ownership in full first.
pub fn receipt_tree_prefix(data_dir: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(data_dir.join("install.conf")).ok()?;
    let mut section = String::new();
    let (mut platform_prefix, mut journal_prefix) = (None, None);
    let mut has_journal = false;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.to_owned();
            has_journal |= section == "component:journal" || section == "component:cli";
        } else if let Some(value) = line.strip_prefix("route=") {
            if value != "tree" {
                return None;
            }
        } else if let Some(value) = line.strip_prefix("prefix=") {
            match section.as_str() {
                "solstone" => platform_prefix = Some(value.to_owned()),
                "component:journal" | "component:cli" => journal_prefix = Some(value.to_owned()),
                _ => {}
            }
        }
    }
    let prefix = PathBuf::from(journal_prefix.or(platform_prefix)?);
    (has_journal && prefix.is_absolute()).then_some(prefix)
}

/// Queue a restart of the journal's service, for the owner's press. The caller
/// runs inside that service, and the restart stops its whole control group, so
/// `systemctl` can be ended by that stop before it exits: ended that way, the
/// restart has started.
pub fn restart_journal() -> bool {
    use std::os::unix::process::ExitStatusExt;
    std::process::Command::new("systemctl")
        .args(["--user", "--no-block", "restart", "solstone.service"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success() || status.signal() == Some(15))
}

pub fn read_choice(data_dir: &Path) -> Choice {
    match fs::read_to_string(data_dir.join(CHOICE_FILENAME)) {
        Ok(text) if text.trim() == "on" => Choice::On,
        Ok(text) if text.trim() == "off" => Choice::Off,
        // A choice that can't be read is not a yes.
        Ok(_) => Choice::Off,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Choice::Unasked,
        Err(_) => Choice::Off,
    }
}

fn write_choice(data_dir: &Path, choice: Choice) -> io::Result<()> {
    let text = match choice {
        Choice::On => "on\n",
        Choice::Off => "off\n",
        Choice::Unasked => return remove_if_present(&data_dir.join(CHOICE_FILENAME)),
    };
    write_private(&data_dir.join(CHOICE_FILENAME), text.as_bytes())
}

pub fn read_state(data_dir: &Path) -> State {
    fs::read(data_dir.join(STATE_FILENAME))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn write_state(data_dir: &Path, state: &State) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
    write_private(&data_dir.join(STATE_FILENAME), &bytes)
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let next = dir.join(format!(
        ".{}.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("next"),
        std::process::id()
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&next)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&next, path)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

pub fn unit_dir(home: &Path) -> PathBuf {
    home.join(".config/systemd/user")
}

/// The service runs the newest installed journal's updater, so a fix to the
/// updater itself takes effect on the next run.
fn service_unit(solstone: &Path) -> Option<String> {
    let path = solstone.to_str()?;
    if path.contains(['"', '\\', '\n', '\r']) {
        return None;
    }
    let escaped = path.replace('%', "%%").replace('$', "$$");
    Some(format!(
        "[Unit]\n\
         Description=solstone automatic update\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart=\"{escaped}\" update --unattended\n\
         Nice=10\n\
         IOSchedulingClass=idle\n\
         TimeoutStartSec=2h\n"
    ))
}

/// Whether `pos` is an install that can update itself: a tree install by the
/// platform installer, carrying a journal, with every component on the tree.
pub fn eligible(pos: &PositiveOwnership) -> bool {
    pos.route_kind == RouteKind::PlatformTree
        && pos.prefix.is_some()
        && pos
            .components
            .iter()
            .any(|c| c.name == "journal" || c.name == "cli")
        && pos.components.iter().all(|c| c.route == "tree")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SwitchRefusal {
    pub code: &'static str,
    pub message: String,
}

impl SwitchRefusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// The words for an install that can't update itself, which also say how a
/// package install gets updates.
pub fn ineligible_message(ownership: &OwnershipResult, has_repository: bool) -> String {
    let package = matches!(
        ownership,
        OwnershipResult::Positive(pos)
            if matches!(pos.route_kind, RouteKind::PlatformPackage | RouteKind::PlatformMixed)
    );
    if package {
        package_route_message(has_repository).to_owned()
    } else {
        "this install can't update itself. automatic updates work on an install from https://solstone.app/install.sh.".to_owned()
    }
}

/// What a package install tells its owner about updates.
pub fn package_route_message(has_repository: bool) -> &'static str {
    if has_repository {
        "installed from a package. updates come through your system's updater."
    } else {
        "installed from a package. to get updates through your system's updater, add the solstone repository: solstone.app/install#linux-packages"
    }
}

/// Turn automatic updates on: write the two units, start the timer, and keep
/// the choice.
pub fn turn_on(
    pos: &PositiveOwnership,
    data_dir: &Path,
    home: &Path,
    systemctl: &dyn Systemctl,
) -> Result<(), SwitchRefusal> {
    if !eligible(pos) {
        return Err(SwitchRefusal::new(
            "not-eligible",
            "this install can't update itself.",
        ));
    }
    let prefix = pos.prefix.as_deref().expect("eligible tree has a prefix");
    turn_on_at(prefix, data_dir, home, systemctl)
}

/// Turn automatic updates on for the tree at `prefix`.
pub fn turn_on_at(
    prefix: &Path,
    data_dir: &Path,
    home: &Path,
    systemctl: &dyn Systemctl,
) -> Result<(), SwitchRefusal> {
    let service = service_unit(&prefix.join("current/bin/solstone")).ok_or_else(|| {
        SwitchRefusal::new(
            "prefix-unsupported",
            "the install directory's name can't go in a service file.",
        )
    })?;
    let dir = unit_dir(home);
    let wrote = fs::create_dir_all(&dir)
        .and_then(|()| fs::write(dir.join(SERVICE_UNIT), service))
        .and_then(|()| fs::write(dir.join(TIMER_UNIT), TIMER));
    let started = wrote.is_ok()
        && systemctl.user(&["daemon-reload"]).unwrap_or(false)
        && systemctl
            .user(&["enable", "--now", TIMER_UNIT])
            .unwrap_or(false);
    if !started {
        remove_units(home, systemctl);
        return Err(SwitchRefusal::new(
            "systemd-unavailable",
            "couldn't set up the daily update check. your systemd user session may not be running.",
        ));
    }
    write_choice(data_dir, Choice::On)
        .map_err(|_| SwitchRefusal::new("write-failed", "couldn't save your choice."))?;
    // Turning it on is the owner trying again.
    let mut state = read_state(data_dir);
    state.paused = false;
    state.consecutive_failures = 0;
    let _ = write_state(data_dir, &state);
    Ok(())
}

/// Turn automatic updates off. This works on any install, so the off command
/// is always one that works.
pub fn turn_off(
    data_dir: &Path,
    home: &Path,
    systemctl: &dyn Systemctl,
) -> Result<(), SwitchRefusal> {
    remove_units(home, systemctl);
    write_choice(data_dir, Choice::Off)
        .map_err(|_| SwitchRefusal::new("write-failed", "couldn't save your choice."))
}

/// Remove the units, the choice and the state, for a clean uninstall.
pub fn remove_all(data_dir: &Path, home: &Path, systemctl: &dyn Systemctl) -> io::Result<()> {
    remove_units(home, systemctl);
    remove_if_present(&data_dir.join(CHOICE_FILENAME))?;
    remove_if_present(&data_dir.join(STATE_FILENAME))
}

fn remove_units(home: &Path, systemctl: &dyn Systemctl) {
    let dir = unit_dir(home);
    if dir.join(TIMER_UNIT).exists() {
        let _ = systemctl.user(&["disable", "--now", TIMER_UNIT]);
    }
    let removed = [TIMER_UNIT, SERVICE_UNIT]
        .iter()
        .filter(|unit| fs::remove_file(dir.join(unit)).is_ok())
        .count();
    if removed > 0 {
        let _ = systemctl.user(&["daemon-reload"]);
    }
}

/// Clear a pause and run the update now, for the pane's "try again".
pub fn try_again(data_dir: &Path, systemctl: &dyn Systemctl) -> Result<(), SwitchRefusal> {
    let mut state = read_state(data_dir);
    state.paused = false;
    state.consecutive_failures = 0;
    write_state(data_dir, &state)
        .map_err(|_| SwitchRefusal::new("write-failed", "couldn't save the update state."))?;
    if read_choice(data_dir) == Choice::On
        && systemctl
            .user(&["start", "--no-block", SERVICE_UNIT])
            .unwrap_or(false)
    {
        Ok(())
    } else {
        Err(SwitchRefusal::new(
            "systemd-unavailable",
            "couldn't start the update. your systemd user session may not be running.",
        ))
    }
}

/// An update the owner ran themselves succeeded: it ends any pause, and its
/// catalogue counts as accepted.
pub fn record_owner_update(data_dir: &Path, platform_version: Option<&str>) {
    let mut state = read_state(data_dir);
    if state.consecutive_failures == 0 && !state.paused && platform_version.is_none() {
        return;
    }
    state.consecutive_failures = 0;
    state.paused = false;
    if let Some(version) = platform_version {
        raise_highest(&mut state, version);
    }
    let _ = write_state(data_dir, &state);
}

pub fn status(pos: Option<&PositiveOwnership>, data_dir: &Path) -> AutomaticStatus {
    let state = read_state(data_dir);
    AutomaticStatus {
        choice: read_choice(data_dir),
        eligible: pos.is_some_and(eligible),
        with_app: pos.is_some_and(|pos| pos.components.iter().any(|c| c.name == "desktop")),
        paused: state.paused,
        last_check_unix: state.last_check_unix,
        last_result: state.last_result,
        last_installed: state.last_installed,
    }
}

/// What an unattended run did. `failure` counts toward a pause.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RunOutcome {
    pub result: String,
    pub failure: bool,
    pub paused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed: Option<String>,
}

pub struct UnattendedRun<'a> {
    pub catalogue: &'a dyn PlatformCatalogue,
    pub copy_fetch: &'a dyn InstallerCopyFetch,
    pub runner: &'a dyn InstallerRunner,
    pub probe: &'a dyn PackageProbe,
    pub meter: &'a dyn Meter,
    pub floor: &'a dyn SecurityFloor,
    pub executable: &'a Path,
    pub data_dir: &'a Path,
    pub etc_dir: &'a Path,
    pub now_unix: u64,
}

impl UnattendedRun<'_> {
    pub fn run(&self) -> RunOutcome {
        if read_choice(self.data_dir) != Choice::On {
            return quiet("off");
        }
        let mut state = read_state(self.data_dir);
        if state.paused {
            return RunOutcome {
                result: "paused".to_owned(),
                failure: false,
                paused: true,
                installed: None,
            };
        }
        let step = self.step(&mut state);
        state.last_check_unix = Some(self.now_unix);
        state.last_result = Some(step.result.clone());
        if step.failure {
            state.consecutive_failures = state.consecutive_failures.saturating_add(1);
            state.paused = state.consecutive_failures >= PAUSE_AFTER_FAILURES;
        } else if step.counts_as_success {
            state.consecutive_failures = 0;
        }
        if let Some(ref installed) = step.installed {
            state.last_installed = Some(installed.clone());
        }
        let _ = write_state(self.data_dir, &state);
        RunOutcome {
            result: step.result,
            failure: step.failure,
            paused: state.paused,
            installed: step.installed,
        }
    }

    fn step(&self, state: &mut State) -> Step {
        match read_hold_state(self.data_dir) {
            HoldState::Unheld => {}
            HoldState::Held => return Step::skipped("held"),
            HoldState::Unknown => return Step::skipped("hold-unknown"),
        }
        let pos = match classify_ownership(self.executable, self.data_dir, self.etc_dir, self.probe)
        {
            OwnershipResult::Positive(pos) if eligible(&pos) => pos,
            _ => return Step::skipped("not-eligible"),
        };
        if self.meter.metered() == Some(true) {
            return Step::skipped("metered");
        }
        let listing = match self.catalogue.listing(&pos.lane, None) {
            Ok(listing) => listing,
            Err(refusal) if refusal.code == "fetch-failed" => return Step::skipped("offline"),
            Err(refusal) => return Step::failed(refusal.code),
        };
        // The release rail never moves a coordinate back, so an older one is
        // someone serving a stale catalogue to hold this computer back.
        if let Some(ref highest) = state.highest_platform_version
            && matches!(
                compare_platform_versions(&listing.platform_version, highest),
                Some(Ordering::Less) | None
            )
        {
            return Step::failed("catalogue-went-backwards");
        }
        raise_highest(state, &listing.platform_version);
        if let Some(floor) = self.floor.linux_floor()
            && crate::advisory::is_below(&pos.current_journal_version, &floor)
            && crate::advisory::is_below(&listing.journal_version, &floor)
        {
            return Step::failed("security-update-unavailable");
        }
        if !platform_update_available(&pos, &listing) {
            return Step::succeeded("up-to-date", None);
        }
        let Some(ref copy) = listing.installer else {
            return Step::skipped("waiting-for-signed-installer");
        };
        let script = match self
            .copy_fetch
            .fetch_copy(&pos.lane, &listing.platform_version, copy)
        {
            Ok(path) => path,
            Err(CopyFetchError::Offline) => return Step::skipped("offline"),
            Err(CopyFetchError::Unavailable) => return Step::failed("installer-unavailable"),
            Err(CopyFetchError::DigestMismatch) => {
                return Step::failed("installer-digest-mismatch");
            }
        };
        let result = self.install(&pos, &listing, &script);
        self.copy_fetch.cleanup(&script);
        result
    }

    fn install(&self, pos: &PositiveOwnership, listing: &CatalogueListing, script: &Path) -> Step {
        let Ok(invocations) = unattended_invocations(script, pos, &listing.platform_version) else {
            return Step::failed("argv-failed");
        };
        for args in invocations {
            let out = match self
                .runner
                .run(Path::new("/bin/sh"), &args, StdinMode::Closed)
            {
                Ok(out) => out,
                Err(_) => return Step::failed("runner-failed"),
            };
            if !out.success {
                let value = serde_json::from_slice::<serde_json::Value>(&out.stdout)
                    .unwrap_or(serde_json::Value::Null);
                let code = value["root_code"].as_str().unwrap_or("installer-failed");
                // An update the owner started holds the installer's lock.
                if code == "route-busy" {
                    return Step::skipped("busy");
                }
                return Step::failed(code);
            }
        }
        Step::succeeded("installed", Some(listing.platform_version.clone()))
    }
}

/// Whether the catalogue offers something newer than this platform install.
pub fn platform_update_available(pos: &PositiveOwnership, listing: &CatalogueListing) -> bool {
    // A run that stopped part-way can record the newer catalogue for its
    // header while a component stayed on its older version, so each recorded
    // component is compared too, and a failed component is tried again.
    let platform_newer = pos
        .current_platform_version
        .as_ref()
        .is_some_and(|current| {
            compare_platform_versions(&listing.platform_version, current) == Some(Ordering::Greater)
        });
    let journal_newer =
        compare_semver_3part(&listing.journal_version, &pos.current_journal_version)
            == Some(Ordering::Greater);
    let desktop_newer = pos
        .components
        .iter()
        .find(|c| c.name == "desktop")
        .and_then(|c| c.version.as_deref())
        .zip(listing.desktop_version.as_deref())
        .is_some_and(|(current, latest)| {
            compare_semver_3part(latest, current) == Some(Ordering::Greater)
        });
    platform_newer || journal_newer || desktop_newer
}

/// The installer arguments for an unattended run: every recorded component,
/// pinned to the verified catalogue, with nothing to answer and nothing it may
/// take as consent. Never `--yes`.
pub fn unattended_invocations(
    script: &Path,
    pos: &PositiveOwnership,
    platform_version: &str,
) -> Result<Vec<Vec<OsString>>, String> {
    validate_safe_arg(&pos.lane)?;
    validate_safe_arg(platform_version)?;
    let script = script.to_str().ok_or("script path is not valid utf-8")?;
    validate_safe_arg(script)?;
    let prefix = match pos.platform_prefix.as_deref() {
        Some(prefix) => {
            let prefix = prefix.to_str().ok_or("prefix path is not valid utf-8")?;
            validate_safe_arg(prefix)?;
            Some(prefix)
        }
        None => None,
    };
    let mut invocations = Vec::new();
    for policy in [ServicePolicy::Start, ServicePolicy::NoStart] {
        let names: Vec<&str> = pos
            .components
            .iter()
            .filter(|c| c.policy == policy)
            .map(|c| c.name.as_str())
            .collect();
        if names.is_empty() {
            continue;
        }
        for name in &names {
            validate_safe_arg(name)?;
        }
        let mut argv: Vec<OsString> = [
            script,
            "--upgrade",
            "--json",
            "--no-path",
            "--non-interactive",
            "--lane",
            &pos.lane,
            "--version",
            platform_version,
            "--components",
            &names.join(","),
        ]
        .iter()
        .map(OsString::from)
        .collect();
        if let Some(prefix) = prefix {
            argv.push("--prefix".into());
            argv.push(prefix.into());
        }
        if policy == ServicePolicy::NoStart {
            argv.push("--no-start".into());
        }
        invocations.push(argv);
    }
    Ok(invocations)
}

fn raise_highest(state: &mut State, version: &str) {
    let higher = state
        .highest_platform_version
        .as_deref()
        .is_none_or(|highest| {
            compare_platform_versions(version, highest) == Some(Ordering::Greater)
        });
    if higher && compare_platform_versions(version, version).is_some() {
        state.highest_platform_version = Some(version.to_owned());
    }
}

fn quiet(result: &str) -> RunOutcome {
    RunOutcome {
        result: result.to_owned(),
        failure: false,
        paused: false,
        installed: None,
    }
}

struct Step {
    result: String,
    failure: bool,
    counts_as_success: bool,
    installed: Option<String>,
}

impl Step {
    fn skipped(result: &str) -> Self {
        Self {
            result: result.to_owned(),
            failure: false,
            counts_as_success: false,
            installed: None,
        }
    }
    fn failed(result: &str) -> Self {
        Self {
            result: result.to_owned(),
            failure: true,
            counts_as_success: false,
            installed: None,
        }
    }
    fn succeeded(result: &str, installed: Option<String>) -> Self {
        Self {
            result: result.to_owned(),
            failure: false,
            counts_as_success: true,
            installed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_metered_connection_is_only_what_networkmanager_says_is_metered() {
        assert_eq!(parse_metered("u 1\n"), Some(true));
        assert_eq!(parse_metered("u 3"), Some(true));
        assert_eq!(parse_metered("u 4"), Some(false));
        assert_eq!(parse_metered("u 0"), Some(false));
        assert_eq!(parse_metered("s \"yes\""), None);
        assert_eq!(parse_metered(""), None);
    }

    #[test]
    fn the_service_runs_the_newest_journal_and_escapes_what_systemd_would_expand() {
        let unit = service_unit(Path::new("/home/o/my 100%$/current/bin/solstone")).unwrap();
        assert!(unit.contains(
            "ExecStart=\"/home/o/my 100%%$$/current/bin/solstone\" update --unattended\n"
        ));
        assert!(unit.contains("Type=oneshot\n"));
        for bad in [
            "/home/o/\"x/solstone",
            "/home/o/a\\b/solstone",
            "/home/o/a\nb/solstone",
        ] {
            assert_eq!(service_unit(Path::new(bad)), None, "{bad:?}");
        }
    }

    #[test]
    fn the_timer_redraws_its_delay_every_day() {
        assert!(TIMER.contains("OnCalendar=daily\n"));
        assert!(TIMER.contains("RandomizedDelaySec=4h\n"));
        assert!(TIMER.contains("Persistent=true\n"));
        // A fixed delay is derived from the machine id: a stable per-computer
        // request time is an identifier.
        assert!(!TIMER.contains("FixedRandomDelay"));
    }

    fn owned(components: &[(&str, ServicePolicy)]) -> PositiveOwnership {
        PositiveOwnership {
            route_kind: RouteKind::PlatformTree,
            prefix: Some(PathBuf::from("/home/o/.local")),
            platform_prefix: Some(PathBuf::from("/home/o/.local")),
            lane: "release".to_owned(),
            current_journal_version: "2.0.80".to_owned(),
            current_platform_version: Some("2.0.80-r1".to_owned()),
            components: components
                .iter()
                .map(|(name, policy)| crate::RecordedComponent {
                    name: (*name).to_owned(),
                    route: "tree".to_owned(),
                    policy: *policy,
                    version: Some("2.0.80".to_owned()),
                })
                .collect(),
        }
    }

    #[test]
    fn an_unattended_install_is_pinned_asks_nothing_and_never_says_yes() {
        let pos = owned(&[
            ("journal", ServicePolicy::Start),
            ("desktop", ServicePolicy::NoStart),
        ]);
        let runs = unattended_invocations(Path::new("/var/tmp/i.sh"), &pos, "2.0.81-r1").unwrap();
        let text: Vec<Vec<String>> = runs
            .iter()
            .map(|argv| {
                argv.iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect()
            })
            .collect();
        assert_eq!(
            text,
            [
                vec![
                    "/var/tmp/i.sh",
                    "--upgrade",
                    "--json",
                    "--no-path",
                    "--non-interactive",
                    "--lane",
                    "release",
                    "--version",
                    "2.0.81-r1",
                    "--components",
                    "journal",
                    "--prefix",
                    "/home/o/.local"
                ],
                vec![
                    "/var/tmp/i.sh",
                    "--upgrade",
                    "--json",
                    "--no-path",
                    "--non-interactive",
                    "--lane",
                    "release",
                    "--version",
                    "2.0.81-r1",
                    "--components",
                    "desktop",
                    "--prefix",
                    "/home/o/.local",
                    "--no-start"
                ],
            ]
        );
        assert!(
            text.iter()
                .flatten()
                .all(|a| a != "--yes" && a != "-y" && a != "--confirm-package-command")
        );
    }

    #[test]
    fn only_a_tree_install_with_a_journal_updates_itself() {
        assert!(eligible(&owned(&[("journal", ServicePolicy::Start)])));
        assert!(eligible(&owned(&[("cli", ServicePolicy::NoStart)])));
        assert!(!eligible(&owned(&[("desktop", ServicePolicy::Start)])));
        let mut package = owned(&[("journal", ServicePolicy::Start)]);
        package.route_kind = RouteKind::PlatformPackage;
        assert!(!eligible(&package));
        let mut mixed = owned(&[
            ("journal", ServicePolicy::Start),
            ("desktop", ServicePolicy::Start),
        ]);
        mixed.route_kind = RouteKind::PlatformMixed;
        mixed.components[1].route = "deb".to_owned();
        assert!(!eligible(&mixed));
    }

    #[test]
    fn a_component_left_behind_by_a_part_way_run_is_still_an_update() {
        let listing = |platform: &str, desktop: &str| CatalogueListing {
            platform_version: platform.to_owned(),
            journal_version: "2.0.80".to_owned(),
            desktop_version: Some(desktop.to_owned()),
            installer: None,
        };
        let mut pos = owned(&[
            ("journal", ServicePolicy::Start),
            ("desktop", ServicePolicy::Start),
        ]);
        assert!(!platform_update_available(
            &pos,
            &listing("2.0.80-r1", "2.0.80")
        ));
        assert!(platform_update_available(
            &pos,
            &listing("2.0.80-r2", "2.0.80")
        ));
        // The header names r4, but the app stayed on its older version.
        pos.current_platform_version = Some("2.0.80-r4".to_owned());
        pos.components[1].version = Some("2.0.46".to_owned());
        assert!(platform_update_available(
            &pos,
            &listing("2.0.80-r4", "2.0.48")
        ));
        pos.components[1].version = Some("2.0.48".to_owned());
        assert!(!platform_update_available(
            &pos,
            &listing("2.0.80-r4", "2.0.48")
        ));
    }

    #[test]
    fn the_highest_coordinate_compares_as_numbers() {
        let mut state = State::default();
        for version in ["2.0.99-r1", "2.0.100-r1", "2.0.99-r9", "2.0.100"] {
            raise_highest(&mut state, version);
        }
        assert_eq!(
            state.highest_platform_version.as_deref(),
            Some("2.0.100-r1")
        );
    }
}

#[cfg(all(test, feature = "full-tests", target_os = "linux"))]
mod run_tests {
    use std::cell::RefCell;
    use std::sync::Mutex;

    use super::*;
    use crate::catalogue::CatalogueRefusal;
    use crate::delegate::InstallerOutput;

    struct Probe;
    impl PackageProbe for Probe {
        fn journal_version(&self, _: &Path) -> Option<String> {
            Some("2.0.80".to_owned())
        }
        fn payload_matches(&self, _: &Path, _: &str) -> bool {
            true
        }
        fn owns_journal(&self, _: &Path) -> bool {
            false
        }
    }

    struct Catalogue(Mutex<Vec<Result<CatalogueListing, CatalogueRefusal>>>);
    impl PlatformCatalogue for Catalogue {
        fn listing(&self, _: &str, _: Option<&str>) -> Result<CatalogueListing, CatalogueRefusal> {
            let mut answers = self.0.lock().unwrap();
            if answers.len() > 1 {
                answers.remove(0)
            } else {
                answers[0].clone()
            }
        }
    }

    struct Copy(Mutex<Vec<Result<PathBuf, CopyFetchError>>>, Mutex<u32>);
    impl InstallerCopyFetch for Copy {
        fn fetch_copy(
            &self,
            _: &str,
            _: &str,
            _: &InstallerCopy,
        ) -> Result<PathBuf, CopyFetchError> {
            *self.1.lock().unwrap() += 1;
            let mut answers = self.0.lock().unwrap();
            if answers.len() > 1 {
                answers.remove(0)
            } else {
                answers[0].clone()
            }
        }
        fn cleanup(&self, _: &Path) {}
    }

    struct Runner(Mutex<Vec<InstallerOutput>>, Mutex<Vec<Vec<OsString>>>);
    impl InstallerRunner for Runner {
        fn run(
            &self,
            _: &Path,
            args: &[OsString],
            stdin: StdinMode,
        ) -> io::Result<InstallerOutput> {
            assert_eq!(stdin, StdinMode::Closed);
            self.1.lock().unwrap().push(args.to_vec());
            let mut answers = self.0.lock().unwrap();
            Ok(if answers.len() > 1 {
                answers.remove(0)
            } else {
                answers[0].clone()
            })
        }
    }

    struct Meter(Option<bool>);
    impl super::Meter for Meter {
        fn metered(&self) -> Option<bool> {
            self.0
        }
    }

    struct Floor(Option<&'static str>);
    impl SecurityFloor for Floor {
        fn linux_floor(&self) -> Option<String> {
            self.0.map(str::to_owned)
        }
    }

    #[derive(Default)]
    struct Recording(RefCell<Vec<Vec<String>>>, bool);
    impl Systemctl for Recording {
        fn user(&self, args: &[&str]) -> io::Result<bool> {
            self.0
                .borrow_mut()
                .push(args.iter().map(|a| (*a).to_owned()).collect());
            Ok(!self.1)
        }
    }

    fn installed(success: bool, code: &str) -> InstallerOutput {
        InstallerOutput {
            success,
            exit_code: Some(i32::from(!success)),
            stdout: serde_json::to_vec(&serde_json::json!({"root_code": code})).unwrap(),
            stderr: Vec::new(),
        }
    }

    fn listing(platform: &str, journal: &str, copy: bool) -> CatalogueListing {
        CatalogueListing {
            platform_version: platform.to_owned(),
            journal_version: journal.to_owned(),
            desktop_version: None,
            installer: copy.then(|| InstallerCopy {
                filename: "install.sh".to_owned(),
                sha256: "a".repeat(64),
                revision: 22,
            }),
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        data: PathBuf,
        etc: PathBuf,
        exe: PathBuf,
        home: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("prefix");
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let entry = format!("2.0.80-{}", &digest[..12]);
        let version = prefix.join("versions").join(&entry);
        fs::create_dir_all(version.join("bin")).unwrap();
        fs::write(version.join("bin/solstone"), b"#!/bin/sh\n").unwrap();
        fs::write(version.join(".archive-sha256"), digest).unwrap();
        fs::write(
            version.join(".release"),
            "product=solstone-journal\nversion=2.0.80\n",
        )
        .unwrap();
        fs::write(prefix.join("install-receipt"), "schema_version=1\norigin=https://updates.solstone.app\nroute=tree\njournal_version=2.0.80\nlane=release\nrole=journal\nservice_policy=start\njournal_state=present\nsignature_verification=verified\n").unwrap();
        std::os::unix::fs::symlink(format!("versions/{entry}"), prefix.join("current")).unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("install.conf"), format!("[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.80-r1\n[component:journal]\nroute=tree\nstatus=installed\n", prefix.display())).unwrap();
        let home = dir.path().join("home");
        Fixture {
            etc: dir.path().join("etc"),
            exe: prefix.join("current/bin/solstone"),
            data,
            home,
            _dir: dir,
        }
    }

    struct Seams {
        catalogue: Catalogue,
        copy: Copy,
        runner: Runner,
        meter: Meter,
        floor: Floor,
    }

    fn seams(listings: Vec<Result<CatalogueListing, CatalogueRefusal>>) -> Seams {
        Seams {
            catalogue: Catalogue(Mutex::new(listings)),
            copy: Copy(
                Mutex::new(vec![Ok(PathBuf::from("/var/tmp/copy.sh"))]),
                Mutex::new(0),
            ),
            runner: Runner(
                Mutex::new(vec![installed(true, "installed")]),
                Mutex::new(Vec::new()),
            ),
            meter: Meter(None),
            floor: Floor(None),
        }
    }

    fn run(f: &Fixture, s: &Seams) -> RunOutcome {
        UnattendedRun {
            catalogue: &s.catalogue,
            copy_fetch: &s.copy,
            runner: &s.runner,
            probe: &Probe,
            meter: &s.meter,
            floor: &s.floor,
            executable: &f.exe,
            data_dir: &f.data,
            etc_dir: &f.etc,
            now_unix: 1_700_000_000,
        }
        .run()
    }

    fn on(f: &Fixture) {
        write_choice(&f.data, Choice::On).unwrap();
    }

    #[test]
    fn nothing_runs_until_the_owner_says_yes() {
        let f = fixture();
        let s = seams(vec![Ok(listing("2.0.81-r1", "2.0.81", true))]);
        assert_eq!(read_choice(&f.data), Choice::Unasked);
        assert_eq!(run(&f, &s).result, "off");
        write_choice(&f.data, Choice::Off).unwrap();
        assert_eq!(run(&f, &s).result, "off");
        assert_eq!(*s.copy.1.lock().unwrap(), 0);
        assert!(s.runner.1.lock().unwrap().is_empty());
        assert_eq!(read_state(&f.data), State::default());
    }

    #[test]
    fn a_newer_signed_release_is_installed_with_its_own_checked_installer() {
        let f = fixture();
        on(&f);
        let s = seams(vec![Ok(listing("2.0.81-r1", "2.0.81", true))]);
        let outcome = run(&f, &s);
        assert_eq!(
            (outcome.result.as_str(), outcome.failure),
            ("installed", false)
        );
        assert_eq!(outcome.installed.as_deref(), Some("2.0.81-r1"));
        assert_eq!(*s.copy.1.lock().unwrap(), 1);
        let args = s.runner.1.lock().unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0][0], OsString::from("/var/tmp/copy.sh"));
        let state = read_state(&f.data);
        assert_eq!(state.last_installed.as_deref(), Some("2.0.81-r1"));
        assert_eq!(state.highest_platform_version.as_deref(), Some("2.0.81-r1"));
        assert_eq!(state.last_check_unix, Some(1_700_000_000));
    }

    #[test]
    fn up_to_date_runs_nothing_and_ends_a_streak_of_failures() {
        let f = fixture();
        on(&f);
        write_state(
            &f.data,
            &State {
                consecutive_failures: 1,
                ..State::default()
            },
        )
        .unwrap();
        let s = seams(vec![Ok(listing("2.0.80-r1", "2.0.80", true))]);
        assert_eq!(run(&f, &s).result, "up-to-date");
        assert_eq!(*s.copy.1.lock().unwrap(), 0);
        assert_eq!(read_state(&f.data).consecutive_failures, 0);
    }

    #[test]
    fn a_tampered_installer_runs_nothing_and_two_failures_pause_until_tried_again() {
        let f = fixture();
        on(&f);
        let mut s = seams(vec![Ok(listing("2.0.81-r1", "2.0.81", true))]);
        s.copy = Copy(
            Mutex::new(vec![Err(CopyFetchError::DigestMismatch)]),
            Mutex::new(0),
        );
        let first = run(&f, &s);
        assert_eq!(
            (first.result.as_str(), first.failure, first.paused),
            ("installer-digest-mismatch", true, false)
        );
        let second = run(&f, &s);
        assert!(second.paused);
        assert!(s.runner.1.lock().unwrap().is_empty());
        // Paused: nothing is fetched until the owner tries again.
        assert_eq!(run(&f, &s).result, "paused");
        assert_eq!(*s.copy.1.lock().unwrap(), 2);

        let systemctl = Recording::default();
        try_again(&f.data, &systemctl).unwrap();
        assert_eq!(
            systemctl.0.borrow().as_slice(),
            [vec!["start", "--no-block", SERVICE_UNIT]]
        );
        assert!(!read_state(&f.data).paused);
    }

    #[test]
    fn a_refused_catalogue_or_an_installer_refusal_is_a_failure_and_offline_is_not() {
        for (answer, copy, runner, expected, failure) in [
            (
                Err(CatalogueRefusal {
                    code: "signature-invalid",
                    message: String::new(),
                }),
                Ok(PathBuf::new()),
                installed(true, ""),
                "signature-invalid",
                true,
            ),
            (
                Err(CatalogueRefusal {
                    code: "fetch-failed",
                    message: String::new(),
                }),
                Ok(PathBuf::new()),
                installed(true, ""),
                "offline",
                false,
            ),
            (
                Ok(listing("2.0.81-r1", "2.0.81", true)),
                Err(CopyFetchError::Offline),
                installed(true, ""),
                "offline",
                false,
            ),
            (
                Ok(listing("2.0.81-r1", "2.0.81", true)),
                Err(CopyFetchError::Unavailable),
                installed(true, ""),
                "installer-unavailable",
                true,
            ),
            (
                Ok(listing("2.0.81-r1", "2.0.81", true)),
                Ok(PathBuf::new()),
                installed(false, "digest-mismatch"),
                "digest-mismatch",
                true,
            ),
            (
                Ok(listing("2.0.81-r1", "2.0.81", true)),
                Ok(PathBuf::new()),
                installed(false, "route-busy"),
                "busy",
                false,
            ),
            (
                Ok(listing("2.0.81-r1", "2.0.81", false)),
                Ok(PathBuf::new()),
                installed(true, ""),
                "waiting-for-signed-installer",
                false,
            ),
        ] {
            let f = fixture();
            on(&f);
            let mut s = seams(vec![answer]);
            s.copy = Copy(Mutex::new(vec![copy]), Mutex::new(0));
            s.runner = Runner(Mutex::new(vec![runner]), Mutex::new(Vec::new()));
            let outcome = run(&f, &s);
            assert_eq!(
                (outcome.result.as_str(), outcome.failure),
                (expected, failure)
            );
            assert_eq!(read_state(&f.data).consecutive_failures, u32::from(failure));
        }
    }

    #[test]
    fn a_catalogue_older_than_one_already_accepted_is_refused_and_numbers_compare_as_numbers() {
        let f = fixture();
        on(&f);
        for (offered, expected) in [
            ("2.0.99-r1", "installed"),
            ("2.0.100-r1", "installed"),
            ("2.0.100-r1", "up-to-date"),
            ("2.0.99-r9", "catalogue-went-backwards"),
            ("2.0.100", "catalogue-went-backwards"),
        ] {
            // The fixture's journal stays at 2.0.80; only the coordinate moves.
            let s = seams(vec![Ok(listing(offered, "2.0.80", true))]);
            let outcome = run(&f, &s);
            assert_eq!(outcome.result, expected, "{offered}");
            if expected == "installed" {
                // The receipt now names what was installed.
                let receipt = f.data.join("install.conf");
                let text = fs::read_to_string(&receipt).unwrap();
                let current = text
                    .lines()
                    .find(|l| l.starts_with("platform_version="))
                    .unwrap()
                    .to_owned();
                fs::write(
                    &receipt,
                    text.replace(&current, &format!("platform_version={offered}")),
                )
                .unwrap();
            }
            write_state(
                &f.data,
                &State {
                    consecutive_failures: 0,
                    paused: false,
                    ..read_state(&f.data)
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn a_security_floor_the_catalogue_does_not_offer_is_a_failure() {
        let f = fixture();
        on(&f);
        let mut s = seams(vec![Ok(listing("2.0.81-r1", "2.0.81", true))]);
        s.floor = Floor(Some("2.0.82"));
        assert_eq!(run(&f, &s).result, "security-update-unavailable");
        // A floor the catalogue offers installs as usual.
        let f = fixture();
        on(&f);
        let mut s = seams(vec![Ok(listing("2.0.82-r1", "2.0.82", true))]);
        s.floor = Floor(Some("2.0.82"));
        assert_eq!(run(&f, &s).result, "installed");
    }

    #[test]
    fn a_metered_connection_or_a_hold_waits_without_counting_as_a_failure() {
        let f = fixture();
        on(&f);
        let mut s = seams(vec![Ok(listing("2.0.81-r1", "2.0.81", true))]);
        s.meter = Meter(Some(true));
        assert_eq!(run(&f, &s).result, "metered");
        crate::set_hold(&f.data).unwrap();
        s.meter = Meter(None);
        assert_eq!(run(&f, &s).result, "held");
        assert_eq!(*s.copy.1.lock().unwrap(), 0);
        assert_eq!(read_state(&f.data).consecutive_failures, 0);
    }

    #[test]
    fn the_switch_writes_both_units_and_off_takes_them_away() {
        let f = fixture();
        let pos = match classify_ownership(&f.exe, &f.data, &f.etc, &Probe) {
            OwnershipResult::Positive(pos) => pos,
            other => panic!("{other:?}"),
        };
        let systemctl = Recording::default();
        turn_on(&pos, &f.data, &f.home, &systemctl).unwrap();
        let units = unit_dir(&f.home);
        assert!(
            fs::read_to_string(units.join(SERVICE_UNIT))
                .unwrap()
                .contains("/current/bin/solstone\" update --unattended")
        );
        assert_eq!(fs::read_to_string(units.join(TIMER_UNIT)).unwrap(), TIMER);
        assert_eq!(read_choice(&f.data), Choice::On);
        assert_eq!(
            systemctl.0.borrow().as_slice(),
            [vec!["daemon-reload"], vec!["enable", "--now", TIMER_UNIT]]
        );

        turn_off(&f.data, &f.home, &systemctl).unwrap();
        assert!(!units.join(SERVICE_UNIT).exists() && !units.join(TIMER_UNIT).exists());
        assert_eq!(read_choice(&f.data), Choice::Off);

        // Without a user session, nothing is left half set up.
        let failing = Recording(RefCell::default(), true);
        assert_eq!(
            turn_on(&pos, &f.data, &f.home, &failing).unwrap_err().code,
            "systemd-unavailable"
        );
        assert!(!units.join(SERVICE_UNIT).exists() && !units.join(TIMER_UNIT).exists());
        assert_eq!(read_choice(&f.data), Choice::Off);
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &fs::metadata(f.data.join(CHOICE_FILENAME))
                .unwrap()
                .permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_journal_waiting_for_its_restart_is_told_from_the_receipt() {
        let f = fixture();
        let prefix = f
            .exe
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let old = fs::canonicalize(&f.exe).unwrap();
        // An update installs 2.0.81 beside the running 2.0.80.
        let digest = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
        let entry = format!("2.0.81-{}", &digest[..12]);
        let new = prefix.join("versions").join(&entry);
        fs::create_dir_all(new.join("bin")).unwrap();
        fs::write(new.join("bin/solstone"), b"#!/bin/sh\n").unwrap();
        fs::write(new.join(".archive-sha256"), digest).unwrap();
        fs::write(
            new.join(".release"),
            "product=solstone-journal\nversion=2.0.81\n",
        )
        .unwrap();
        fs::remove_file(prefix.join("current")).unwrap();
        std::os::unix::fs::symlink(format!("versions/{entry}"), prefix.join("current")).unwrap();

        assert_eq!(
            crate::pending_restart(&old, &f.data, "2.0.80").as_deref(),
            Some("2.0.81")
        );
        assert_eq!(
            crate::pending_restart(&prefix.join("current/bin/solstone"), &f.data, "2.0.81"),
            None
        );
        assert_eq!(crate::pending_restart(&old, &f.data, "2.0.81"), None);
        assert_eq!(receipt_tree_prefix(&f.data), Some(prefix.clone()));
        let package = fs::read_to_string(f.data.join("install.conf"))
            .unwrap()
            .replace(
                "[component:journal]\nroute=tree",
                "[component:journal]\nroute=deb",
            );
        fs::write(f.data.join("install.conf"), package).unwrap();
        assert_eq!(receipt_tree_prefix(&f.data), None);
    }
}
