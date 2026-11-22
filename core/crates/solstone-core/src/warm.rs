// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Install-wide native sibling-binary warmup.
//!
//! This is a substitution, not a sharpening, of Python warm. Python warm proves that
//! `numpy`, `PIL`, `cv2`, `av`, `soundfile`, and `onnxruntime` load (and adds `mlx`
//! and `mlx_vlm` on Darwin arm64); native warm proves that native binaries start.
//! Those are disjoint sets. Both verbs exist today, but the cut wave must retain this
//! as an explicit obligation. In particular, macOS Gatekeeper commonly evaluates the
//! notarized `.so` payloads under site-packages, which this native-binary census does
//! not touch.

// Do not extend solstone-core-local::install::readiness::probe_binary: it hard-codes
// --version, reduces outcomes to status.code(), discards stderr needed to name loader
// libraries, and its wire shape is consumed by
// solstone.think.providers.local_install.probe_binary_runnable and
// solstone.think.providers.state.local_status_dict. Warm is a separate install-wide
// sibling-binary census with per-row argv and closed stdin.

use solstone_core_cli_copy::{Diagnostic, Value as CopyValue, display_for, render_for};
use std::env;
use std::io;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

use serde_json::{Value, json};
use solstone_core_system_health::{BoundedStderr, classify_loader_failure, read_bounded_stderr};

const SCHEMA: &str = "solstone-warm-v1";
const CARGO_CACHE_TAG: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InventoryRow {
    pub(crate) binary_name: &'static str,
    pub(crate) distribution: &'static str,
    pub(crate) crate_name: &'static str,
    pub(crate) argv: &'static [&'static str],
    pub(crate) expected: &'static str,
    pub(crate) applicability: PlatformApplicability,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlatformApplicability {
    All,
    Linux,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Host {
    Linux,
    MacOs,
}

impl Host {
    fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::MacOs
        }
    }
}

impl PlatformApplicability {
    fn applies_to(self, host: Host) -> bool {
        match self {
            Self::All => true,
            Self::Linux => host == Host::Linux,
        }
    }

    fn platform_name(self) -> &'static str {
        match self {
            Self::All => "all supported platforms",
            Self::Linux => "Linux",
        }
    }
}

