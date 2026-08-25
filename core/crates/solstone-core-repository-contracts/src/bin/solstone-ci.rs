// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Serialize;
use solstone_core_repository_contracts::ci::{
    Leg, PackageSuite, Registry, Suite, load_registry, scan_routine_boundaries, validate_boundary,
    validate_registry,
};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn main() {
    match run() {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("solstone-ci: {error}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<i32, String> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "plan".to_owned());
    let repo = repo_root()?;
    let registry_path = repo.join("core/ci/suites.toml");

    match command.as_str() {
        "validate" => {
            if args.next().is_some() {
                return Err("validate does not accept arguments".to_owned());
            }
            validate_all(&repo, &registry_path)?;
            Ok(0)
        }
        "plan" | "run" => {
            let (selectors, receipt) = parse_selectors(args)?;
            validate_all(&repo, &registry_path)?;
            let registry = load_registry(&registry_path)?;
            let plan = select(&registry, &selectors)?;
            print_plan(&plan, &selectors);
            if command == "plan" {
                if receipt.is_some() {
                    return Err("--receipt is valid only with run".to_owned());
                }
                return Ok(0);
            }
            execute(&repo, &registry, selectors, plan, receipt)
        }
        "boundary-snapshot" => {
            if args.next().is_some() {
                return Err("boundary-snapshot does not accept arguments".to_owned());
            }
            println!("version = 1");
            for id in scan_routine_boundaries(&repo)? {
                println!("\n[[findings]]");
                println!(
                    "id = {}",
                    serde_json::to_string(&id).map_err(|error| error.to_string())?
                );
            }
            Ok(0)
        }
        "help" | "--help" | "-h" => {
            println!(
                "usage: solstone-ci <validate|plan|run|boundary-snapshot> [--sets CSV] [--areas CSV] [--packages CSV] [--targets CSV] [--receipt PATH]"
            );
            println!("selectors union within a dimension and intersect across dimensions");
            Ok(0)
        }
        _ => Err(format!("unknown command {command:?}")),
    }
}

fn validate_all(repo: &Path, registry_path: &Path) -> Result<(), String> {
    let registry = load_registry(registry_path)?;
    let boundary = scan_routine_boundaries(repo)?;
    let mut errors = Vec::new();
    if let Err(found) = validate_registry(repo, &registry) {
        errors.extend(found);
    }
    if let Err(found) = validate_boundary(repo) {
        errors.extend(found);
    }
    if errors.is_empty() {
        println!(
            "CI topology valid: {} Cargo integration targets, {} package scopes, {} named legs, {} hard-boundary findings",
            registry.suites.len(),
            registry.package_suites.len(),
            registry.legs.len(),
            boundary.len()
        );
        Ok(())
    } else {
        for error in &errors {
            eprintln!("- {error}");
        }
        Err(format!("CI topology has {} error(s)", errors.len()))
    }
}

#[derive(Clone, Debug, Default, Serialize)]
struct Selectors {
    sets: BTreeSet<String>,
    areas: BTreeSet<String>,
    packages: BTreeSet<String>,
    targets: BTreeSet<String>,
}

impl Selectors {
    fn is_empty(&self) -> bool {
        self.sets.is_empty()
            && self.areas.is_empty()
            && self.packages.is_empty()
            && self.targets.is_empty()
    }
}

fn parse_selectors(
    args: impl Iterator<Item = String>,
) -> Result<(Selectors, Option<PathBuf>), String> {
    parse_selectors_with_environment(args, environment_value)
}

fn parse_selectors_with_environment(
    mut args: impl Iterator<Item = String>,
    mut environment: impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<(Selectors, Option<PathBuf>), String> {
    let mut selectors = Selectors::default();
    let mut receipt = None;
    while let Some(argument) = args.next() {
        let (flag, inline) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(flag, value)| {
                (flag, Some(value))
            });
        let value = match inline {
            Some(value) => value.to_owned(),
            None => args
                .next()
                .ok_or_else(|| format!("{flag} requires a value"))?,
        };
        match flag {
            "--sets" => extend_csv(&mut selectors.sets, &value, flag)?,
            "--areas" => extend_csv(&mut selectors.areas, &value, flag)?,
            "--packages" => extend_csv(&mut selectors.packages, &value, flag)?,
            "--targets" => extend_csv(&mut selectors.targets, &value, flag)?,
            "--receipt" => {
                if receipt.replace(PathBuf::from(value)).is_some() {
                    return Err("--receipt may be supplied only once".to_owned());
                }
            }
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    extend_environment_selector(
        &mut selectors.sets,
        "SOLSTONE_CI_SETS",
        "--sets",
        &mut environment,
    )?;
    extend_environment_selector(
        &mut selectors.areas,
        "SOLSTONE_CI_AREAS",
        "--areas",
        &mut environment,
    )?;
    extend_environment_selector(
        &mut selectors.packages,
        "SOLSTONE_CI_PACKAGES",
        "--packages",
        &mut environment,
    )?;
    extend_environment_selector(
        &mut selectors.targets,
        "SOLSTONE_CI_TARGETS",
        "--targets",
        &mut environment,
    )?;
    if let Some(value) = environment("SOLSTONE_CI_RECEIPT")?
        && receipt.replace(PathBuf::from(value)).is_some()
    {
        return Err("receipt path was supplied by both argument and environment".to_owned());
    }
    Ok((selectors, receipt))
}

fn extend_csv(destination: &mut BTreeSet<String>, value: &str, flag: &str) -> Result<(), String> {
    let values = value
        .split(|character: char| character == ',' || character.is_ascii_whitespace())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Err(format!("{flag} must name at least one value"));
    }
    destination.extend(values.into_iter().map(ToOwned::to_owned));
    Ok(())
}

fn extend_environment_selector(
    destination: &mut BTreeSet<String>,
    variable: &str,
    flag: &str,
    environment: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<(), String> {
    if let Some(value) = environment(variable)? {
        extend_csv(destination, &value, flag)?;
    }
    Ok(())
}

fn environment_value(variable: &str) -> Result<Option<String>, String> {
    let Some(value) = env::var_os(variable) else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| format!("{variable} is not valid UTF-8"))?;
    Ok((!value.trim().is_empty()).then_some(value))
}

#[derive(Clone, Debug)]
enum ItemKind {
    Suite {
        package: String,
        target: String,
        required_features: Vec<String>,
        runtime: String,
    },
    Package {
        package: String,
        runtime: String,
    },
    Leg {
        make_target: String,
    },
}

#[derive(Clone, Debug)]
struct PlanItem {
    id: String,
    set: String,
    areas: Vec<String>,
    packages: Vec<String>,
    platforms: Vec<String>,
    prerequisites: Vec<String>,
    timeout: String,
    serial_group: Option<String>,
    default_full: bool,
    kind: ItemKind,
}

impl From<&Suite> for PlanItem {
    fn from(suite: &Suite) -> Self {
        Self {
            id: suite.id.clone(),
            set: suite.set.clone(),
            areas: suite.areas.clone(),
            packages: vec![suite.package.clone()],
            platforms: suite.platforms.clone(),
            prerequisites: suite.prerequisites.clone(),
            timeout: suite.timeout.clone(),
            serial_group: suite.serial_group.clone(),
            default_full: suite.default_full,
            kind: ItemKind::Suite {
                package: suite.package.clone(),
                target: suite.target.clone(),
                required_features: suite.required_features.clone(),
                runtime: suite.runtime.clone(),
            },
        }
    }
}

impl From<&Leg> for PlanItem {
    fn from(leg: &Leg) -> Self {
        Self {
            id: leg.id.clone(),
            set: leg.set.clone(),
            areas: leg.areas.clone(),
            packages: leg.packages.clone(),
            platforms: leg.platforms.clone(),
            prerequisites: leg.prerequisites.clone(),
            timeout: leg.timeout.clone(),
            serial_group: leg.serial_group.clone(),
            default_full: leg.default_full,
            kind: ItemKind::Leg {
                make_target: leg.make_target.clone(),
            },
        }
    }
}

impl From<&PackageSuite> for PlanItem {
    fn from(package_suite: &PackageSuite) -> Self {
        Self {
            id: package_suite.id.clone(),
            set: package_suite.set.clone(),
            areas: package_suite.areas.clone(),
            packages: vec![package_suite.package.clone()],
            platforms: package_suite.platforms.clone(),
            prerequisites: package_suite.prerequisites.clone(),
            timeout: package_suite.timeout.clone(),
            serial_group: package_suite.serial_group.clone(),
            default_full: package_suite.default_full,
            kind: ItemKind::Package {
                package: package_suite.package.clone(),
                runtime: package_suite.runtime.clone(),
            },
        }
    }
}

fn all_items(registry: &Registry) -> Vec<PlanItem> {
    registry
        .legs
        .iter()
        .map(PlanItem::from)
        .chain(registry.package_suites.iter().map(PlanItem::from))
        .chain(registry.suites.iter().map(PlanItem::from))
        .collect()
}

fn select(registry: &Registry, selectors: &Selectors) -> Result<Vec<PlanItem>, String> {
    let items = all_items(registry);
    reject_unknown_selectors(registry, &items, selectors)?;
    let selected = items
        .into_iter()
        .filter(|item| {
            if selectors.is_empty() {
                return item.default_full;
            }
            matches_dimension(&selectors.sets, std::slice::from_ref(&item.set))
                && matches_dimension(&selectors.areas, &item.areas)
                && matches_dimension(&selectors.packages, &item.packages)
                && (selectors.targets.is_empty()
                    || selectors.targets.contains(&item.id)
                    || match &item.kind {
                        ItemKind::Suite { target, .. } => selectors.targets.contains(target),
                        ItemKind::Leg { .. } | ItemKind::Package { .. } => false,
                    })
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err("selectors matched zero CI entries".to_owned());
    }
    Ok(selected)
}

fn matches_dimension(selectors: &BTreeSet<String>, values: &[String]) -> bool {
    selectors.is_empty() || values.iter().any(|value| selectors.contains(value))
}

fn reject_unknown_selectors(
    registry: &Registry,
    items: &[PlanItem],
    selectors: &Selectors,
) -> Result<(), String> {
    let known_sets = registry.sets.iter().cloned().collect::<BTreeSet<_>>();
    let known_areas = registry.areas.iter().cloned().collect::<BTreeSet<_>>();
    let known_packages = items
        .iter()
        .flat_map(|item| item.packages.iter().cloned())
        .collect::<BTreeSet<_>>();
    let mut known_targets = items
        .iter()
        .map(|item| item.id.clone())
        .collect::<BTreeSet<_>>();
    known_targets.extend(items.iter().filter_map(|item| match &item.kind {
        ItemKind::Suite { target, .. } => Some(target.clone()),
        ItemKind::Leg { .. } | ItemKind::Package { .. } => None,
    }));
    let mut errors = Vec::new();
    collect_unknown("set", &selectors.sets, &known_sets, &mut errors);
    collect_unknown("area", &selectors.areas, &known_areas, &mut errors);
    collect_unknown("package", &selectors.packages, &known_packages, &mut errors);
    collect_unknown("target", &selectors.targets, &known_targets, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn collect_unknown(
    kind: &str,
    requested: &BTreeSet<String>,
    known: &BTreeSet<String>,
    errors: &mut Vec<String>,
) {
    for value in requested.difference(known) {
        errors.push(format!("unknown {kind} selector {value:?}"));
    }
}

fn print_plan(plan: &[PlanItem], selectors: &Selectors) {
    if selectors.is_empty() {
        println!("Default full CI plan ({} entries)", plan.len());
    } else {
        println!("Selected CI plan ({} entries)", plan.len());
    }
    for item in plan {
        let kind = match item.kind {
            ItemKind::Suite { .. } => "suite",
            ItemKind::Package { .. } => "package",
            ItemKind::Leg { .. } => "leg",
        };
        println!(
            "{kind}\t{}\tset={}\tareas={}\tpackages={}\tplatforms={}\tprerequisites={}\ttimeout={}\tserial={}",
            item.id,
            item.set,
            item.areas.join(","),
            item.packages.join(","),
            item.platforms.join(","),
            item.prerequisites.join(","),
            item.timeout,
            item.serial_group.as_deref().unwrap_or("-")
        );
    }
    println!(
        "Excluded from the default: differential -> reserved excluded lane, no suites registered; race -> make check-rust-race or SETS=race; live -> operator-only live validation."
    );
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum Verdict {
    Pass,
    Fail,
    Blocked,
    Skip,
    Inconclusive,
}

#[derive(Debug, Serialize)]
struct ResultRow {
    id: String,
    verdict: Verdict,
    duration_ms: u128,
    cpu_user_ms: u128,
    cpu_system_ms: u128,
    command: Vec<String>,
    log_path: Option<String>,
    detail: String,
}

#[derive(Debug, Serialize)]
struct Receipt {
    schema: &'static str,
    revision: String,
    started_unix: u64,
    selectors: Selectors,
    results: Vec<ResultRow>,
}

fn execute(
    repo: &Path,
    registry: &Registry,
    selectors: Selectors,
    plan: Vec<PlanItem>,
    receipt_path: Option<PathBuf>,
) -> Result<i32, String> {
    ensure_clean_tree(repo)?;
    let revision = command_stdout(repo, "git", &["rev-parse", "HEAD"])?;
    let started_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock: {error}"))?
        .as_secs();
    let current_platform = env::consts::OS;
    let log_root = default_log_root(repo, &revision, started_unix);
    fs::create_dir_all(&log_root)
        .map_err(|error| format!("create log directory {}: {error}", log_root.display()))?;
    let mut prerequisite_cache = BTreeMap::<String, Result<(), String>>::new();
    let mut results = Vec::new();

    for (index, item) in plan.into_iter().enumerate() {
        let command = item_command(&item);
        println!("\n==> {}", item.id);
        if !item
            .platforms
            .iter()
            .any(|platform| platform == current_platform)
        {
            println!(
                "SKIP {}: platform {current_platform} is outside {:?}",
                item.id, item.platforms
            );
            results.push(ResultRow {
                id: item.id,
                verdict: Verdict::Skip,
                duration_ms: 0,
                cpu_user_ms: 0,
                cpu_system_ms: 0,
                command,
                log_path: None,
                detail: format!("platform {current_platform} not selected"),
            });
            continue;
        }
        if item.set == "differential" {
            println!(
                "BLOCKED {}: differential suites remain in the reserved excluded lane",
                item.id
            );
            results.push(ResultRow {
                id: item.id,
                verdict: Verdict::Blocked,
                duration_ms: 0,
                cpu_user_ms: 0,
                cpu_system_ms: 0,
                command,
                log_path: None,
                detail: "reserved excluded lane: differential".to_owned(),
            });
            continue;
        }
        let mut blocked = Vec::new();
        for prerequisite in &item.prerequisites {
            let result = prerequisite_cache
                .entry(prerequisite.clone())
                .or_insert_with(|| check_prerequisite(repo, prerequisite));
            if let Err(error) = result {
                blocked.push(format!("{prerequisite}: {error}"));
            }
        }
        if !blocked.is_empty() {
            let detail = blocked.join("; ");
            println!("BLOCKED {}: {detail}", item.id);
            results.push(ResultRow {
                id: item.id,
                verdict: Verdict::Blocked,
                duration_ms: 0,
                cpu_user_ms: 0,
                cpu_system_ms: 0,
                command,
                log_path: None,
                detail,
            });
            continue;
        }

        let timeout = *registry
            .timeouts
            .get(&item.timeout)
            .ok_or_else(|| format!("unknown timeout class {}", item.timeout))?;
        let log_path = log_root.join(format!("{:03}-{}.log", index + 1, safe_id(&item.id)));
        let receipt_log_path = log_path
            .strip_prefix(repo)
            .unwrap_or(&log_path)
            .to_string_lossy()
            .replace('\\', "/");
        println!("log: {receipt_log_path}");
        let started = Instant::now();
        let outcome = run_item(repo, &item, Duration::from_secs(timeout), &log_path);
        let duration_ms = started.elapsed().as_millis();
        let (verdict, detail, cpu_user_ms, cpu_system_ms) = match outcome {
            Ok(outcome) if outcome.status.is_some_and(|status| status.success()) => (
                Verdict::Pass,
                "exit 0".to_owned(),
                outcome.cpu_user_ms,
                outcome.cpu_system_ms,
            ),
            Ok(outcome) if outcome.status.is_some() => {
                let exit = outcome
                    .status
                    .and_then(|status| status.code())
                    .unwrap_or(128);
                let classification = if item.set == "race" {
                    fs::read_to_string(&log_path)
                        .map(|log| race_log_inconclusive(&log))
                        .map_err(|error| format!("read race log {}: {error}", log_path.display()))
                } else {
                    Ok(false)
                };
                match classification {
                    Ok(true) => (
                        Verdict::Inconclusive,
                        format!("race gate reported host-contention inconclusive (exit {exit})"),
                        outcome.cpu_user_ms,
                        outcome.cpu_system_ms,
                    ),
                    Ok(false) => (
                        Verdict::Fail,
                        format!("exit {exit}"),
                        outcome.cpu_user_ms,
                        outcome.cpu_system_ms,
                    ),
                    Err(error) => (Verdict::Blocked, error, 0, 0),
                }
            }
            Ok(outcome) => (
                Verdict::Inconclusive,
                format!("timed out after {timeout}s"),
                outcome.cpu_user_ms,
                outcome.cpu_system_ms,
            ),
            Err(error) => (Verdict::Blocked, error, 0, 0),
        };
        let label = match &verdict {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Blocked => "BLOCKED",
            Verdict::Skip => "SKIP",
            Verdict::Inconclusive => "INCONCLUSIVE",
        };
        println!("{label} {} ({duration_ms} ms): {detail}", item.id);
        results.push(ResultRow {
            id: item.id,
            verdict,
            duration_ms,
            cpu_user_ms,
            cpu_system_ms,
            command,
            log_path: Some(receipt_log_path),
            detail,
        });
    }

    let receipt = Receipt {
        schema: "solstone-ci-receipt-v1",
        revision: revision.clone(),
        started_unix,
        selectors,
        results,
    };
    let path = receipt_path.unwrap_or_else(|| default_receipt_path(repo, &revision, started_unix));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create receipt directory {}: {error}", parent.display()))?;
    }
    fs::write(
        &path,
        serde_json::to_vec_pretty(&receipt).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write receipt {}: {error}", path.display()))?;
    println!("\nCI receipt: {}", path.display());

    let hard = receipt.results.iter().any(|row| {
        matches!(
            row.verdict,
            Verdict::Fail | Verdict::Blocked | Verdict::Inconclusive
        )
    });
    Ok(i32::from(hard))
}

fn item_command(item: &PlanItem) -> Vec<String> {
    match &item.kind {
        ItemKind::Leg { make_target } => vec![
            "make".to_owned(),
            "--no-print-directory".to_owned(),
            make_target.clone(),
        ],
        ItemKind::Suite {
            package,
            target,
            required_features,
            runtime,
        } => vec![
            "make".to_owned(),
            "--no-print-directory".to_owned(),
            "check-rust-registry-suite".to_owned(),
            format!("CI_PACKAGE={package}"),
            format!("CI_TARGET={target}"),
            format!("CI_FEATURES={}", required_features.join(",")),
            format!("CI_RUNTIME={runtime}"),
        ],
        ItemKind::Package { package, runtime } => vec![
            "make".to_owned(),
            "--no-print-directory".to_owned(),
            "check-rust-registry-package".to_owned(),
            format!("CI_PACKAGE={package}"),
            format!("CI_RUNTIME={runtime}"),
        ],
    }
}

struct RunOutcome {
    status: Option<ExitStatus>,
    cpu_user_ms: u128,
    cpu_system_ms: u128,
}

fn run_item(
    repo: &Path,
    item: &PlanItem,
    timeout: Duration,
    log_path: &Path,
) -> Result<RunOutcome, String> {
    let argv = item_command(item);
    let log = File::create(log_path)
        .map_err(|error| format!("create log {}: {error}", log_path.display()))?;
    let stderr_log = log
        .try_clone()
        .map_err(|error| format!("clone log {}: {error}", log_path.display()))?;
    let before = child_cpu_usage()?;
    let mut command = Command::new(&argv[0]);
    pin_ci_cargo_environment(&mut command);
    command
        .args(&argv[1..])
        .current_dir(repo)
        .env("CARGO_NET_OFFLINE", "true")
        .env("TMPDIR", "/var/tmp")
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env_remove("GNUMAKEFLAGS")
        .env_remove("MAKELEVEL")
        .env_remove("MAKEOVERRIDES")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr_log));
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("start {}: {error}", argv.join(" ")))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let (cpu_user_ms, cpu_system_ms) = child_cpu_delta(before)?;
                return Ok(RunOutcome {
                    status: Some(status),
                    cpu_user_ms,
                    cpu_system_ms,
                });
            }
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(200)),
            Ok(None) => {
                terminate_child_tree(&mut child)?;
                let (cpu_user_ms, cpu_system_ms) = child_cpu_delta(before)?;
                return Ok(RunOutcome {
                    status: None,
                    cpu_user_ms,
                    cpu_system_ms,
                });
            }
            Err(error) => return Err(format!("wait for {}: {error}", item.id)),
        }
    }
}

