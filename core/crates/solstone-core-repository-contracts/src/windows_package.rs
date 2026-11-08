// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cargo-owned package launches with independent binary captures.
//!
//! Cargo builds each batch and hands every library test binary to our runner,
//! which records the exact launch (executable, arguments, directory and the
//! environment Cargo gave it) instead of running it. The recorded launches then
//! run side by side, never two of one serial group at once, as the sharded
//! Linux gate runs package suites. Inside a binary, tests stay on one thread.

use crate::windows_suite::{
    PackageTestResult, PlanStatus, ProcessTree, SourceBinding, WindowsDisposition, WindowsPlan,
    WindowsPlanItem, WindowsSuiteEvidenceRow, cargo_package_names, is_package_suite,
    package_test_artifacts, spawn_process_tree, wait_bounded, windows_package_batches,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PackageContext {
    directory: PathBuf,
    timeout_seconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CaptureConfig {
    invocation_id: String,
    commit: String,
    cargo_lock_sha256: String,
    features: Vec<String>,
    packages: BTreeMap<String, PackageContext>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildCapture {
    config_sha256: String,
    executable: PathBuf,
    argv: Vec<String>,
    directory: PathBuf,
    exit_code: i32,
    timed_out: bool,
    supervision_error: Option<String>,
    elapsed_seconds: f64,
}

/// What Cargo would have run for one library binary.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Launch {
    config_sha256: String,
    executable: PathBuf,
    argv: Vec<String>,
    directory: PathBuf,
    env: BTreeMap<String, String>,
}

struct CommandCapture {
    exit_code: i32,
    timed_out: bool,
    supervision_error: Option<String>,
    stdout: String,
    logs: Vec<String>,
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(|e| format!("write {}: {e}", path.display()))
}

fn emit_log(path: &Path) -> Result<(), String> {
    let contents = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    println!("WINDOWS_PACKAGE_LOG_BEGIN {}", path.display());
    print!("{contents}");
    println!("\nWINDOWS_PACKAGE_LOG_END {}", path.display());
    std::io::stdout().flush().map_err(|e| e.to_string())
}

fn run_command(
    directory: &Path,
    argv: &[String],
    prefix: &Path,
    timeout_seconds: u64,
    env: Option<&BTreeMap<String, String>>,
    emit: bool,
) -> Result<CommandCapture, String> {
    let stdout_path = prefix.with_extension("stdout.log");
    let stderr_path = prefix.with_extension("stderr.log");
    let command_path = prefix.with_extension("command.json");
    write_new(
        &command_path,
        &serde_json::to_vec(argv).map_err(|e| e.to_string())?,
    )?;
    let log_file = |path: &Path| {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| format!("create {}: {e}", path.display()))
    };
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(log_file(&stdout_path)?)
        .stderr(log_file(&stderr_path)?);
    if let Some(env) = env {
        command.env_clear().envs(env);
    }
    let mut tree =
        spawn_process_tree(&mut command).map_err(|e| format!("spawn {}: {e}", argv[0]))?;
    let result = wait_bounded(
        &mut tree,
        Duration::from_secs(timeout_seconds),
        Instant::now,
        std::thread::sleep,
    );
    // A successful parent may leave descendants writing to the same logs.
    // Finish bounded tree cleanup before inspecting or publishing any evidence.
    let cleanup = tree.kill_tree();
    drop(tree);
    let timed_out = result.as_ref().is_err_and(|e| e == "timed out");
    let (exit_code, mut supervision_error) = match result {
        Ok(status) => (status.code().unwrap_or(1), None),
        Err(error) => (-1, Some(error)),
    };
    if let Err(error) = cleanup {
        supervision_error = Some(format!("process-tree cleanup failed: {error}"));
    }
    let stdout = fs::read_to_string(&stdout_path).map_err(|e| e.to_string())?;
    let logs = [stdout_path, stderr_path, command_path];
    if emit {
        println!(
            "WINDOWS_PACKAGE_COMMAND {}",
            serde_json::to_string(argv).map_err(|e| e.to_string())?
        );
        for path in &logs {
            emit_log(path)?;
        }
    }
    Ok(CommandCapture {
        exit_code,
        timed_out,
        supervision_error,
        stdout,
        logs: logs.iter().map(|p| p.display().to_string()).collect(),
    })
}

fn executable_key(executable: &Path) -> Result<String, String> {
    let text = executable
        .to_str()
        .ok_or("non-UTF-8 Cargo executable path")?;
    Ok(format!("{:x}", Sha256::digest(text.as_bytes())))
}

/// Private entry used only by Cargo's invocation-local runner configuration.
/// It records the launch and runs nothing; `run_member` runs it later.
pub fn capture_child(args: Vec<String>) -> Result<i32, String> {
    if args.len() < 2 {
        return Err("capture requires config and executable".to_owned());
    }
    let config_path = Path::new(&args[0]);
    let bytes = fs::read(config_path).map_err(|e| e.to_string())?;
    let config: CaptureConfig = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let directory = fs::canonicalize(env::current_dir().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let matches = config
        .packages
        .values()
        .filter(|p| p.directory == directory)
        .count();
    if matches != 1 {
        return Err("Cargo supplied unknown or ambiguous package directory".to_owned());
    }
    let executable = PathBuf::from(&args[1]);
    if !executable.is_absolute() {
        return Err("Cargo executable must be absolute".to_owned());
    }
    let prefix = config_path
        .parent()
        .ok_or("config lacks parent")?
        .join(executable_key(&executable)?);
    let claim = prefix.with_extension("claim");
    if let Err(error) = write_new(&claim, b"claimed") {
        let _ = write_new(&prefix.with_extension("duplicate"), b"duplicate invocation");
        return Err(error);
    }
    let env = env::vars_os()
        .map(|(k, v)| match (k.into_string(), v.into_string()) {
            (Ok(k), Ok(v)) => Ok((k, v)),
            _ => Err("non-UTF-8 environment from Cargo".to_owned()),
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let launch = Launch {
        config_sha256: format!("{:x}", Sha256::digest(&bytes)),
        executable,
        argv: args[1..].to_vec(),
        directory,
        env,
    };
    write_new(
        &prefix.with_extension("launch.json"),
        &serde_json::to_vec(&launch).map_err(|e| e.to_string())?,
    )?;
    Ok(0)
}

/// Run one recorded launch and publish its capture as `result.json`.
fn run_member(directory: &Path, executable: &Path, timeout_seconds: u64) -> Result<(), String> {
    let prefix = directory.join(executable_key(executable)?);
    let launch: Launch = serde_json::from_slice(
        &fs::read(prefix.with_extension("launch.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("incomplete library launch: {e}"))?;
    if launch.executable != executable || launch.argv.is_empty() {
        return Err("library launch names another binary".to_owned());
    }
    let started = Instant::now();
    let capture = run_command(
        &launch.directory,
        &launch.argv,
        &prefix,
        timeout_seconds,
        Some(&launch.env),
        false,
    )?;
    let report = ChildCapture {
        config_sha256: launch.config_sha256,
        executable: launch.executable,
        argv: launch.argv,
        directory: launch.directory,
        exit_code: capture.exit_code,
        timed_out: capture.timed_out,
        supervision_error: capture.supervision_error,
        elapsed_seconds: started.elapsed().as_secs_f64(),
    };
    let temporary = prefix.with_extension("result.tmp");
    write_new(
        &temporary,
        &serde_json::to_vec(&report).map_err(|e| e.to_string())?,
    )?;
    fs::rename(&temporary, prefix.with_extension("result.json")).map_err(|e| e.to_string())
}

/// Run `jobs` at most `width` at a time, never two of one serial group together.
/// Results come back in job order; a panicking job is an error, not a hang.
fn run_admitted<J: Sync>(
    jobs: &[J],
    width: usize,
    group: impl Fn(&J) -> Option<&str>,
    run: impl Fn(&J) -> Result<(), String> + Sync,
) -> Vec<Result<(), String>> {
    let width = width.max(1);
    let mut results: Vec<Option<Result<(), String>>> = jobs.iter().map(|_| None).collect();
    std::thread::scope(|scope| {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut pending: VecDeque<usize> = (0..jobs.len()).collect();
        let mut running: Vec<usize> = Vec::new();
        loop {
            let mut position = 0;
            while running.len() < width && position < pending.len() {
                let index = pending[position];
                let busy = group(&jobs[index])
                    .is_some_and(|g| running.iter().any(|&r| group(&jobs[r]) == Some(g)));
                if busy {
                    position += 1;
                    continue;
                }
                pending.remove(position);
                running.push(index);
                let (sender, job, run) = (sender.clone(), &jobs[index], &run);
                scope.spawn(move || {
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(job)))
                            .unwrap_or_else(|_| {
                                Err("package member supervisor panicked".to_owned())
                            });
                    let _ = sender.send((index, result));
                });
            }
            if running.is_empty() {
                break;
            }
            let (index, result) = receiver.recv().expect("a running member reports");
            running.retain(|&r| r != index);
            results[index] = Some(result);
        }
    });
    results.into_iter().map(Option::unwrap).collect()
}

/// A quarter of the host's logical processors, at most four. Library binaries
/// run one test at a time, but they share the disk: on a 16-way host, eight
/// side by side ran each member about 2.5 times slower than alone and pushed
/// the longest suites toward their timeouts.
fn member_width() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get() / 4)
        .clamp(1, 4)
}

fn library_result(stdout: &str, exit_code: i32) -> Result<PackageTestResult, String> {
    let running = Regex::new(r"^running (\d+) tests?$").unwrap();
    let result =
        Regex::new(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;")
            .unwrap();
    let total = stdout
        .lines()
        .find_map(|line| running.captures(line))
        .ok_or("library output lacks outer harness start")?[1]
        .parse::<usize>()
        .map_err(|e| e.to_string())?;
    // Nested harness diagnostics belong to this binary, not another package.
    // Only the final outer aggregate plus the real process exit can qualify it.
    let summary = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .ok_or("empty library output")?;
    let capture = result
        .captures(summary)
        .ok_or("library output lacks final outer aggregate")?;
    let passed = capture[2].parse::<usize>().map_err(|e| e.to_string())?;
    let failed = capture[3].parse::<usize>().map_err(|e| e.to_string())?;
    let ignored = capture[4].parse::<usize>().map_err(|e| e.to_string())?;
    if Some(total)
        != passed
            .checked_add(failed)
            .and_then(|n| n.checked_add(ignored))
        || (capture.get(1).unwrap().as_str() == "ok") != (failed == 0)
        || (exit_code == 0) != (failed == 0)
    {
        return Err("library totals disagree with actual exit".to_owned());
    }
    Ok(PackageTestResult {
        executed: passed + failed,
        passed: exit_code == 0,
    })
}

fn verify_member(
    config_path: &Path,
    config: &CaptureConfig,
    package: &str,
    executable: &Path,
) -> Result<ChildCapture, String> {
    let prefix = config_path
        .parent()
        .ok_or("config lacks parent")?
        .join(executable_key(executable)?);
    let result_path = prefix.with_extension("result.json");
    let report: ChildCapture =
        serde_json::from_slice(&fs::read(&result_path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("incomplete library capture: {e}"))?;
    verify_capture(config, &report, package, executable)?;
    Ok(report)
}

fn verify_capture(
    config: &CaptureConfig,
    report: &ChildCapture,
    package: &str,
    executable: &Path,
) -> Result<(), String> {
    let expected_hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(config).map_err(|e| e.to_string())?)
    );
    let expected_argv = [
        executable.to_str().ok_or("non-UTF-8 executable")?,
        "--test-threads=1",
    ];
    if report.config_sha256 != expected_hash
        || report.executable != executable
        || report.argv != expected_argv
        || report.directory != config.packages[package].directory
    {
        return Err("library capture identity/source/argv/directory mismatch".to_owned());
    }
    Ok(())
}

fn verify_inventory(
    artifacts: &BTreeMap<String, PathBuf>,
    filenames: &[String],
) -> Result<(), String> {
    let keys: BTreeSet<_> = artifacts
        .values()
        .map(|p| executable_key(p))
        .collect::<Result<_, _>>()?;
    if keys.len() != artifacts.len() {
        return Err("multiple packages claimed the same executable".to_owned());
    }
    let foreign = |name: &str, extension: &str| {
        name.strip_suffix(extension)
            .is_some_and(|key| !keys.contains(key))
    };
    if filenames.iter().any(|name| {
        name.ends_with(".duplicate")
            || foreign(name, ".result.json")
            || foreign(name, ".launch.json")
    }) {
        return Err("duplicate or foreign library capture".to_owned());
    }
    Ok(())
}

/// Independent member evidence plus the enclosing Cargo status.
pub struct PackageRun {
    pub rows: Vec<WindowsSuiteEvidenceRow>,
    pub aggregate_failed: bool,
}

impl PackageRun {
    pub fn failed(&self) -> bool {
        self.aggregate_failed
            || self
                .rows
                .iter()
                .any(|r| r.disposition != WindowsDisposition::Passed)
    }
}

pub fn execute_windows_packages(
    repo: &Path,
    plan: &WindowsPlan,
    target_dir: &Path,
    invocation_id: &str,
    binding: &SourceBinding,
    runner: &Path,
) -> Result<PackageRun, String> {
    let batches = windows_package_batches(plan);
    let mut run = PackageRun {
        rows: Vec::new(),
        aggregate_failed: false,
    };
    if batches.is_empty() {
        return Ok(run);
    }
    let metadata_argv = [
        "cargo",
        "metadata",
        "--manifest-path",
        "core/Cargo.toml",
        "--locked",
        "--no-deps",
        "--format-version=1",
    ]
    .map(str::to_owned);
    let metadata = run_command(
        repo,
        &metadata_argv,
        &target_dir.join(format!("journal-win-package-metadata-{invocation_id}")),
        batches.iter().map(|b| b.timeout_seconds).max().unwrap(),
        None,
        true,
    )?;
    if metadata.exit_code != 0 || metadata.supervision_error.is_some() {
        return Err("Cargo package metadata failed".to_owned());
    }
    let names = cargo_package_names(&metadata.stdout)?;
    let metadata_value: serde_json::Value =
        serde_json::from_str(&metadata.stdout).map_err(|e| e.to_string())?;
    for (index, batch) in batches.iter().enumerate() {
        let directory = target_dir.join(format!(
            "journal-win-package-captures-{index}-{invocation_id}"
        ));
        fs::create_dir(&directory)
            .map_err(|e| format!("create exclusive capture directory: {e}"))?;
        let mut config = CaptureConfig {
            invocation_id: invocation_id.to_owned(),
            commit: binding.commit.clone(),
            cargo_lock_sha256: binding.cargo_lock_sha256.clone(),
            features: batch.features.clone(),
            packages: BTreeMap::new(),
        };
        for package in &batch.packages {
            let member = metadata_value["packages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["name"] == *package)
                .ok_or("missing selected package metadata")?;
            let manifest = Path::new(
                member["manifest_path"]
                    .as_str()
                    .ok_or("metadata lacks manifest")?,
            );
            let item = plan
                .items
                .iter()
                .find(|i| {
                    is_package_suite(i) && &i.package == package && i.status == PlanStatus::Ready
                })
                .unwrap();
            config.packages.insert(
                package.clone(),
                PackageContext {
                    directory: fs::canonicalize(
                        manifest.parent().ok_or("manifest lacks directory")?,
                    )
                    .map_err(|e| e.to_string())?,
                    timeout_seconds: item.timeout_seconds,
                },
            );
        }
        let config_path = directory.join("config.json");
        write_new(
            &config_path,
            &serde_json::to_vec(&config).map_err(|e| e.to_string())?,
        )?;
        let runner_args = [
            runner.to_str().ok_or("non-UTF-8 runner")?,
            "windows-package-child",
            config_path.to_str().ok_or("non-UTF-8 config")?,
        ];
        let key = if cfg!(windows) {
            "target.x86_64-pc-windows-msvc.runner"
        } else {
            "target.'cfg(all())'.runner"
        };
        let runner_config = format!(
            "{key}={}",
            serde_json::to_string(&runner_args).map_err(|e| e.to_string())?
        );
        let mut argv = [
            "cargo",
            "test",
            "--manifest-path",
            "core/Cargo.toml",
            "--locked",
            "--lib",
            "--no-fail-fast",
            "--message-format=json",
            "--color=never",
            "--config",
        ]
        .map(str::to_owned)
        .to_vec();
        argv.push(runner_config);
        for package in &batch.packages {
            argv.extend(["-p".to_owned(), package.clone()]);
        }
        if !batch.features.is_empty() {
            argv.push("--features".to_owned());
            argv.push(
                batch
                    .packages
                    .iter()
                    .flat_map(|p| batch.features.iter().map(move |f| format!("{p}/{f}")))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        argv.extend(["--".to_owned(), "--test-threads=1".to_owned()]);
        let build_started = Instant::now();
        let capture = run_command(
            repo,
            &argv,
            &target_dir.join(format!("journal-win-package-batch-{index}-{invocation_id}")),
            batch.timeout_seconds,
            None,
            true,
        );
        let build_seconds = build_started.elapsed().as_secs_f64();
        run.aggregate_failed |= capture
            .as_ref()
            .map_or(true, |c| c.exit_code != 0 || c.supervision_error.is_some());
        let artifacts = capture
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|c| package_test_artifacts(&c.stdout, &names, batch));
        // Every member Cargo launched runs now, side by side. A member whose
        // launch or run fails has no result and is judged failed below.
        let mut members: Vec<(&WindowsPlanItem, &PathBuf)> = artifacts
            .as_ref()
            .map(|artifacts| {
                batch
                    .packages
                    .iter()
                    .filter_map(|package| {
                        let item = plan.items.iter().find(|i| {
                            is_package_suite(i)
                                && &i.package == package
                                && i.status == PlanStatus::Ready
                        })?;
                        Some((item, artifacts.get(package)?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // The suites declared longest start first, so the batch is not left
        // waiting on one of them admitted late. The sort is stable.
        members.sort_by_key(|(item, _)| std::cmp::Reverse(item.timeout_seconds));
        let width = member_width();
        let run_started = Instant::now();
        let outcomes = run_admitted(
            &members,
            width,
            |(item, _)| item.serial_group.as_deref(),
            |(item, executable)| run_member(&directory, executable, item.timeout_seconds),
        );
        println!(
            "WINDOWS_PACKAGE_BATCH index={index} members={} width={width} build_seconds={build_seconds:.1} run_seconds={:.1}",
            members.len(),
            run_started.elapsed().as_secs_f64()
        );
        for ((item, _), outcome) in members.iter().zip(&outcomes) {
            if let Err(error) = outcome {
                eprintln!("suite {}: {error}", item.id);
            }
        }
        let inventory = artifacts
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|artifacts| {
                let filenames = fs::read_dir(&directory)
                    .map_err(|e| e.to_string())?
                    .map(|entry| {
                        entry
                            .map(|e| e.file_name().to_string_lossy().into_owned())
                            .map_err(|e| e.to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                verify_inventory(artifacts, &filenames)
            });
        emit_log(&config_path)?;
        for package in &batch.packages {
            let item = plan
                .items
                .iter()
                .find(|i| is_package_suite(i) && &i.package == package)
                .unwrap();
            let mut row = WindowsSuiteEvidenceRow {
                suite_id: item.id.clone(),
                disposition: WindowsDisposition::Failed,
                commands: vec![argv.join(" ")],
                exit_codes: vec![capture.as_ref().map_or(1, |c| c.exit_code)],
                log_paths: metadata.logs.clone(),
                features: item.required_features.clone(),
                executed_counts: vec![0],
                commit: binding.commit.clone(),
                cargo_lock_sha256: binding.cargo_lock_sha256.clone(),
                invocation_id: invocation_id.to_owned(),
                nonce: None,
                owner_account: None,
                owner_sid: None,
                elevated: None,
                rail_matched: None,
            };
            row.log_paths.push(config_path.display().to_string());
            if let Ok(capture) = &capture {
                row.log_paths.extend(capture.logs.iter().cloned());
            }
            let member_artifact = artifacts.as_ref().ok().and_then(|a| a.get(package));
            if let Some(executable) = member_artifact {
                let prefix = directory.join(executable_key(executable)?);
                // The launch carries Cargo's whole environment; it stays on the host.
                for extension in ["stdout.log", "stderr.log", "command.json", "result.json"] {
                    let path = prefix.with_extension(extension);
                    if path.is_file() {
                        emit_log(&path)?;
                        row.log_paths.push(path.display().to_string());
                    }
                }
            }
            let result = inventory.as_ref().map_err(Clone::clone).and_then(|()| {
                let executable = member_artifact.ok_or("missing library artifact")?;
                verify_member(&config_path, &config, package, executable)
            });
            match result {
                Ok(report) => {
                    println!(
                        "WINDOWS_PACKAGE_MEMBER {} seconds={:.1}",
                        item.id, report.elapsed_seconds
                    );
                    row.commands.push(report.argv.join(" "));
                    row.exit_codes.push(report.exit_code);
                    if report.timed_out {
                        row.disposition = WindowsDisposition::TimedOut;
                    } else if report.supervision_error.is_none() {
                        let prefix = directory.join(executable_key(&report.executable)?);
                        let stdout = fs::read_to_string(prefix.with_extension("stdout.log"))
                            .map_err(|e| e.to_string())?;
                        match library_result(&stdout, report.exit_code) {
                            Ok(result) => {
                                row.executed_counts = vec![result.executed];
                                row.disposition = if result.executed == 0 {
                                    WindowsDisposition::Unexecuted
                                } else if result.passed {
                                    WindowsDisposition::Passed
                                } else {
                                    WindowsDisposition::Failed
                                };
                            }
                            Err(error) => eprintln!("suite {}: {error}", item.id),
                        }
                    }
                }
                Err(error) => {
                    eprintln!("suite {}: {error}", item.id);
                }
            }
            println!(
                "WIN_SUITE {} disposition={}",
                item.id,
                match row.disposition {
                    WindowsDisposition::Passed => "passed",
                    WindowsDisposition::Unexecuted => "unexecuted",
                    WindowsDisposition::TimedOut => "timed-out",
                    _ => "failed",
                }
            );
            run.rows.push(row);
        }
    }
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_inventory_refuses_duplicates_and_foreign_results_without_erasing_missing_member_identity()
     {
        let artifacts = BTreeMap::from([
            ("first".into(), PathBuf::from("first-binary")),
            ("second".into(), PathBuf::from("second-binary")),
        ]);
        let first = format!(
            "{}.result.json",
            executable_key(&artifacts["first"]).unwrap()
        );
        // A missing second report is judged per-member, rather than losing the first.
        verify_inventory(&artifacts, std::slice::from_ref(&first)).unwrap();
        assert!(
            verify_inventory(&artifacts, &[first.clone(), "unknown.result.json".into()]).is_err()
        );
        assert!(verify_inventory(&artifacts, &[first, "first.duplicate".into()]).is_err());
        let same_binary = BTreeMap::from([
            ("first".into(), PathBuf::from("binary")),
            ("second".into(), PathBuf::from("binary")),
        ]);
        assert!(verify_inventory(&same_binary, &[]).is_err());
    }

    #[test]
    fn inventory_refuses_a_launch_cargo_did_not_make_for_this_batch() {
        let artifacts = BTreeMap::from([("first".into(), PathBuf::from("first-binary"))]);
        let key = executable_key(&artifacts["first"]).unwrap();
        verify_inventory(&artifacts, &[format!("{key}.launch.json")]).unwrap();
        assert!(verify_inventory(&artifacts, &["unknown.launch.json".into()]).is_err());
    }

    #[test]
    fn members_run_side_by_side_within_the_width_and_never_two_of_one_serial_group() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Condvar, Mutex};
        let jobs: Vec<(usize, Option<&str>)> = vec![
            (0, Some("local-services")),
            (1, Some("local-services")),
            (2, None),
            (3, None),
            (4, Some("host-contention")),
            (5, None),
            (6, Some("local-services")),
        ];
        let live = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let groups = Mutex::new(Vec::<&str>::new());
        // Jobs 2 and 3 are admitted in the first wave; each waits for the other.
        let met = (Mutex::new(0usize), Condvar::new());
        let results = run_admitted(
            &jobs,
            3,
            |(_, group)| *group,
            |(index, group)| {
                peak.fetch_max(live.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                if let Some(group) = group {
                    let mut held = groups.lock().unwrap();
                    if held.contains(group) {
                        return Err(format!("{group} ran twice at once"));
                    }
                    held.push(group);
                }
                let outcome = match index {
                    2 | 3 => {
                        let mut arrived = met.0.lock().unwrap();
                        *arrived += 1;
                        met.1.notify_all();
                        let (arrived, _) = met
                            .1
                            .wait_timeout_while(arrived, Duration::from_secs(30), |n| *n < 2)
                            .unwrap();
                        if *arrived < 2 {
                            Err("ungrouped members did not run together".into())
                        } else if *index == 3 {
                            Err("red".into())
                        } else {
                            Ok(())
                        }
                    }
                    _ => Ok(()),
                };
                if let Some(group) = group {
                    groups.lock().unwrap().retain(|g| g != group);
                }
                live.fetch_sub(1, Ordering::SeqCst);
                if *index == 5 {
                    panic!("member supervisor fault");
                }
                outcome
            },
        );
        assert!(peak.load(Ordering::SeqCst) <= 3);
        assert_eq!(results.len(), jobs.len());
        assert_eq!(results[3], Err("red".to_owned()));
        assert!(results[5].is_err());
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            5,
            "{results:?}"
        );
        assert!(run_admitted(&Vec::<()>::new(), 0, |_| None, |_| Ok(())).is_empty());
    }

    fn captured_blocks() -> Vec<String> {
        // Real Cargo capture: empty, failed and successful binaries, in Cargo's order.
        include_str!("fixtures/windows-package/batch.stdout")
            .split("\nrunning ")
            .skip(1)
            .map(|s| format!("running {s}"))
            .collect()
    }

    #[test]
    fn per_binary_parser_keeps_real_failure_nested_diagnostics_and_zero_execution_distinct() {
        let blocks = captured_blocks();
        assert_eq!(library_result(&blocks[0], 0).unwrap().executed, 0);
        let failed = library_result(&blocks[1], 101).unwrap();
        assert_eq!(
            failed,
            PackageTestResult {
                executed: 2,
                passed: false
            }
        );
        assert_eq!(
            library_result(&blocks[2], 0).unwrap(),
            PackageTestResult {
                executed: 1,
                passed: true
            }
        );
        let nested = blocks[1].replace(
            "---- fails stdout ----",
            &format!("---- fails stdout ----\n{}", blocks[2]),
        );
        assert_eq!(library_result(&nested, 101).unwrap(), failed);
        assert!(library_result(&nested, 0).is_err());
        assert!(library_result(&blocks[2], 101).is_err());
        assert!(library_result("", 0).is_err());
        assert!(library_result("running 1 test\n", 0).is_err());
        assert!(
            library_result(&blocks[2].replace("running 1 test", "running 2 tests"), 0).is_err()
        );
        assert!(library_result(&format!("{}\nunfinished", blocks[2]), 0).is_err());
        let actual_windows = include_str!("fixtures/windows-package/windows-failure.stdout");
        assert!(!library_result(actual_windows, 101).unwrap().passed);
    }

    #[test]
    fn capture_provenance_refuses_another_binary_directory_argv_invocation_source_or_feature_set() {
        let executable = Path::new("binary");
        let config = CaptureConfig {
            invocation_id: "inv-one".into(),
            commit: "a".repeat(40),
            cargo_lock_sha256: "b".repeat(64),
            features: vec!["full-tests".into()],
            packages: BTreeMap::from([(
                "package".into(),
                PackageContext {
                    directory: PathBuf::from("package-dir"),
                    timeout_seconds: 10,
                },
            )]),
        };
        let report = ChildCapture {
            config_sha256: format!("{:x}", Sha256::digest(serde_json::to_vec(&config).unwrap())),
            executable: executable.into(),
            argv: vec!["binary".into(), "--test-threads=1".into()],
            directory: PathBuf::from("package-dir"),
            exit_code: 0,
            timed_out: false,
            supervision_error: None,
            elapsed_seconds: 0.0,
        };
        verify_capture(&config, &report, "package", executable).unwrap();
        let mut bad = report.clone();
        bad.executable = "another".into();
        assert!(verify_capture(&config, &bad, "package", executable).is_err());
        let mut bad = report.clone();
        bad.argv.push("--ignored".into());
        assert!(verify_capture(&config, &bad, "package", executable).is_err());
        let mut bad = report.clone();
        bad.directory = "another".into();
        assert!(verify_capture(&config, &bad, "package", executable).is_err());
        for field in ["invocation", "commit", "lock", "features"] {
            let mut bad = config.clone();
            match field {
                "invocation" => bad.invocation_id.push('2'),
                "commit" => bad.commit.push('2'),
                "lock" => bad.cargo_lock_sha256.push('2'),
                _ => bad.features.clear(),
            }
            assert!(
                verify_capture(&bad, &report, "package", executable).is_err(),
                "{field}"
            );
        }
        assert!(serde_json::from_slice::<ChildCapture>(b"{").is_err());
    }
}
