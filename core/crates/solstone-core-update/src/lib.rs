// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub mod command;
pub mod delegate;
pub mod downgrade;
pub mod fetch;
pub mod hold;
pub mod ownership;

use std::cmp::Ordering;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub use command::{CommandMode, CommandParseError, UpdateArgs, parse_args, render_help};
pub use delegate::{
    InstallerOutput, InstallerRunner, PlatformInvocation, RealInstallerRunner, StdinMode,
    build_check_argv, build_list_argv, build_native_mutation_argv,
    build_platform_mutation_invocations,
};
pub use downgrade::{
    ReleaseMetadata, RollbackRefusal, check_target_admissibility, compare_semver_3part,
    parse_release_metadata, select_newest_admissible_rollback, version_is_older,
};
pub use fetch::{InstallerFetch, InstallerSource, RealInstallerFetch, download_installer};
pub use hold::{
    HOLD_CONTENTS, HOLD_FILENAME, HoldError, HoldState, read_hold_state, remove_hold, set_hold,
};
pub use ownership::{
    OwnershipResult, PackageProbe, PositiveOwnership, RealPackageProbe, RecordedComponent,
    RouteKind, ServicePolicy, classify_ownership, positive_tree_release,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    NotAvailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateTarget {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_version: Option<String>,
    pub journal_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentReport {
    pub role: String,
    pub status: String,
    pub route: String,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateReport {
    pub availability: Availability,
    pub current: Option<String>,
    pub platform_version: Option<String>,
    pub latest: Option<String>,
    pub latest_platform_version: Option<String>,
    pub lane: Option<String>,
    pub route: Option<String>,
    pub hold: HoldState,
    pub target: Option<UpdateTarget>,
    pub code: Option<String>,
    pub message: Option<String>,
    pub components: Option<Vec<ComponentReport>>,
}

impl UpdateReport {
    pub fn checkout_unknown(hold: HoldState) -> Self {
        Self {
            availability: Availability::Unknown,
            current: None,
            platform_version: None,
            latest: None,
            latest_platform_version: None,
            lane: None,
            route: None,
            hold,
            target: None,
            code: Some("source-checkout".to_owned()),
            message: Some(
                "this copy is a source checkout. install the released journal to update it."
                    .to_owned(),
            ),
            components: None,
        }
    }

    pub fn not_owned_unknown(hold: HoldState) -> Self {
        Self {
            availability: Availability::Unknown,
            current: None,
            platform_version: None,
            latest: None,
            latest_platform_version: None,
            lane: None,
            route: None,
            hold,
            target: None,
            code: Some("install-not-owned".to_owned()),
            message: Some(
                "could not identify how this copy of solstone was installed. use the installer that installed it to update it.".to_owned(),
            ),
            components: None,
        }
    }

    pub fn refusal_unknown(code: &str, message: &str, hold: HoldState) -> Self {
        Self {
            availability: Availability::Unknown,
            current: None,
            platform_version: None,
            latest: None,
            latest_platform_version: None,
            lane: None,
            route: None,
            hold,
            target: None,
            code: Some(code.to_owned()),
            message: Some(message.to_owned()),
            components: None,
        }
    }
}

pub fn compare_platform_versions(a: &str, b: &str) -> Option<Ordering> {
    let (a_base, a_rev) = match a.split_once("-r") {
        Some((base, rev)) => (base, rev.parse::<u64>().ok()?),
        None => (a, 0),
    };
    let (b_base, b_rev) = match b.split_once("-r") {
        Some((base, rev)) => (base, rev.parse::<u64>().ok()?),
        None => (b, 0),
    };

    match compare_semver_3part(a_base, b_base)? {
        Ordering::Equal => Some(a_rev.cmp(&b_rev)),
        other => Some(other),
    }
}

pub fn owner_data_dir() -> Option<std::path::PathBuf> {
    owner_data_dir_from(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

fn owner_data_dir_from(
    xdg: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Option<std::path::PathBuf> {
    xdg.map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.map(std::path::PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(".local/share"))
        })
        .map(|path| path.join("solstone"))
}

impl UpdateReport {
    pub fn owner_context_unknown() -> Self {
        Self::refusal_unknown(
            "hold-unknown",
            "the update hold cannot be read.",
            HoldState::Unknown,
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub fn check(
    runner: &dyn InstallerRunner,
    fetch: &dyn InstallerFetch,
    probe: &dyn PackageProbe,
    executable: &Path,
    data_dir: &Path,
    etc_dir: &Path,
    os_linux: bool,
    lane_override: Option<&str>,
) -> UpdateReport {
    check_selected(
        runner,
        fetch,
        probe,
        executable,
        data_dir,
        etc_dir,
        os_linux,
        lane_override,
        None,
    )
}

struct InstallerLease<'a> {
    path: std::path::PathBuf,
    fetch: &'a dyn InstallerFetch,
}
impl Drop for InstallerLease<'_> {
    fn drop(&mut self) {
        self.fetch.cleanup(&self.path);
    }
}
fn acquire(
    fetch: &dyn InstallerFetch,
    source: InstallerSource,
) -> std::io::Result<InstallerLease<'_>> {
    Ok(InstallerLease {
        path: fetch.fetch(source)?,
        fetch,
    })
}

#[allow(clippy::too_many_arguments)]
fn check_selected(
    runner: &dyn InstallerRunner,
    fetch: &dyn InstallerFetch,
    probe: &dyn PackageProbe,
    executable: &Path,
    data_dir: &Path,
    etc_dir: &Path,
    os_linux: bool,
    lane_override: Option<&str>,
    version: Option<&str>,
) -> UpdateReport {
    let hold = read_hold_state(data_dir);

    if !os_linux {
        return UpdateReport::refusal_unknown(
            "non-linux",
            "solstone update is available on linux.",
            HoldState::Unknown,
        );
    }

    if hold == HoldState::Unknown {
        return UpdateReport::refusal_unknown(
            "hold-unknown",
            "the update hold cannot be read.",
            hold,
        );
    }
    let ownership = classify_ownership(executable, data_dir, etc_dir, probe);
    let pos = match ownership {
        OwnershipResult::SourceCheckout => return UpdateReport::checkout_unknown(hold),
        OwnershipResult::NotOwned => return UpdateReport::not_owned_unknown(hold),
        OwnershipResult::Refusal { code, message } => {
            return UpdateReport::refusal_unknown(&code, &message, hold);
        }
        OwnershipResult::Positive(pos) => pos,
    };

    let lane = lane_override.unwrap_or(&pos.lane);

    // Fetch platform installer for catalogue metadata check
    let script_path = match acquire(fetch, InstallerSource::Platform) {
        Ok(p) => p,
        Err(_) => {
            return UpdateReport {
                availability: Availability::Unknown,
                current: Some(pos.current_journal_version),
                platform_version: pos.current_platform_version,
                latest: None,
                latest_platform_version: None,
                lane: Some(lane.to_owned()),
                route: Some(pos.route_kind.as_str().to_owned()),
                hold,
                target: None,
                code: Some("fetch-failed".to_owned()),
                message: Some("could not fetch installer.".to_owned()),
                components: None,
            };
        }
    };

    let check_args = match build_list_argv(&script_path.path, lane, version) {
        Ok(args) => args,
        Err(_) => {
            return UpdateReport {
                availability: Availability::Unknown,
                current: Some(pos.current_journal_version),
                platform_version: pos.current_platform_version,
                latest: None,
                latest_platform_version: None,
                lane: Some(lane.to_owned()),
                route: Some(pos.route_kind.as_str().to_owned()),
                hold,
                target: None,
                code: Some("argv-failed".to_owned()),
                message: Some("invalid installer arguments.".to_owned()),
                components: None,
            };
        }
    };

    let check_output = match runner.run(Path::new("/bin/sh"), &check_args, StdinMode::Closed) {
        Ok(out) => out,
        Err(error) => {
            let timed_out = error.kind() == std::io::ErrorKind::TimedOut;
            return UpdateReport {
                availability: Availability::Unknown,
                current: Some(pos.current_journal_version),
                platform_version: pos.current_platform_version,
                latest: None,
                latest_platform_version: None,
                lane: Some(lane.to_owned()),
                route: Some(pos.route_kind.as_str().to_owned()),
                hold,
                target: None,
                code: Some(
                    if timed_out {
                        "list-timeout"
                    } else {
                        "runner-failed"
                    }
                    .to_owned(),
                ),
                message: Some(if timed_out {
                    "catalogue check timed out. check your connection and try again.".to_owned()
                } else {
                    format!(
                        "could not run installer: {}",
                        bounded_diagnostic(&error.to_string())
                    )
                }),
                components: None,
            };
        }
    };

    if !check_output.success {
        let (code, message) = catalogue_failure(&check_output);
        return UpdateReport {
            availability: Availability::Unknown,
            current: Some(pos.current_journal_version),
            platform_version: pos.current_platform_version,
            latest: None,
            latest_platform_version: None,
            lane: Some(lane.to_owned()),
            route: Some(pos.route_kind.as_str().to_owned()),
            hold,
            target: None,
            code: Some(code),
            message: Some(message),
            components: None,
        };
    }

    let parsed: serde_json::Value = match serde_json::from_slice(&check_output.stdout) {
        Ok(v) => v,
        Err(_) => {
            return UpdateReport {
                availability: Availability::Unknown,
                current: Some(pos.current_journal_version),
                platform_version: pos.current_platform_version,
                latest: None,
                latest_platform_version: None,
                lane: Some(lane.to_owned()),
                route: Some(pos.route_kind.as_str().to_owned()),
                hold,
                target: None,
                code: Some("schema-invalid".to_owned()),
                message: Some("catalogue output was not valid json.".to_owned()),
                components: None,
            };
        }
    };

    let cat_plat_ver = parsed["platform_version"].as_str().map(|s| s.to_owned());
    let cat_journal_ver = parsed["components"]["journal"]["version"]
        .as_str()
        .or_else(|| parsed["components"]["cli"]["version"].as_str())
        .map(|s| s.to_owned());

    let (latest_journal, latest_platform) = match (cat_journal_ver, cat_plat_ver) {
        (Some(j), Some(p)) => (j, p),
        _ => {
            return UpdateReport {
                availability: Availability::Unknown,
                current: Some(pos.current_journal_version),
                platform_version: pos.current_platform_version,
                latest: None,
                latest_platform_version: None,
                lane: Some(lane.to_owned()),
                route: Some(pos.route_kind.as_str().to_owned()),
                hold,
                target: None,
                code: Some("catalogue-incomplete".to_owned()),
                message: Some(
                    "catalogue output is missing platform or journal versions.".to_owned(),
                ),
                components: None,
            };
        }
    };

    if compare_platform_versions(&latest_platform, &latest_platform).is_none()
        || compare_semver_3part(&latest_journal, &latest_journal).is_none()
        || version.is_some_and(|v| v != latest_platform)
    {
        return UpdateReport::refusal_unknown(
            "catalogue-invalid",
            "the selected release metadata is invalid.",
            hold,
        );
    }

    if pos.route_kind == RouteKind::NativeTree {
        // Native tree compares journal component version only
        let is_newer = matches!(
            compare_semver_3part(&latest_journal, &pos.current_journal_version),
            Some(Ordering::Greater)
        );

        let (avail, target) = if is_newer {
            (
                Availability::Available,
                Some(UpdateTarget {
                    platform_version: None,
                    journal_version: latest_journal.clone(),
                }),
            )
        } else {
            (Availability::NotAvailable, None)
        };

        UpdateReport {
            availability: avail,
            current: Some(pos.current_journal_version),
            platform_version: None,
            latest: Some(latest_journal),
            latest_platform_version: Some(latest_platform),
            lane: Some(lane.to_owned()),
            route: Some(pos.route_kind.as_str().to_owned()),
            hold,
            target,
            code: None,
            message: None,
            components: None,
        }
    } else {
        // Platform routes compare platform versions
        let is_newer = if let Some(ref current_pver) = pos.current_platform_version {
            matches!(
                compare_platform_versions(&latest_platform, current_pver),
                Some(Ordering::Greater)
            )
        } else {
            matches!(
                compare_semver_3part(&latest_journal, &pos.current_journal_version),
                Some(Ordering::Greater)
            )
        };

        let (avail, target) = if is_newer {
            (
                Availability::Available,
                Some(UpdateTarget {
                    platform_version: Some(latest_platform.clone()),
                    journal_version: latest_journal.clone(),
                }),
            )
        } else {
            (Availability::NotAvailable, None)
        };

        UpdateReport {
            availability: avail,
            current: Some(pos.current_journal_version),
            platform_version: pos.current_platform_version,
            latest: Some(latest_journal),
            latest_platform_version: Some(latest_platform),
            lane: Some(lane.to_owned()),
            route: Some(pos.route_kind.as_str().to_owned()),
            hold,
            target,
            code: None,
            message: None,
            components: None,
        }
    }
}

fn bounded_diagnostic(text: &str) -> String {
    let mut result = String::new();
    for character in text.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if result.len() + character.len_utf8() > 512 {
            break;
        }
        result.push(character);
    }
    result.trim().to_owned()
}

fn catalogue_failure(output: &InstallerOutput) -> (String, String) {
    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .unwrap_or(serde_json::Value::Null);
    let code = value["root_code"]
        .as_str()
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 64
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        .unwrap_or("list-failed")
        .to_owned();
    let detail = value["message"]
        .as_str()
        .map(bounded_diagnostic)
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| bounded_diagnostic(&String::from_utf8_lossy(&output.stderr)));
    let message = if detail.is_empty() {
        match output.exit_code {
            Some(code) => format!("catalogue check failed (installer exit {code})."),
            None => "catalogue check failed (installer terminated by a signal).".to_owned(),
        }
    } else {
        format!("catalogue check failed: {detail}")
    };
    (code, message)
}

fn base_report(pos: &PositiveOwnership, hold: HoldState, lane: &str) -> UpdateReport {
    UpdateReport {
        availability: Availability::Unknown,
        current: Some(pos.current_journal_version.clone()),
        platform_version: pos.current_platform_version.clone(),
        latest: None,
        latest_platform_version: None,
        lane: Some(lane.to_owned()),
        route: Some(pos.route_kind.as_str().to_owned()),
        hold,
        target: None,
        code: None,
        message: None,
        components: None,
    }
}

fn component_reports(value: &serde_json::Value) -> Vec<ComponentReport> {
    value["components"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.iter())
        .map(|(name, c)| ComponentReport {
            role: c["role"].as_str().unwrap_or(name).to_owned(),
            status: c["status"].as_str().unwrap_or("unknown").to_owned(),
            route: c["route"].as_str().unwrap_or("unknown").to_owned(),
            phase: c["phase"].as_str().unwrap_or("unknown").to_owned(),
            target_version: c["target_version"].as_str().map(str::to_owned),
        })
        .collect()
}

#[allow(clippy::too_many_arguments, clippy::result_large_err)]
pub fn perform(
    runner: &dyn InstallerRunner,
    fetch: &dyn InstallerFetch,
    probe: &dyn PackageProbe,
    executable: &Path,
    data_dir: &Path,
    etc_dir: &Path,
    os_linux: bool,
    args: &UpdateArgs,
) -> Result<UpdateReport, (UpdateReport, i32)> {
    let hold = read_hold_state(data_dir);
    let refusal =
        |code: &str, message: &str| (UpdateReport::refusal_unknown(code, message, hold), 1);
    if !os_linux {
        return Err(refusal(
            "non-linux",
            "solstone update is available on linux.",
        ));
    }
    let pos = match classify_ownership(executable, data_dir, etc_dir, probe) {
        OwnershipResult::SourceCheckout => return Err((UpdateReport::checkout_unknown(hold), 1)),
        OwnershipResult::NotOwned => return Err((UpdateReport::not_owned_unknown(hold), 1)),
        OwnershipResult::Refusal { code, message } => return Err(refusal(&code, &message)),
        OwnershipResult::Positive(pos) => pos,
    };
    let lane = args.lane.as_deref().unwrap_or(&pos.lane);
    let mut report = base_report(&pos, hold, lane);
    match args.mode {
        CommandMode::Help => return Ok(report),
        CommandMode::Hold | CommandMode::Unhold => {
            let held = args.mode == CommandMode::Hold;
            let result = if held {
                set_hold(data_dir)
            } else {
                remove_hold(data_dir)
            };
            result.map_err(|_| {
                refusal(
                    "hold-failed",
                    "could not save the update hold. check your data directory permissions.",
                )
            })?;
            report.hold = if held {
                HoldState::Held
            } else {
                HoldState::Unheld
            };
            report.message = Some(
                if held {
                    "updates are held."
                } else {
                    "updates are no longer held."
                }
                .to_owned(),
            );
            return Ok(report);
        }
        CommandMode::Check => {
            report = check(
                runner,
                fetch,
                probe,
                executable,
                data_dir,
                etc_dir,
                true,
                args.lane.as_deref(),
            );
            report.message = match report.availability {
                Availability::Available => Some("an update is available.".to_owned()),
                Availability::NotAvailable => Some("your journal is up to date.".to_owned()),
                Availability::Unknown => return Err((report, 1)),
            };
            return Ok(report);
        }
        _ => {}
    }
    if hold == HoldState::Held {
        return Err(refusal(
            "held",
            "updates are held. run solstone update --unhold to release them.",
        ));
    }
    if hold == HoldState::Unknown {
        return Err(refusal(
            "hold-unknown",
            "the update hold cannot be read. nothing was changed.",
        ));
    }
    let journal_is_package = pos
        .components
        .iter()
        .any(|c| (c.name == "journal" || c.name == "cli") && c.route != "tree");
    if args.mode == CommandMode::Rollback && journal_is_package {
        return Err(refusal(
            "downgrade-route-unsupported",
            "this package route cannot install an older journal. nothing was changed.",
        ));
    }
    let retained = if args.mode == CommandMode::Rollback
        && pos.route_kind == RouteKind::NativeTree
        && args.version.is_some()
    {
        args.version.clone()
    } else if args.mode == CommandMode::Rollback && args.version.is_none() {
        let prefix = pos.prefix.as_deref().ok_or_else(|| {
            refusal(
                "ownership-unknown",
                "the installation directory could not be identified.",
            )
        })?;
        Some(
            select_newest_admissible_rollback(prefix, &pos.current_journal_version)
                .map_err(|e| refusal(&e.code, &e.message))?
                .0,
        )
    } else {
        None
    };
    // Platform revisions cannot be inferred from a retained native version.
    // Native-only rollback needs no platform mapping.
    let (target_journal, target_platform) = if let Some(ref retained) = retained {
        if pos.route_kind == RouteKind::NativeTree {
            (retained.clone(), None)
        } else {
            return Err(refusal(
                "platform-revision-unknown",
                "could not match this older build to a platform release. choose a platform version with --version.",
            ));
        }
    } else {
        report = check_selected(
            runner,
            fetch,
            probe,
            executable,
            data_dir,
            etc_dir,
            true,
            Some(lane),
            args.version.as_deref(),
        );
        if report.availability == Availability::Unknown {
            return Err((report, 1));
        }
        if args.mode == CommandMode::Update
            && args.version.is_none()
            && args.lane.is_none()
            && report.availability == Availability::NotAvailable
        {
            report.message = Some("your journal is up to date.".to_owned());
            return Ok(report);
        }
        (
            report.latest.clone().expect("validated catalogue journal"),
            report.latest_platform_version.clone(),
        )
    };
    let older = version_is_older(&target_journal, &pos.current_journal_version);
    if older || args.mode == CommandMode::Rollback {
        if journal_is_package {
            return Err(refusal(
                "downgrade-route-unsupported",
                "this package route cannot install an older journal. nothing was changed.",
            ));
        }
        let prefix = pos.prefix.as_deref().ok_or_else(|| {
            refusal(
                "ownership-unknown",
                "the installation directory could not be identified.",
            )
        })?;
        check_target_admissibility(prefix, &pos.current_journal_version, &target_journal)
            .map_err(|e| refusal(&e.code, &e.message))?;
    }
    let source = if pos.route_kind == RouteKind::NativeTree {
        InstallerSource::Native
    } else {
        InstallerSource::Platform
    };
    let script = acquire(fetch, source)
        .map_err(|_| refusal("fetch-failed", "could not fetch the installer."))?;
    let invocations = if pos.route_kind == RouteKind::NativeTree {
        let component = pos.components.first().expect("positive native component");
        vec![PlatformInvocation {
            args: build_native_mutation_argv(
                &script.path,
                pos.prefix.as_deref().expect("positive native prefix"),
                &target_journal,
                lane,
                &component.name,
                component.policy,
            )
            .map_err(|_| refusal("argv-failed", "invalid installer arguments."))?,
            stdin_mode: StdinMode::Closed,
            components: vec![component.name.clone()],
        }]
    } else {
        build_platform_mutation_invocations(
            &script.path,
            &pos.components,
            pos.prefix.as_deref(),
            target_platform.as_deref(),
            lane,
            args.yes,
        )
        .map_err(|_| refusal("argv-failed", "invalid installer arguments."))?
    };
    let mut components = Vec::new();
    for invocation in invocations {
        let out = match runner.run(
            Path::new("/bin/sh"),
            &invocation.args,
            invocation.stdin_mode,
        ) {
            Ok(out) => out,
            Err(_) => {
                report.availability = Availability::Unknown;
                report.current = None;
                report.platform_version = None;
                report.code = Some("runner-failed".to_owned());
                report.message = Some("could not run the installer.".to_owned());
                report.components = Some(components);
                return Err((report, 1));
            }
        };
        let value = serde_json::from_slice::<serde_json::Value>(&out.stdout)
            .unwrap_or(serde_json::Value::Null);
        let details = component_reports(&value);
        if details.is_empty() {
            for name in &invocation.components {
                let comp = pos
                    .components
                    .iter()
                    .find(|c| &c.name == name)
                    .expect("recorded component");
                components.push(ComponentReport {
                    role: name.clone(),
                    status: if out.success { "succeeded" } else { "unknown" }.to_owned(),
                    route: comp.route.clone(),
                    phase: if out.success { "complete" } else { "unknown" }.to_owned(),
                    target_version: None,
                });
            }
        } else {
            components.extend(details);
        }
        if !out.success {
            report.availability = Availability::Unknown;
            report.current = None;
            report.platform_version = None;
            report.code = Some(
                value["root_code"]
                    .as_str()
                    .unwrap_or("installer-failed")
                    .to_owned(),
            );
            report.message = Some(
                value["message"]
                    .as_str()
                    .unwrap_or(
                        "installation did not succeed. use the original installer to recover.",
                    )
                    .to_owned(),
            );
            report.components = Some(components);
            return Err((
                report,
                out.exit_code.filter(|c| *c > 0 && *c < 126).unwrap_or(1),
            ));
        }
    }
    report.current = Some(target_journal);
    report.platform_version = if pos.route_kind == RouteKind::NativeTree {
        None
    } else {
        target_platform
    };
    report.availability = Availability::NotAvailable;
    report.target = None;
    report.components = Some(components);
    report.message = Some("updated your journal.".to_owned());
    Ok(report)
}

pub fn render_human(report: &UpdateReport) -> String {
    let mut lines = Vec::new();
    for (label, value) in [
        ("journal", report.current.as_deref()),
        ("platform", report.platform_version.as_deref()),
        ("lane", report.lane.as_deref()),
        ("route", report.route.as_deref()),
    ] {
        if let Some(value) = value {
            lines.push(format!("{label}: {value}"));
        }
    }
    lines.push(format!(
        "hold: {}",
        match report.hold {
            HoldState::Held => "held",
            HoldState::Unheld => "unheld",
            HoldState::Unknown => "unknown",
        }
    ));
    if let Some(ref target) = report.target {
        lines.push(format!(
            "available: {} (journal {})",
            target
                .platform_version
                .as_deref()
                .unwrap_or(&target.journal_version),
            target.journal_version
        ));
    }
    if let Some(ref code) = report.code {
        lines.push(format!(
            "{code}: {}",
            report
                .message
                .as_deref()
                .unwrap_or("update status is unknown.")
        ));
    } else if let Some(ref message) = report.message {
        lines.push(message.clone());
    }
    format!("{}\n", lines.join("\n"))
}

#[cfg(all(test, feature = "full-tests", target_os = "linux"))]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;

    struct ScriptedFetch {
        script_path: PathBuf,
        fetch_called: Mutex<bool>,
    }

    impl InstallerFetch for ScriptedFetch {
        fn fetch(&self, _source: InstallerSource) -> std::io::Result<PathBuf> {
            *self.fetch_called.lock().unwrap() = true;
            Ok(self.script_path.clone())
        }
    }

    struct ScriptedRunner {
        responses: Mutex<Vec<InstallerOutput>>,
        recorded_args: Mutex<Vec<Vec<OsString>>>,
    }

    impl InstallerRunner for ScriptedRunner {
        fn run(
            &self,
            _program: &Path,
            args: &[OsString],
            _stdin: StdinMode,
        ) -> std::io::Result<InstallerOutput> {
            self.recorded_args.lock().unwrap().push(args.to_vec());
            let mut list = self.responses.lock().unwrap();
            if list.is_empty() {
                Ok(InstallerOutput {
                    success: true,
                    exit_code: Some(0),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                })
            } else {
                Ok(list.remove(0))
            }
        }
    }

    struct MockPackageProbe(bool);
    impl PackageProbe for MockPackageProbe {
        fn journal_version(&self, _executable: &Path) -> Option<String> {
            Some("2.0.48".to_owned())
        }
        fn payload_matches(&self, _executable: &Path, _version: &str) -> bool {
            true
        }
        fn owns_journal(&self, _executable: &Path) -> bool {
            self.0
        }
    }

    fn make_tree(prefix: &Path, ver: &str) {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let entry = format!("{ver}-{}", &digest[..12]);
        let dir = prefix.join("versions").join(&entry);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join("bin/solstone"), b"#!/bin/sh\n").unwrap();
        fs::write(dir.join(".archive-sha256"), digest).unwrap();
        fs::write(prefix.join("install-receipt"), format!("schema_version=1\norigin=https://updates.solstone.app\nroute=tree\njournal_version={ver}\nlane=release\nrole=journal\nservice_policy=start\njournal_state=present\nsignature_verification=verified\n")).unwrap();
        let release = format!(
            "product=solstone-journal\nversion={ver}\nupgrade_epoch=journal-v2\nretention_window=3\nstate_reader_min=1.0.0\nstate_reader_max={ver}\n"
        );
        fs::write(dir.join(".release"), release).unwrap();
        #[cfg(unix)]
        {
            let current = prefix.join("current");
            let _ = fs::remove_file(&current);
            std::os::unix::fs::symlink(format!("versions/{entry}"), &current).unwrap();
        }
    }

    #[test]
    fn native_cli_and_no_start_receipts_support_check_and_delegate() {
        // These native receipt values come from distribution/install.sh;
        // package receipts use the distinct skip-service vocabulary.
        for platform in [true, false] {
            for (role, policy) in [
                ("cli", "none"),
                ("journal", "no-start"),
                ("journal", "start"),
            ] {
                let (dir, prefix, data, etc) = fixture();
                let receipt = prefix.join("install-receipt");
                let native = fs::read_to_string(&receipt)
                    .unwrap()
                    .replace("role=journal\n", &format!("role={role}\n"))
                    .replace(
                        "service_policy=start\n",
                        &format!("service_policy={policy}\n"),
                    );
                fs::write(&receipt, native).unwrap();
                let ini = data.join("install.conf");
                if platform {
                    let contents = fs::read_to_string(&ini)
                        .unwrap()
                        .replace("component:journal", &format!("component:{role}"));
                    fs::write(&ini, contents).unwrap();
                } else {
                    fs::remove_file(&ini).unwrap();
                }
                let list = serde_json::json!({"lane":"release","platform_version":"2.0.49-r1","components":{"journal":{"version":"2.0.49"}}});
                let runner = ScriptedRunner {
                    responses: Mutex::new(vec![
                        response(true, list.clone()),
                        response(true, list),
                        response(true, serde_json::json!({})),
                    ]),
                    recorded_args: Mutex::new(vec![]),
                };
                let fetch = ScriptedFetch {
                    script_path: dir.path().join("installer.sh"),
                    fetch_called: Mutex::new(false),
                };
                let exe = prefix.join("current/bin/solstone");
                let report = check(
                    &runner,
                    &fetch,
                    &MockPackageProbe(false),
                    &exe,
                    &data,
                    &etc,
                    true,
                    None,
                );
                assert_eq!(
                    report.availability,
                    Availability::Available,
                    "{platform}/{role}/{policy}: {report:?}"
                );
                let args = parse_args(&[OsString::from("--yes")]).unwrap();
                perform(
                    &runner,
                    &fetch,
                    &MockPackageProbe(false),
                    &exe,
                    &data,
                    &etc,
                    true,
                    &args,
                )
                .unwrap();
                let calls = runner.recorded_args.lock().unwrap();
                assert_eq!(calls.len(), 3);
                let mutation = calls.last().unwrap();
                assert_eq!(
                    mutation.contains(&OsString::from("--no-start")),
                    policy != "start"
                );
                let role_flag = if platform { "--components" } else { "--role" };
                assert!(
                    mutation
                        .windows(2)
                        .any(|w| w == [OsString::from(role_flag), OsString::from(role)])
                );
            }
        }
    }

    #[test]
    fn check_returns_available_on_newer_catalogue_version() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_tree(&prefix, "2.0.48");

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix.display()
        );
        fs::write(data.join("install.conf"), ini).unwrap();
        let etc = temp.path().join("etc");

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let list_json = r#"{"status":"success","root_code":"list","platform_version":"2.0.49-r1","lane":"release","components":{"journal":{"version":"2.0.49"}}}"#;
        let runner = ScriptedRunner {
            responses: Mutex::new(vec![InstallerOutput {
                success: true,
                exit_code: Some(0),
                stdout: list_json.as_bytes().to_vec(),
                stderr: Vec::new(),
            }]),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let rep = check(&runner, &fetch, &probe, &exe, &data, &etc, true, None);
        assert_eq!(rep.availability, Availability::Available);
        assert_eq!(rep.latest.as_deref(), Some("2.0.49"));
        assert_eq!(rep.latest_platform_version.as_deref(), Some("2.0.49-r1"));
    }

    #[test]
    fn check_returns_available_on_platform_respin_with_same_journal_version() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_tree(&prefix, "2.0.48");

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix.display()
        );
        fs::write(data.join("install.conf"), ini).unwrap();
        let etc = temp.path().join("etc");

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let list_json = r#"{"status":"success","root_code":"list","platform_version":"2.0.48-r2","lane":"release","components":{"journal":{"version":"2.0.48"}}}"#;
        let runner = ScriptedRunner {
            responses: Mutex::new(vec![InstallerOutput {
                success: true,
                exit_code: Some(0),
                stdout: list_json.as_bytes().to_vec(),
                stderr: Vec::new(),
            }]),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let rep = check(&runner, &fetch, &probe, &exe, &data, &etc, true, None);
        assert_eq!(rep.availability, Availability::Available);
        assert_eq!(rep.latest.as_deref(), Some("2.0.48"));
        assert_eq!(rep.latest_platform_version.as_deref(), Some("2.0.48-r2"));
    }

    #[test]
    fn check_returns_not_available_on_same_catalogue_version() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_tree(&prefix, "2.0.48");

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix.display()
        );
        fs::write(data.join("install.conf"), ini).unwrap();
        let etc = temp.path().join("etc");

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let list_json = r#"{"status":"success","root_code":"list","platform_version":"2.0.48-r1","lane":"release","components":{"journal":{"version":"2.0.48"}}}"#;
        let runner = ScriptedRunner {
            responses: Mutex::new(vec![InstallerOutput {
                success: true,
                exit_code: Some(0),
                stdout: list_json.as_bytes().to_vec(),
                stderr: Vec::new(),
            }]),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let rep = check(&runner, &fetch, &probe, &exe, &data, &etc, true, None);
        assert_eq!(rep.availability, Availability::NotAvailable);
    }

    #[test]
    fn check_returns_not_available_when_catalogue_is_older() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_tree(&prefix, "2.0.48");

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r2\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix.display()
        );
        fs::write(data.join("install.conf"), ini).unwrap();
        let etc = temp.path().join("etc");

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let list_json = r#"{"status":"success","root_code":"list","platform_version":"2.0.47-r1","lane":"release","components":{"journal":{"version":"2.0.47"}}}"#;
        let runner = ScriptedRunner {
            responses: Mutex::new(vec![InstallerOutput {
                success: true,
                exit_code: Some(0),
                stdout: list_json.as_bytes().to_vec(),
                stderr: Vec::new(),
            }]),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let rep = check(&runner, &fetch, &probe, &exe, &data, &etc, true, None);
        assert_eq!(rep.availability, Availability::NotAvailable);
    }

    #[test]
    fn check_returns_not_available_for_native_tree_on_same_journal_version_respin() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_tree(&prefix, "2.0.48");

        let receipt = "schema_version=1\norigin=https://updates.solstone.app\njournal_version=2.0.48\nlane=release\nroute=tree\nrole=journal\nservice_policy=start\nsignature_verification=verified\n";
        fs::write(prefix.join("install-receipt"), receipt).unwrap();

        let data = temp.path().join("data");
        let etc = temp.path().join("etc");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&etc).unwrap();

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        // Platform revision updated, but journal version is identical (2.0.48)
        let list_json = r#"{"status":"success","root_code":"list","platform_version":"2.0.48-r2","lane":"release","components":{"journal":{"version":"2.0.48"}}}"#;
        let runner = ScriptedRunner {
            responses: Mutex::new(vec![InstallerOutput {
                success: true,
                exit_code: Some(0),
                stdout: list_json.as_bytes().to_vec(),
                stderr: Vec::new(),
            }]),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let rep = check(&runner, &fetch, &probe, &exe, &data, &etc, true, None);
        assert_eq!(rep.availability, Availability::NotAvailable);
    }

    #[test]
    fn check_preserves_availability_reporting_when_hold_active() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_tree(&prefix, "2.0.48");

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        set_hold(&data).unwrap(); // Hold active

        let ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix.display()
        );
        fs::write(data.join("install.conf"), ini).unwrap();
        let etc = temp.path().join("etc");

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let list_json = r#"{"status":"success","root_code":"list","platform_version":"2.0.49-r1","lane":"release","components":{"journal":{"version":"2.0.49"}}}"#;
        let runner = ScriptedRunner {
            responses: Mutex::new(vec![InstallerOutput {
                success: true,
                exit_code: Some(0),
                stdout: list_json.as_bytes().to_vec(),
                stderr: Vec::new(),
            }]),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let rep = check(&runner, &fetch, &probe, &exe, &data, &etc, true, None);
        assert_eq!(rep.hold, HoldState::Held);
        assert_eq!(rep.availability, Availability::Available);
    }

    #[test]
    fn check_returns_unknown_on_manifest_signature_failure_or_non_zero_exit() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_tree(&prefix, "2.0.48");

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix.display()
        );
        fs::write(data.join("install.conf"), ini).unwrap();
        let etc = temp.path().join("etc");

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let runner = ScriptedRunner {
            responses: Mutex::new(vec![InstallerOutput {
                success: false,
                exit_code: Some(1),
                stdout: b"signature verification failed".to_vec(),
                stderr: Vec::new(),
            }]),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let rep = check(&runner, &fetch, &probe, &exe, &data, &etc, true, None);
        assert_eq!(rep.availability, Availability::Unknown);
    }

    #[test]
    fn check_returns_unknown_on_malformed_receipt_or_ambiguous_ownership() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("install.conf"), b"bad syntax not ini").unwrap();
        let etc = temp.path().join("etc");

        let exe = PathBuf::from("/usr/bin/solstone");
        let probe = MockPackageProbe(false);

        let runner = ScriptedRunner {
            responses: Mutex::new(Vec::new()),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let rep = check(&runner, &fetch, &probe, &exe, &data, &etc, true, None);
        assert_eq!(rep.availability, Availability::Unknown);
    }

    #[test]
    fn check_executes_no_mutations_and_runner_sees_only_list_flag() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_tree(&prefix, "2.0.48");

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix.display()
        );
        fs::write(data.join("install.conf"), ini).unwrap();
        let etc = temp.path().join("etc");

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let list_json = r#"{"status":"success","root_code":"list","platform_version":"2.0.49-r1","lane":"release","components":{"journal":{"version":"2.0.49"}}}"#;
        let runner = ScriptedRunner {
            responses: Mutex::new(vec![InstallerOutput {
                success: true,
                exit_code: Some(0),
                stdout: list_json.as_bytes().to_vec(),
                stderr: Vec::new(),
            }]),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let _ = check(&runner, &fetch, &probe, &exe, &data, &etc, true, None);

        let recorded = runner.recorded_args.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0].contains(&OsString::from("--list")));
        assert!(!recorded[0].contains(&OsString::from("--upgrade")));
    }

    #[test]
    fn perform_refuses_held_or_hold_unknown_before_fetch_or_runner() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_tree(&prefix, "2.0.48");

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        set_hold(&data).unwrap(); // held

        let ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix.display()
        );
        fs::write(data.join("install.conf"), ini).unwrap();
        let etc = temp.path().join("etc");

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let runner = ScriptedRunner {
            responses: Mutex::new(Vec::new()),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let args = UpdateArgs {
            mode: CommandMode::Update,
            json: false,
            lane: None,
            version: None,
            yes: false,
        };

        let res = perform(&runner, &fetch, &probe, &exe, &data, &etc, true, &args);
        let (err_rep, exit) = res.unwrap_err();
        assert_eq!(exit, 1);
        assert_eq!(err_rep.code.as_deref(), Some("held"));
        assert!(!*fetch.fetch_called.lock().unwrap());
        assert_eq!(runner.recorded_args.lock().unwrap().len(), 0);
    }

    #[test]
    fn perform_plain_not_available_does_not_fetch_or_spawn() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_tree(&prefix, "2.0.48");

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix.display()
        );
        fs::write(data.join("install.conf"), ini).unwrap();
        let etc = temp.path().join("etc");

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let list_json = r#"{"status":"success","root_code":"list","platform_version":"2.0.48-r1","lane":"release","components":{"journal":{"version":"2.0.48"}}}"#;
        let runner = ScriptedRunner {
            responses: Mutex::new(vec![InstallerOutput {
                success: true,
                exit_code: Some(0),
                stdout: list_json.as_bytes().to_vec(),
                stderr: Vec::new(),
            }]),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let args = UpdateArgs {
            mode: CommandMode::Update,
            json: false,
            lane: None,
            version: None,
            yes: false,
        };

        let res = perform(&runner, &fetch, &probe, &exe, &data, &etc, true, &args);
        let rep = res.expect("successful plain perform when up to date");
        assert_eq!(rep.availability, Availability::NotAvailable);
        assert_eq!(rep.message.as_deref(), Some("your journal is up to date."));

        // Runner was only called once for check, no mutating invocation
        let recorded = runner.recorded_args.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0].contains(&OsString::from("--list")));
    }

    #[test]
    fn perform_rollback_package_refuses_with_downgrade_route_unsupported() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();

        let etc = temp.path().join("etc");
        let etc_sol = etc.join("solstone");
        fs::create_dir_all(&etc_sol).unwrap();

        let ini = "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=deb\nphase=complete\nstatus=installed\npackage_name=solstone-journal\npackage_version=2.0.48\nservice_policy=start\n";
        fs::write(etc_sol.join("install.conf"), ini).unwrap();

        let exe = PathBuf::from("/usr/bin/solstone");
        let probe = MockPackageProbe(true);

        let runner = ScriptedRunner {
            responses: Mutex::new(Vec::new()),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let args = UpdateArgs {
            mode: CommandMode::Rollback,
            json: false,
            lane: None,
            version: None,
            yes: false,
        };

        let res = perform(&runner, &fetch, &probe, &exe, &data, &etc, true, &args);
        let (err_rep, exit) = res.unwrap_err();
        assert_eq!(exit, 1);
        assert_eq!(err_rep.code.as_deref(), Some("downgrade-route-unsupported"));
        assert_eq!(
            err_rep.message.as_deref(),
            Some("this package route cannot install an older journal. nothing was changed.")
        );
    }

    #[test]
    fn perform_rollback_platform_tree_with_explicit_version_lists_metadata_and_verifies_admissibility()
     {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_tree(&prefix, "2.0.48");

        // Add retained version 2.0.47
        let digest = "1111111111111111111111111111111111111111111111111111111111111111";
        let entry = format!("2.0.47-{}", &digest[..12]);
        let dir = prefix.join("versions").join(&entry);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join(".archive-sha256"), digest).unwrap();
        fs::write(prefix.join("install-receipt"), String::from("schema_version=1\norigin=https://updates.solstone.app\nroute=tree\njournal_version=2.0.48\nlane=release\nrole=journal\nservice_policy=start\njournal_state=present\nsignature_verification=verified\n")).unwrap();
        let release = "product=solstone-journal\nversion=2.0.47\nupgrade_epoch=journal-v2\nretention_window=3\nstate_reader_min=1.0.0\nstate_reader_max=2.0.47\n";
        fs::write(dir.join(".release"), release).unwrap();

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix.display()
        );
        fs::write(data.join("install.conf"), ini).unwrap();
        let etc = temp.path().join("etc");

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let list_json = r#"{"status":"success","root_code":"list","platform_version":"2.0.47-r1","lane":"release","components":{"journal":{"version":"2.0.47"}}}"#;
        let mutation_json = r#"{"status":"success","root_code":"upgrade","components":[{"role":"journal","status":"installed","route":"tree","phase":"complete","target_version":"2.0.47"}]}"#;
        let runner = ScriptedRunner {
            responses: Mutex::new(vec![
                InstallerOutput {
                    success: true,
                    exit_code: Some(0),
                    stdout: list_json.as_bytes().to_vec(),
                    stderr: Vec::new(),
                },
                InstallerOutput {
                    success: true,
                    exit_code: Some(0),
                    stdout: mutation_json.as_bytes().to_vec(),
                    stderr: Vec::new(),
                },
            ]),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let args = UpdateArgs {
            mode: CommandMode::Rollback,
            json: false,
            lane: None,
            version: Some("2.0.47-r1".to_owned()),
            yes: true,
        };

        let res = perform(&runner, &fetch, &probe, &exe, &data, &etc, true, &args);
        let rep = res.expect("rollback succeeded");
        assert_eq!(rep.current.as_deref(), Some("2.0.47"));
        assert_eq!(rep.platform_version.as_deref(), Some("2.0.47-r1"));

        let recorded = runner.recorded_args.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        // First was list with --version 2.0.47-r1
        assert!(recorded[0].contains(&OsString::from("--list")));
        assert!(recorded[0].contains(&OsString::from("2.0.47-r1")));
        // Second was mutation with --version 2.0.47-r1
        assert!(recorded[1].contains(&OsString::from("--upgrade")));
        assert!(recorded[1].contains(&OsString::from("2.0.47-r1")));
    }

    #[test]
    fn perform_rollback_platform_tree_without_version_refuses_when_latest_catalogue_mismatches_retained()
     {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_tree(&prefix, "2.0.48");

        // Retained version is 2.0.47
        let digest = "1111111111111111111111111111111111111111111111111111111111111111";
        let entry = format!("2.0.47-{}", &digest[..12]);
        let dir = prefix.join("versions").join(&entry);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join(".archive-sha256"), digest).unwrap();
        fs::write(prefix.join("install-receipt"), String::from("schema_version=1\norigin=https://updates.solstone.app\nroute=tree\njournal_version=2.0.48\nlane=release\nrole=journal\nservice_policy=start\njournal_state=present\nsignature_verification=verified\n")).unwrap();
        let release = "product=solstone-journal\nversion=2.0.47\nupgrade_epoch=journal-v2\nretention_window=3\nstate_reader_min=1.0.0\nstate_reader_max=2.0.47\n";
        fs::write(dir.join(".release"), release).unwrap();

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix.display()
        );
        fs::write(data.join("install.conf"), ini).unwrap();
        let etc = temp.path().join("etc");

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        // Catalogue latest in lane is 2.0.48-r2 (not 2.0.47)
        let list_json = r#"{"status":"success","root_code":"list","platform_version":"2.0.48-r2","lane":"release","components":{"journal":{"version":"2.0.48"}}}"#;
        let runner = ScriptedRunner {
            responses: Mutex::new(vec![InstallerOutput {
                success: true,
                exit_code: Some(0),
                stdout: list_json.as_bytes().to_vec(),
                stderr: Vec::new(),
            }]),
            recorded_args: Mutex::new(Vec::new()),
        };
        let fetch = ScriptedFetch {
            script_path: temp.path().join("install.sh"),
            fetch_called: Mutex::new(false),
        };

        let args = UpdateArgs {
            mode: CommandMode::Rollback,
            json: false,
            lane: None,
            version: None,
            yes: true,
        };

        let res = perform(&runner, &fetch, &probe, &exe, &data, &etc, true, &args);
        let (err_rep, exit) = res.unwrap_err();
        assert_eq!(exit, 1);
        assert_eq!(err_rep.code.as_deref(), Some("platform-revision-unknown"));
        let recorded = runner.recorded_args.lock().unwrap();
        assert!(recorded.is_empty());
    }
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("prefix");
        make_tree(&prefix, "2.0.48");
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("install.conf"), format!("[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n[component:journal]\nroute=tree\nstatus=installed\n", prefix.display())).unwrap();
        let etc = dir.path().join("etc");
        (dir, prefix, data, etc)
    }
    #[test]
    fn catalogue_refusal_and_timeout_reach_the_public_report() {
        struct TimedOut;
        impl InstallerRunner for TimedOut {
            fn run(
                &self,
                _: &Path,
                _: &[OsString],
                _: StdinMode,
            ) -> std::io::Result<InstallerOutput> {
                Err(std::io::ErrorKind::TimedOut.into())
            }
        }
        let (dir, prefix, data, etc) = fixture();
        let fetch = ScriptedFetch {
            script_path: dir.path().join("installer.sh"),
            fetch_called: Mutex::new(false),
        };
        let exe = prefix.join("current/bin/solstone");
        let report = check(
            &TimedOut,
            &fetch,
            &MockPackageProbe(false),
            &exe,
            &data,
            &etc,
            true,
            None,
        );
        assert_eq!(report.availability, Availability::Unknown);
        assert_eq!(report.code.as_deref(), Some("list-timeout"));
        assert!(report.message.unwrap().len() <= 512);

        // Fields captured from the real installer's --list --json refusal.
        let refusal = serde_json::json!({"root_code":"lane-invalid","message":"Invalid release lane: invalid"});
        let runner = ScriptedRunner {
            responses: Mutex::new(vec![response(false, refusal.clone())]),
            recorded_args: Mutex::new(vec![]),
        };
        let report = check(
            &runner,
            &fetch,
            &MockPackageProbe(false),
            &exe,
            &data,
            &etc,
            true,
            None,
        );
        assert_eq!(report.availability, Availability::Unknown);
        assert_eq!(report.code.as_deref(), refusal["root_code"].as_str());
        assert!(
            report
                .message
                .unwrap()
                .contains(refusal["message"].as_str().unwrap())
        );
    }
    fn response(success: bool, value: serde_json::Value) -> InstallerOutput {
        InstallerOutput {
            success,
            exit_code: Some(if success { 0 } else { 1 }),
            stdout: serde_json::to_vec(&value).unwrap(),
            stderr: vec![],
        }
    }
    #[test]
    fn explicit_version_and_lane_reach_both_verified_check_and_mutation() {
        let (dir, prefix, data, etc) = fixture();
        let runner = ScriptedRunner {
            responses: Mutex::new(vec![
                response(
                    true,
                    serde_json::json!({"lane":"staging","platform_version":"2.0.49-r3","components":{"journal":{"version":"2.0.49"}}}),
                ),
                response(
                    true,
                    serde_json::json!({"components":{"journal":{"role":"journal","status":"succeeded","route":"tree","phase":"complete"}}}),
                ),
            ]),
            recorded_args: Mutex::new(vec![]),
        };
        let fetch = ScriptedFetch {
            script_path: dir.path().join("installer.sh"),
            fetch_called: Mutex::new(false),
        };
        let args = parse_args(
            &["--version", "2.0.49-r3", "--lane", "staging", "--yes"].map(OsString::from),
        )
        .unwrap();
        let report = perform(
            &runner,
            &fetch,
            &MockPackageProbe(false),
            &prefix.join("current/bin/solstone"),
            &data,
            &etc,
            true,
            &args,
        )
        .unwrap();
        assert_eq!(report.lane.as_deref(), Some("staging"));
        assert_eq!(report.platform_version.as_deref(), Some("2.0.49-r3"));
        let calls = runner.recorded_args.lock().unwrap();
        assert_eq!(calls.len(), 2);
        for call in calls.iter() {
            assert!(
                call.windows(2)
                    .any(|w| w == [OsString::from("--lane"), OsString::from("staging")])
            );
            assert!(
                call.windows(2)
                    .any(|w| w == [OsString::from("--version"), OsString::from("2.0.49-r3")])
            );
        }
    }
    #[test]
    fn partial_component_object_keeps_earlier_success_and_recovery_details() {
        let (dir, prefix, data, etc) = fixture();
        let receipt = data.join("install.conf");
        let mut contents = fs::read_to_string(&receipt).unwrap();
        contents.push_str(
            "[component:desktop]\nroute=tree\nstatus=installed\nno_start=1\nversion=2.0.48\n",
        );
        fs::write(&receipt, contents).unwrap();
        let runner = ScriptedRunner {
            responses: Mutex::new(vec![
                response(
                    true,
                    serde_json::json!({"lane":"release","platform_version":"2.0.49-r1","components":{"journal":{"version":"2.0.49"}}}),
                ),
                response(
                    true,
                    serde_json::json!({"components":{"journal":{"role":"journal","status":"succeeded","route":"tree","phase":"complete"}}}),
                ),
                response(
                    false,
                    serde_json::json!({"root_code":"fetch-failed","message":"run solstone update again to resume.","components":{"desktop":{"role":"desktop","status":"failed","route":"tree","phase":"download"},"tmux":{"role":"tmux","status":"unattempted","route":"tree","phase":"pending"}}}),
                ),
            ]),
            recorded_args: Mutex::new(vec![]),
        };
        let fetch = ScriptedFetch {
            script_path: dir.path().join("installer.sh"),
            fetch_called: Mutex::new(false),
        };
        let args = parse_args(&[OsString::from("--yes")]).unwrap();
        let (report, code) = perform(
            &runner,
            &fetch,
            &MockPackageProbe(false),
            &prefix.join("current/bin/solstone"),
            &data,
            &etc,
            true,
            &args,
        )
        .unwrap_err();
        assert_ne!(code, 0);
        assert_eq!(report.code.as_deref(), Some("fetch-failed"));
        assert!(report.current.is_none() && report.platform_version.is_none());
        let details = report.components.unwrap();
        assert!(
            details
                .iter()
                .any(|c| c.role == "journal" && c.status == "succeeded")
        );
        assert!(
            details
                .iter()
                .any(|c| c.role == "desktop" && c.status == "failed")
        );
        assert!(
            details
                .iter()
                .any(|c| c.role == "tmux" && c.status == "unattempted")
        );
        assert_eq!(runner.recorded_args.lock().unwrap().len(), 3);
    }

    #[test]
    fn real_delegate_executes_the_selected_installer_through_its_shell_grammar() {
        let (dir, prefix, data, etc) = fixture();
        let script = dir.path().join("installer.sh");
        let marker = dir.path().join("actual-argv");
        fs::write(&script, format!(r#"case "$1" in
--list) printf '%s\n' '{{"platform_version":"2.0.49-r3","lane":"staging","components":{{"journal":{{"version":"2.0.49"}}}}}}';;
--upgrade) printf '%s\n' "$@" > '{}'; printf '%s\n' '{{"components":{{"journal":{{"role":"journal","status":"succeeded","route":"tree","phase":"complete"}}}}}}';;
*) exit 19;;
esac
"#, marker.display())).unwrap();
        let fetch = ScriptedFetch {
            script_path: script,
            fetch_called: Mutex::new(false),
        };
        let args = parse_args(
            &["--lane", "staging", "--version", "2.0.49-r3", "--yes"].map(OsString::from),
        )
        .unwrap();
        let result = perform(
            &RealInstallerRunner,
            &fetch,
            &MockPackageProbe(false),
            &prefix.join("current/bin/solstone"),
            &data,
            &etc,
            true,
            &args,
        )
        .unwrap();
        assert_eq!(result.platform_version.as_deref(), Some("2.0.49-r3"));
        let actual = fs::read_to_string(marker).unwrap();
        assert!(actual.contains("--lane\nstaging\n") && actual.contains("--version\n2.0.49-r3\n"));
        assert!(actual.contains("--confirm-package-command\n") && actual.contains("--no-path\n"));
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[test]
    fn failure_details_are_bounded_and_control_free() {
        let output = InstallerOutput {
            success: false,
            exit_code: Some(17),
            stdout: serde_json::to_vec(
                &serde_json::json!({"root_code":"x".repeat(65),"message":"é\n\u{1b}".repeat(500)}),
            )
            .unwrap(),
            stderr: vec![],
        };
        let (code, message) = catalogue_failure(&output);
        assert_eq!(code, "list-failed");
        assert!(message.len() <= "catalogue check failed: ".len() + 512);
        assert!(!message.chars().any(char::is_control));
        let mut output = output;
        output.stdout = b"not JSON".to_vec();
        output.stderr = b"connection refused\n".repeat(500);
        let (_, message) = catalogue_failure(&output);
        assert!(message.contains("connection refused"));
        assert!(message.len() <= "catalogue check failed: ".len() + 512);
        output.stderr.clear();
        assert!(catalogue_failure(&output).1.contains("17"));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod owner_context_tests {
    use super::*;
    use std::ffi::OsStr;
    #[test]
    fn owner_data_directory_never_falls_back_to_the_working_directory() {
        assert_eq!(owner_data_dir_from(None, None), None);
        assert_eq!(
            owner_data_dir_from(
                Some(OsStr::new("relative")),
                Some(OsStr::new("also-relative"))
            ),
            None
        );
        assert_eq!(
            owner_data_dir_from(Some(OsStr::new("/data")), Some(OsStr::new("/home/owner"))),
            Some("/data/solstone".into())
        );
        assert_eq!(
            owner_data_dir_from(
                Some(OsStr::new("relative")),
                Some(OsStr::new("/home/owner"))
            ),
            Some("/home/owner/.local/share/solstone".into())
        );
    }
}