const CI_CARGO_ENVIRONMENT: [(&str, &str); 2] =
    [("CARGO_INCREMENTAL", "0"), ("CARGO_PROFILE_DEV_DEBUG", "0")];

fn pin_ci_cargo_environment(command: &mut Command) {
    command.envs(CI_CARGO_ENVIRONMENT);
}

fn default_log_root(repo: &Path, revision: &str, started_unix: u64) -> PathBuf {
    repo.join("target/ci-logs")
        .join(format!("{revision}-{started_unix}"))
}

fn default_receipt_path(repo: &Path, revision: &str, started_unix: u64) -> PathBuf {
    repo.join("target/ci-receipts")
        .join(format!("{revision}-{started_unix}.json"))
}

#[cfg(unix)]
fn child_cpu_usage() -> Result<(i64, i64), String> {
    use nix::sys::resource::{UsageWho, getrusage};
    use nix::sys::time::TimeValLike;

    let usage = getrusage(UsageWho::RUSAGE_CHILDREN)
        .map_err(|error| format!("read child CPU usage: {error}"))?;
    Ok((
        usage.user_time().num_microseconds(),
        usage.system_time().num_microseconds(),
    ))
}

#[cfg(not(unix))]
fn child_cpu_usage() -> Result<(i64, i64), String> {
    Ok((0, 0))
}