// parakeet-helper is intentionally excluded: it is a macOS Swift helper resolved
// by solstone-core-transcribe (sibling binary, then bin/ and lib/parakeet-helper/),
// not a warm inventory leaf.
const INVENTORY: [InventoryRow; 10] = [
    InventoryRow {
        binary_name: "solstone-core",
        distribution: "solstone-core",
        crate_name: "solstone-core",
        argv: &["--version"],
        expected: "--version exits 0",
        applicability: PlatformApplicability::All,
    },
    InventoryRow {
        binary_name: "solstone-core-depict",
        distribution: "solstone-core-depict",
        crate_name: "solstone-core-depict",
        argv: &[],
        expected: "empty invocation exits 1 with solstone-depict-error-v1 malformed-request",
        applicability: PlatformApplicability::All,
    },
    InventoryRow {
        binary_name: "solstone-core-describe",
        distribution: "solstone-core-describe",
        crate_name: "solstone-core-describe",
        argv: &["--version"],
        expected: "--version exits 0",
        applicability: PlatformApplicability::Linux,
    },
    InventoryRow {
        binary_name: "solstone-core-journal",
        distribution: "solstone-core-journal",
        crate_name: "solstone-core-journal-bin",
        argv: &["--version"],
        expected: "--version exits 0",
        applicability: PlatformApplicability::All,
    },
    InventoryRow {
        binary_name: "solstone-core-pdf",
        distribution: "solstone-core-pdf",
        crate_name: "solstone-core-pdf",
        argv: &["--version"],
        expected: "--version exits 0",
        applicability: PlatformApplicability::Linux,
    },
    InventoryRow {
        binary_name: "solstone-retention",
        distribution: "solstone-core-retention",
        crate_name: "solstone-core-retention-cli",
        argv: &["--help"],
        expected: "--help exits 0",
        applicability: PlatformApplicability::All,
    },
    InventoryRow {
        binary_name: "solstone-core-sol",
        distribution: "solstone-core-sol",
        crate_name: "solstone-core-sol-bin",
        argv: &["--version"],
        expected: "--version exits 0",
        applicability: PlatformApplicability::All,
    },
    InventoryRow {
        binary_name: "solstone-core-speakers-analyze",
        distribution: "solstone-core-speakers-analyze",
        crate_name: "solstone-core-speakers-analyze",
        argv: &[],
        expected: "closed-stdin invocation exits 64 with solstone-speaker-analyze-error-v1 malformed-request",
        applicability: PlatformApplicability::All,
    },
    InventoryRow {
        binary_name: "solstone-core-vad-analyze",
        distribution: "solstone-core-vad-analyze",
        crate_name: "solstone-core-vad-analyze",
        argv: &[],
        expected: "closed-stdin invocation exits 64 with solstone-vad-error-v1 malformed-request",
        applicability: PlatformApplicability::All,
    },
    InventoryRow {
        binary_name: "solstone-core-vulkan-probe",
        distribution: "solstone-core-vulkan-probe",
        crate_name: "solstone-core-vulkan-probe",
        argv: &["--version"],
        expected: "--version exits 0 without loading Vulkan",
        applicability: PlatformApplicability::Linux,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Classification {
    Ran,
    Missing,
    CannotLoad,
    Unexercised,
    NotApplicable,
}

impl Classification {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ran => "ran",
            Self::Missing => "missing",
            Self::CannotLoad => "cannot-load",
            Self::Unexercised => "unexercised",
            Self::NotApplicable => "not-applicable",
        }
    }

    fn is_failure(self) -> bool {
        matches!(self, Self::Missing | Self::CannotLoad)
    }
}

#[derive(Debug)]
pub(crate) struct WarmRecord {
    pub(crate) row: InventoryRow,
    pub(crate) classification: Classification,
    pub(crate) reason_code: &'static str,
    pub(crate) unresolved_library: Option<String>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) signal: Option<i32>,
    pub(crate) stderr: String,
    pub(crate) stderr_truncated: bool,
    pub(crate) error: Option<String>,
    pub(crate) actual: String,
    line: Diagnostic,
    pub(crate) repair: &'static str,
}

#[derive(Debug)]
pub(crate) struct WarmReport {
    pub(crate) records: Vec<WarmRecord>,
}

impl WarmReport {
    pub(crate) fn failed(&self) -> bool {
        self.records
            .iter()
            .any(|record| record.classification.is_failure())
    }

    pub(crate) fn as_json(&self) -> Value {
        let ran = self
            .records
            .iter()
            .filter(|record| record.classification == Classification::Ran)
            .count();
        let missing = self
            .records
            .iter()
            .filter(|record| record.classification == Classification::Missing)
            .count();
        let cannot_load = self
            .records
            .iter()
            .filter(|record| record.classification == Classification::CannotLoad)
            .count();
        let unexercised = self
            .records
            .iter()
            .filter(|record| record.classification == Classification::Unexercised)
            .count();
        let not_applicable = self
            .records
            .iter()
            .filter(|record| record.classification == Classification::NotApplicable)
            .count();

        json!({
            "schema": SCHEMA,
            "ok": !self.failed(),
            "summary": {
                "ran": ran,
                "missing": missing,
                "cannot-load": cannot_load,
                "unexercised": unexercised,
                "not-applicable": not_applicable,
            },
            "binaries": self.records.iter().map(WarmRecord::as_json).collect::<Vec<_>>(),
        })
    }
}