fn child_cpu_delta(before: (i64, i64)) -> Result<(u128, u128), String> {
    let after = child_cpu_usage()?;
    Ok((
        after.0.saturating_sub(before.0) as u128 / 1_000,
        after.1.saturating_sub(before.1) as u128 / 1_000,
    ))
}

fn terminate_child_tree(child: &mut std::process::Child) -> Result<(), String> {
    #[cfg(unix)]
    {
        use nix::errno::Errno;
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;

        let group = Pid::from_raw(child.id() as i32);
        let signal_group = |signal| match killpg(group, signal) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(format!("signal timed-out process group {group}: {error}")),
        };
        let group_exists = || match killpg(group, None::<Signal>) {
            Ok(()) | Err(Errno::EPERM) => Ok(true),
            Err(Errno::ESRCH) => Ok(false),
            Err(error) => Err(format!("inspect timed-out process group {group}: {error}")),
        };
        let wait_for_group =
            |child: &mut std::process::Child, deadline: Duration| -> Result<bool, String> {
                let started = Instant::now();
                let mut reaped = false;
                while started.elapsed() < deadline {
                    if !reaped {
                        reaped = child
                            .try_wait()
                            .map_err(|error| format!("reap timed-out child: {error}"))?
                            .is_some();
                    }
                    if reaped && !group_exists()? {
                        return Ok(true);
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                Ok(false)
            };

        signal_group(Some(Signal::SIGTERM))?;
        if wait_for_group(child, Duration::from_secs(2))? {
            return Ok(());
        }
        signal_group(Some(Signal::SIGKILL))?;
        if wait_for_group(child, Duration::from_secs(2))? {
            Ok(())
        } else {
            Err(format!(
                "timed-out process group {group} survived SIGKILL or could not be reaped"
            ))
        }
    }
    #[cfg(not(unix))]
    {
        child
            .kill()
            .map_err(|error| format!("kill timed-out child: {error}"))?;
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            if child
                .try_wait()
                .map_err(|error| format!("reap timed-out child: {error}"))?
                .is_some()
            {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        Err("timed-out child could not be reaped after kill".to_owned())
    }
}

fn ensure_clean_tree(repo: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "--untracked-files=normal"])
        .current_dir(repo)
        .output()
        .map_err(|error| format!("inspect worktree before CI run: {error}"))?;
    if !output.status.success() {
        return Err("git status failed before CI run".to_owned());
    }
    let dirty =
        String::from_utf8(output.stdout).map_err(|error| format!("decode git status: {error}"))?;
    if dirty.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "CI execution requires a clean final tree so receipt SHA provenance is exact:\n{}",
            dirty.trim_end()
        ))
    }
}

fn safe_id(id: &str) -> String {
    id.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn race_log_inconclusive(log: &str) -> bool {
    log.lines()
        .rev()
        .find(|line| line.starts_with("check-rust-race: "))
        .is_some_and(|summary| summary.starts_with("check-rust-race: INCONCLUSIVE ("))
}

fn check_prerequisite(repo: &Path, prerequisite: &str) -> Result<(), String> {
    let success = match prerequisite {
        "cargo-cache" => command_status(
            repo,
            "cargo",
            &[
                "fetch",
                "--manifest-path",
                "core/Cargo.toml",
                "--locked",
                "--offline",
            ],
        )?,
        "cargo-deny" => command_status(repo, "cargo", &["deny", "--version"])?,
        "msrv" => command_status(repo, "rustup", &["run", "1.95.0", "rustc", "--version"])?,
        "ffmpeg-toolchain" => {
            if env::consts::OS != "linux" {
                true
            } else {
                ["/usr/lib/clang", "/usr/lib64/clang"].iter().any(|root| {
                    fs::read_dir(root)
                        .ok()
                        .into_iter()
                        .flatten()
                        .filter_map(Result::ok)
                        .any(|entry| entry.path().join("include/limits.h").is_file())
                })
            }
        }
        "onnx-runtime" => command_status(
            repo,
            "make",
            &["--no-print-directory", "check-rust-onnx-ready"],
        )?,
        "pdf-runtime" => command_status(
            repo,
            "make",
            &["--no-print-directory", "check-rust-pdf-ready"],
        )?,
        "apple-sdk" => {
            env::consts::OS == "macos"
                && command_status(repo, "xcrun", &["--sdk", "iphoneos", "--show-sdk-path"])?
                && command_status(
                    repo,
                    "rustup",
                    &[
                        "run",
                        "1.97.1",
                        "rustc",
                        "--target",
                        "aarch64-apple-ios",
                        "--print",
                        "target-libdir",
                    ],
                )?
        }
        "host-tools" => host_tools_ready(repo)?,
        other => return Err(format!("runner does not know prerequisite {other}")),
    };
    if success {
        Ok(())
    } else {
        Err("not prepared; run the corresponding make ci-full-prep target".to_owned())
    }
}

fn host_tools_ready(repo: &Path) -> Result<bool, String> {
    let mut required = vec![
        "make",
        "/usr/bin/make",
        "/usr/bin/uname",
        "/bin/pwd",
        "/bin/sh",
        "chmod",
        "find",
        "mkdir",
        "mktemp",
        "/usr/bin/printf",
        "rm",
        "uname",
    ];
    match env::consts::OS {
        "linux" => required.push("/usr/bin/sha256sum"),
        "macos" => required.extend(["/usr/bin/shasum", "cc"]),
        _ => {}
    }

    let mut missing = Vec::new();
    for tool in required {
        let available = command_status(
            repo,
            "/bin/sh",
            &[
                "-c",
                "command -v \"$1\" >/dev/null 2>&1",
                "solstone-ci-host-tool",
                tool,
            ],
        )?;
        if !available {
            missing.push(tool);
        }
    }
    if missing.is_empty() {
        Ok(true)
    } else {
        Err(format!(
            "missing required host tools: {}",
            missing.join(", ")
        ))
    }
}

fn command_status(repo: &Path, program: &str, args: &[&str]) -> Result<bool, String> {
    Command::new(program)
        .args(args)
        .current_dir(repo)
        .env("CARGO_NET_OFFLINE", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .map_err(|error| format!("start {program}: {error}"))
}

fn command_stdout(repo: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|error| format!("start {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!("{program} {} failed", args.join(" ")));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|error| format!("decode {program} output: {error}"))
}

fn repo_root() -> Result<PathBuf, String> {
    let mut current = env::current_dir().map_err(|error| format!("current directory: {error}"))?;
    loop {
        if current.join("Makefile").is_file() && current.join("core/Cargo.toml").is_file() {
            return Ok(current);
        }
        if !current.pop() {
            return Err("run from a solstone-journal checkout".to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        Registry {
            version: 1,
            sets: vec![
                "component".to_owned(),
                "differential".to_owned(),
                "race".to_owned(),
                "live".to_owned(),
                "policy".to_owned(),
            ],
            areas: vec!["stats".to_owned(), "support".to_owned()],
            platforms: vec!["linux".to_owned()],
            prerequisites: vec![],
            serial_groups: vec![],
            runtimes: vec!["none".to_owned()],
            timeouts: BTreeMap::from([("quick".to_owned(), 30)]),
            suites: vec![
                Suite {
                    id: "stats::api".to_owned(),
                    package: "stats".to_owned(),
                    target: "api".to_owned(),
                    set: "component".to_owned(),
                    areas: vec!["stats".to_owned()],
                    platforms: vec!["linux".to_owned()],
                    prerequisites: vec![],
                    timeout: "quick".to_owned(),
                    serial_group: None,
                    default_full: true,
                    required_features: vec![],
                    runtime: "none".to_owned(),
                },
                Suite {
                    id: "support::api".to_owned(),
                    package: "support".to_owned(),
                    target: "api".to_owned(),
                    set: "component".to_owned(),
                    areas: vec!["support".to_owned()],
                    platforms: vec!["linux".to_owned()],
                    prerequisites: vec![],
                    timeout: "quick".to_owned(),
                    serial_group: None,
                    default_full: false,
                    required_features: vec![],
                    runtime: "none".to_owned(),
                },
            ],
            package_suites: vec![
                PackageSuite {
                    id: "package::stats".to_owned(),
                    package: "stats".to_owned(),
                    set: "component".to_owned(),
                    areas: vec!["stats".to_owned()],
                    platforms: vec!["linux".to_owned()],
                    prerequisites: vec![],
                    timeout: "quick".to_owned(),
                    serial_group: None,
                    default_full: false,
                    runtime: "none".to_owned(),
                },
                PackageSuite {
                    id: "package::support".to_owned(),
                    package: "support".to_owned(),
                    set: "component".to_owned(),
                    areas: vec!["support".to_owned()],
                    platforms: vec!["linux".to_owned()],
                    prerequisites: vec![],
                    timeout: "quick".to_owned(),
                    serial_group: None,
                    default_full: false,
                    runtime: "none".to_owned(),
                },
            ],
            legs: vec![Leg {
                id: "fmt".to_owned(),
                make_target: "check-rust-fmt".to_owned(),
                set: "policy".to_owned(),
                areas: vec!["stats".to_owned(), "support".to_owned()],
                packages: vec!["workspace".to_owned()],
                platforms: vec!["linux".to_owned()],
                prerequisites: vec![],
                timeout: "quick".to_owned(),
                serial_group: None,
                default_full: true,
            }],
        }
    }

    #[test]
    fn spawned_ci_commands_pin_disk_lean_cargo_settings() {
        assert_eq!(
            CI_CARGO_ENVIRONMENT,
            [("CARGO_INCREMENTAL", "0"), ("CARGO_PROFILE_DEV_DEBUG", "0")]
        );
    }

    #[test]
    fn default_ci_evidence_lives_outside_the_cargo_target() {
        let repo = Path::new("/checkout");
        assert_eq!(
            default_log_root(repo, "abc123", 42),
            PathBuf::from("/checkout/target/ci-logs/abc123-42")
        );
        assert_eq!(
            default_receipt_path(repo, "abc123", 42),
            PathBuf::from("/checkout/target/ci-receipts/abc123-42.json")
        );
    }

    #[test]
    fn explicit_receipt_override_remains_authoritative() {
        let (_, receipt) = parse_selectors_with_environment(
            ["--receipt".to_owned(), "custom/receipt.json".to_owned()].into_iter(),
            |_| Ok(None),
        )
        .expect("explicit receipt parses");
        assert_eq!(receipt, Some(PathBuf::from("custom/receipt.json")));
    }

    #[test]
    fn receipt_conflict_between_argument_and_environment_is_rejected() {
        let error = parse_selectors_with_environment(
            ["--receipt".to_owned(), "custom/receipt.json".to_owned()].into_iter(),
            |variable| {
                Ok((variable == "SOLSTONE_CI_RECEIPT")
                    .then_some("environment/receipt.json".to_owned()))
            },
        )
        .expect_err("duplicate receipt source must fail");
        assert_eq!(
            error,
            "receipt path was supplied by both argument and environment"
        );
    }

    #[test]
    fn selectors_union_within_and_intersect_across_dimensions() {
        let selectors = Selectors {
            areas: BTreeSet::from(["stats".to_owned(), "support".to_owned()]),
            packages: BTreeSet::from(["stats".to_owned()]),
            ..Selectors::default()
        };
        let selected = select(&registry(), &selectors).expect("selection");
        assert_eq!(
            selected
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["package::stats", "stats::api"]
        );
    }

    #[test]
    fn selectors_reject_unknown_and_zero_match_instead_of_passing_green() {
        let unknown = Selectors {
            areas: BTreeSet::from(["mystery".to_owned()]),
            ..Selectors::default()
        };
        assert!(
            select(&registry(), &unknown)
                .unwrap_err()
                .contains("unknown area")
        );
        let zero = Selectors {
            areas: BTreeSet::from(["stats".to_owned()]),
            packages: BTreeSet::from(["support".to_owned()]),
            ..Selectors::default()
        };
        assert_eq!(
            select(&registry(), &zero).unwrap_err(),
            "selectors matched zero CI entries"
        );
    }

    #[test]
    fn default_plan_excludes_non_default_entries() {
        let selected = select(&registry(), &Selectors::default()).expect("default selection");
        assert_eq!(
            selected
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["fmt", "stats::api"]
        );
    }

    #[test]
    fn suite_and_package_commands_delegate_to_runtime_aware_make_wrappers() {
        let registry = registry();
        let suite = PlanItem::from(&registry.suites[0]);
        assert_eq!(
            &item_command(&suite)[1..],
            [
                "--no-print-directory",
                "check-rust-registry-suite",
                "CI_PACKAGE=stats",
                "CI_TARGET=api",
                "CI_FEATURES=",
                "CI_RUNTIME=none",
            ]
        );
        let package = PlanItem::from(&registry.package_suites[0]);
        assert_eq!(
            &item_command(&package)[1..],
            [
                "--no-print-directory",
                "check-rust-registry-package",
                "CI_PACKAGE=stats",
                "CI_RUNTIME=none",
            ]
        );
    }

    #[test]
    fn selectors_accept_comma_or_whitespace_separators() {
        let mut values = BTreeSet::new();
        extend_csv(&mut values, "component,native policy", "--sets").expect("selector values");
        assert_eq!(
            values,
            ["component", "native", "policy"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
    }

    #[test]
    fn race_summary_preserves_inconclusive_distinct_from_failure() {
        assert!(race_log_inconclusive(
            "check-rust-race: run 1 INCONCLUSIVE: load\ncheck-rust-race: INCONCLUSIVE (1 of 5 run(s); 0 hard failures)\n"
        ));
        assert!(!race_log_inconclusive(
            "check-rust-race: run 1 INCONCLUSIVE: load\ncheck-rust-race: FAILED (1 hard-failed run(s); 1 inconclusive)\n"
        ));
    }
}