impl WarmRecord {
    fn as_json(&self) -> Value {
        // A per-leaf install command would be unsafe: installers own tool environments, not
        // these individually shipped leaves. Keep this null for machine consumers.
        json!({
            "binary-name": self.row.binary_name,
            "distribution": self.row.distribution,
            "crate": self.row.crate_name,
            "argv": self.row.argv,
            "classification": self.classification.as_str(),
            "reason-code": self.reason_code,
            "unresolved-library": self.unresolved_library,
            "exit-code": self.exit_code,
            "signal": self.signal,
            "stderr": self.stderr,
            "stderr-truncated": self.stderr_truncated,
            "subject": self.row.binary_name,
            "error": self.error,
            "expected": self.row.expected,
            "actual": self.actual,
            "repair-command": Value::Null,
            "repair": self.repair,
        })
    }
}

pub(crate) fn inventory_rows() -> &'static [InventoryRow] {
    &INVENTORY
}

/// Run warm output and return whether a shipped binary is missing or cannot load.
pub(crate) fn run(json_output: bool) -> bool {
    let host = Host::current();
    let report = match env::current_exe() {
        Ok(executable) => collect_for_executable(&executable, inventory_rows(), host),
        Err(error) => unavailable_report(error, host),
    };
    if json_output {
        println!("{}", report.as_json());
    } else {
        let locale =
            solstone_core_cli_copy::terminal_locale().unwrap_or(solstone_core_cli_copy::ENGLISH);
        print_human(locale, &report);
    }
    report.failed()
}

pub(crate) fn collect_for_executable(
    executable: &Path,
    inventory: &[InventoryRow],
    host: Host,
) -> WarmReport {
    let sibling_dir = executable.parent().unwrap_or_else(|| Path::new("."));
    let cargo_tree = verified_cargo_target_tree(executable);
    WarmReport {
        records: inventory
            .iter()
            .copied()
            .map(|row| {
                if row.applicability.applies_to(host) {
                    probe(sibling_dir.join(row.binary_name), row, cargo_tree)
                } else {
                    not_applicable_record(row)
                }
            })
            .collect(),
    }
}

fn unavailable_report(error: io::Error, host: Host) -> WarmReport {
    WarmReport {
        records: inventory_rows()
            .iter()
            .copied()
            .map(|row| {
                if row.applicability.applies_to(host) {
                    unavailable_record(
                        row,
                        format!("cannot resolve current executable: {error}"),
                        Diagnostic::new(
                            "journal.cli.host.warm.row.executable",
                            [("binary", &row.binary_name), ("error", &(error))],
                        ),
                    )
                } else {
                    not_applicable_record(row)
                }
            })
            .collect(),
    }
}

fn not_applicable_record(row: InventoryRow) -> WarmRecord {
    WarmRecord {
        row,
        classification: Classification::NotApplicable,
        reason_code: "platform-not-applicable",
        unresolved_library: None,
        exit_code: None,
        signal: None,
        stderr: String::new(),
        stderr_truncated: false,
        error: None,
        line: Diagnostic::new(
            "journal.cli.host.warm.row.platform",
            [("binary", &row.binary_name)],
        ),
        actual: "Linux-only binary is not applicable on this platform".to_owned(),
        repair: "No repair needed on this platform.",
    }
}

fn probe(path: PathBuf, row: InventoryRow, cargo_tree: bool) -> WarmRecord {
    let output = run_probe(&path, row.argv);

    match output {
        Err(error) if error.kind() == io::ErrorKind::NotFound && cargo_tree => WarmRecord {
            row,
            classification: Classification::Unexercised,
            reason_code: "development-sibling-not-built",
            unresolved_library: None,
            exit_code: None,
            signal: None,
            stderr: String::new(),
            stderr_truncated: false,
            error: Some("development sibling was not built".to_owned()),
            line: Diagnostic::new(
                "journal.cli.host.warm.row.development",
                [("binary", &row.binary_name), ("path", &(path.display()))],
            ),
            actual: format!(
                "{} is absent beside the development executable",
                path.display()
            ),
            repair: "Build this crate in a configured development environment.",
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => WarmRecord {
            row,
            classification: Classification::Missing,
            reason_code: "binary-missing",
            unresolved_library: None,
            exit_code: None,
            signal: None,
            stderr: String::new(),
            stderr_truncated: false,
            error: Some("shipped binary is absent".to_owned()),
            line: Diagnostic::new(
                "journal.cli.host.warm.row.missing",
                [("binary", &row.binary_name), ("path", &(path.display()))],
            ),
            actual: format!("{} is absent beside the running executable", path.display()),
            repair: "Reinstall or upgrade the owning host tool with the installer family that created this environment.",
        },
        Err(error) => unavailable_record(
            row,
            format!("failed to spawn {}: {error}", path.display()),
            Diagnostic::new(
                "journal.cli.host.warm.row.spawn",
                [
                    ("binary", &row.binary_name),
                    ("path", &(path.display())),
                    ("error", &(error)),
                ],
            ),
        ),
        Ok((status, stderr)) => classify_output(row, status, stderr),
    }
}

fn run_probe(path: &Path, argv: &[&str]) -> io::Result<(ExitStatus, BoundedStderr)> {
    let mut child = Command::new(path)
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = child
        .stderr
        .take()
        .expect("piped child stderr must be available");
    let reader = thread::spawn(move || read_bounded_stderr(stderr));
    let status = child.wait()?;
    let stderr = reader
        .join()
        .map_err(|_| io::Error::other("stderr reader panicked"))??;
    Ok((status, stderr))
}

fn unavailable_record(row: InventoryRow, actual: String, line: Diagnostic) -> WarmRecord {
    WarmRecord {
        row,
        classification: Classification::CannotLoad,
        reason_code: "spawn-failed",
        unresolved_library: None,
        exit_code: None,
        signal: None,
        stderr: String::new(),
        stderr_truncated: false,
        error: Some("binary could not be spawned".to_owned()),
        actual,
        line,
        repair: "Repair the reported host execution failure, then reinstall the owning host tool if needed.",
    }
}

fn classify_output(row: InventoryRow, status: ExitStatus, stderr: BoundedStderr) -> WarmRecord {
    let exit_code = status.code();
    let signal = exit_signal(&status);
    let stderr_truncated = stderr.truncated;
    let loader_library = stderr.loader_library;
    let stderr = String::from_utf8_lossy(&stderr.bytes).into_owned();
    if let Some(library) = classify_loader_failure(exit_code, signal, loader_library.as_deref()) {
        let actual = format!("shared library {library} could not be loaded");
        return WarmRecord {
            row,
            classification: Classification::CannotLoad,
            reason_code: "loader-library-missing",
            line: Diagnostic::new(
                "journal.cli.host.warm.row.library",
                [("binary", &row.binary_name), ("library", &(library))],
            ),
            unresolved_library: Some(library),
            exit_code,
            signal,
            stderr,
            stderr_truncated,
            error: Some("dynamic loader could not resolve a shared library".to_owned()),
            actual,
            repair: "Repair the reported host execution failure, then reinstall the owning host tool if needed.",
        };
    }
    if exit_code.is_none() {
        let actual = match signal {
            Some(signal) => format!("terminated by signal {signal}"),
            None => "terminated without an exit code".to_owned(),
        };
        return WarmRecord {
            row,
            classification: Classification::CannotLoad,
            reason_code: "terminated-by-signal",
            unresolved_library: None,
            exit_code,
            signal,
            stderr,
            stderr_truncated,
            error: Some("binary did not return from its own code".to_owned()),
            line: match signal {
                Some(signal) => Diagnostic::new(
                    "journal.cli.host.warm.row.signal",
                    [("binary", &row.binary_name), ("signal", &(signal))],
                ),
                None => Diagnostic::new(
                    "journal.cli.host.warm.row.no.code",
                    [("binary", &row.binary_name)],
                ),
            },
            actual,
            repair: "Repair the reported host execution failure, then reinstall the owning host tool if needed.",
        };
    }
    WarmRecord {
        row,
        classification: Classification::Ran,
        reason_code: "reached-own-code",
        unresolved_library: None,
        exit_code,
        signal,
        stderr,
        stderr_truncated,
        error: None,
        actual: format!("returned exit {}", exit_code.expect("numeric exit code")),
        line: Diagnostic::new(
            "journal.cli.host.warm.row.exit",
            [
                ("binary", &row.binary_name),
                ("code", &(exit_code.expect("numeric exit code"))),
            ],
        ),
        repair: "No repair needed.",
    }
}

#[cfg(unix)]
fn exit_signal(status: &ExitStatus) -> Option<i32> {
    status.signal()
}

#[cfg(not(unix))]
fn exit_signal(_status: &ExitStatus) -> Option<i32> {
    None
}

fn verified_cargo_target_tree(executable: &Path) -> bool {
    executable.parent().is_some_and(|directory| {
        directory.ancestors().take(32).any(|ancestor| {
            std::fs::read(ancestor.join("CACHEDIR.TAG"))
                .is_ok_and(|contents| contents.starts_with(CARGO_CACHE_TAG))
        })
    })
}

fn print_human(locale: &str, report: &WarmReport) {
    println!(
        "{}",
        display_for(
            locale,
            "journal.cli.host.warm.inventory",
            [("count", &(report.records.len()))]
        )
    );
    println!(
        "{}",
        render_for(locale, "journal.cli.host.warm.linux.static", &[])
    );
    println!(
        "{}",
        render_for(locale, "journal.cli.host.warm.linux.dynamic", &[])
    );
    println!("{}", render_for(locale, "journal.cli.host.warm.mac", &[]));
    let mut restricted_rows = Vec::new();
    for record in &report.records {
        let row = record.row;
        if row.applicability == PlatformApplicability::All {
            continue;
        }
        let description = format!(
            "{} ({})",
            row.binary_name,
            row.applicability.platform_name()
        );
        if !restricted_rows.contains(&description) {
            restricted_rows.push(description);
        }
    }
    let restricted_rows = restricted_rows.join(", ");
    println!(
        "{}",
        display_for(
            locale,
            "journal.cli.host.warm.platforms",
            [("rows", &(restricted_rows))]
        )
    );
    println!(
        "{}",
        render_for(locale, "journal.cli.host.warm.development", &[])
    );
    for record in &report.records {
        println!("{}", record.line.render(locale));
        if let Some(library) = &record.unresolved_library {
            println!(
                "{}",
                display_for(
                    locale,
                    "journal.cli.host.warm.unresolved",
                    [("library", &(library))]
                )
            );
        }
        let repair = match record.classification {
            Classification::Ran => continue,
            Classification::NotApplicable => {
                render_for(locale, "journal.cli.host.warm.repair.platform", &[])
            }
            Classification::CannotLoad => {
                render_for(locale, "journal.cli.host.warm.repair.execution", &[])
            }
            Classification::Unexercised => {
                render_for(locale, "journal.cli.host.warm.repair.development", &[])
            }
            Classification::Missing => {
                render_for(locale, "journal.cli.host.warm.repair.missing", &[])
            }
        };
        println!("{repair}");
    }
    let count = |classification| {
        report
            .records
            .iter()
            .filter(|record| record.classification == classification)
            .count()
    };
    let gaps = count(Classification::Unexercised);
    println!(
        "{}",
        render_for(
            locale,
            "journal.cli.host.warm.summary",
            &[
                (
                    "ran",
                    CopyValue::String(&(count(Classification::Ran)).to_string())
                ),
                (
                    "missing",
                    CopyValue::String(&(count(Classification::Missing)).to_string())
                ),
                (
                    "cannot_load",
                    CopyValue::String(&(count(Classification::CannotLoad)).to_string())
                ),
                ("gaps", CopyValue::String(&(gaps).to_string())),
                (
                    "not_applicable",
                    CopyValue::String(&(count(Classification::NotApplicable)).to_string())
                ),
                (
                    "quantity",
                    CopyValue::Cardinal((gaps).try_into().unwrap_or(i64::MAX))
                )
            ]
        )
    );
}
