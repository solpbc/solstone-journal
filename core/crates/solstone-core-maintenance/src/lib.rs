// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native registry and command surface for recurring journal maintenance.

mod bodies;
mod parser;
pub mod registry;
pub mod schedule_sync;
pub mod timezone;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Utc};
use registry::{RoutineDescriptor, routines};
use solstone_core_backup_runtime::{
    BackupServices, Clock, NativeJournalMaintenance, SystemToolRunner, UreqHttpTransport,
};
use solstone_core_generate::{GenerateRequest, GenerateResponse, OneShotClient};

pub use parser::USAGE;

/// Captured command output for the aggregate journal dispatcher.
#[derive(PartialEq, Eq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl std::fmt::Debug for CliRun {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CliRun")
            .field("stdout", &"<redacted>")
            .field("stderr", &"<redacted>")
            .field("exit_code", &self.exit_code)
            .finish()
    }
}

/// Injectable dependencies for parser and routine-dispatch tests.
#[derive(Debug, Clone, Copy)]
pub struct MaintenanceServices<'a> {
    pub routines: &'a [RoutineDescriptor],
}

/// Injectable time and owner-timezone dependencies for health routines.
pub struct HealthServices<'a> {
    pub now: DateTime<Utc>,
    pub host_timezone: &'a dyn timezone::HostTimezoneSource,
}

/// Injectable generation dependencies for timeline rollup routines.
pub struct TimelineServices<'a> {
    pub now: DateTime<Utc>,
    pub host_timezone: &'a dyn timezone::HostTimezoneSource,
    pub picker: &'a dyn RollupPicker,
    pub model_resolver: &'a dyn GenerateModelResolver,
}

/// One model selection request for a timeline rollup.
pub trait RollupPicker: Sync {
    fn pick(&self, request: &GenerateRequest) -> Result<String, String>;
}

/// Resolve the configured generate-lane model without making a generation request.
pub trait GenerateModelResolver: Sync {
    fn resolve(
        &self,
        config: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, String>;
}

impl<'a> MaintenanceServices<'a> {
    pub const fn new(routines: &'a [RoutineDescriptor]) -> Self {
        Self { routines }
    }
}

/// Run the production maintenance command parser.
pub fn run_cli(args: &[String], journal: &Path) -> CliRun {
    let runner = SystemToolRunner;
    let http = UreqHttpTransport;
    let clock = ProductionClock;
    let restore_hooks = NativeJournalMaintenance;
    let host_timezone = timezone::ProductionHostTimezoneSource;
    let rollup_picker = ProductionRollupPicker;
    let model_resolver = ProductionGenerateModelResolver;
    let now = Utc::now();
    let backup_services = BackupServices {
        runner: &runner,
        http: &http,
        clock: &clock,
        restic_path: Path::new("restic"),
        rclone_path: None,
        version: env!("CARGO_PKG_VERSION"),
        journal_maintenance: &restore_hooks,
    };
    run_cli_with_all_services(
        args,
        journal,
        &MaintenanceServices::new(routines()),
        Some(&backup_services),
        Some(&HealthServices {
            now,
            host_timezone: &host_timezone,
        }),
        Some(&TimelineServices {
            now,
            host_timezone: &host_timezone,
            picker: &rollup_picker,
            model_resolver: &model_resolver,
        }),
    )
}

/// Run the maintenance parser with an injected registry.
pub fn run_cli_with(args: &[String], journal: &Path, services: &MaintenanceServices<'_>) -> CliRun {
    parser::run(args, journal, services, None, None, None)
}

/// Run the maintenance parser with an injected backup runtime service set.
pub fn run_cli_with_backup(
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    backup_services: &BackupServices<'_>,
) -> CliRun {
    parser::run(args, journal, services, Some(backup_services), None, None)
}

/// Run the maintenance parser with injected backup and health routine services.
pub fn run_cli_with_services(
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    backup_services: Option<&BackupServices<'_>>,
    health_services: Option<&HealthServices<'_>>,
) -> CliRun {
    parser::run(
        args,
        journal,
        services,
        backup_services,
        health_services,
        None,
    )
}

/// Run the maintenance parser with injected timeline services.
pub fn run_cli_with_timeline(
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    timeline_services: &TimelineServices<'_>,
) -> CliRun {
    parser::run(args, journal, services, None, None, Some(timeline_services))
}

/// Run the maintenance parser with every routine service set injected.
pub fn run_cli_with_all_services(
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    backup_services: Option<&BackupServices<'_>>,
    health_services: Option<&HealthServices<'_>>,
    timeline_services: Option<&TimelineServices<'_>>,
) -> CliRun {
    parser::run(
        args,
        journal,
        services,
        backup_services,
        health_services,
        timeline_services,
    )
}

struct ProductionRollupPicker;

impl RollupPicker for ProductionRollupPicker {
    fn pick(&self, request: &GenerateRequest) -> Result<String, String> {
        let client = OneShotClient::sibling().map_err(|error| format!("{error:?}"))?;
        match client
            .execute(request)
            .map_err(|error| format!("{error:?}"))?
        {
            GenerateResponse::Generated(response) => Ok(response.text),
            GenerateResponse::Refused(response) => Err(response.detail),
        }
    }
}

struct ProductionGenerateModelResolver;

impl GenerateModelResolver for ProductionGenerateModelResolver {
    fn resolve(
        &self,
        config: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, String> {
        solstone_core_brain::resolve_generate_model(config).map_err(|error| error.to_string())
    }
}

struct ProductionClock;

impl Clock for ProductionClock {
    fn now_unix(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }

    fn iso_week(&self) -> u8 {
        Utc::now().iso_week().week() as u8
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "the composed fixture owns its temporary journal and fake services"
)]
mod composed_tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io;
    use std::path::Path;

    use chrono::{TimeZone, Utc};
    use serde_json::{Map, Value, json};
    use solstone_core_backup_runtime::hosted_runtime::HttpError;
    use solstone_core_backup_runtime::{
        BackupServices, Clock, HttpRequest, HttpResponse, HttpTransport, JournalMaintenance,
        JournalMaintenanceError, ToolOutput, ToolRequest, ToolRunner,
    };
    use solstone_core_generate::GenerateRequest;

    use super::{
        GenerateModelResolver, HealthServices, MaintenanceServices, RollupPicker, TimelineServices,
        registry, run_cli_with_all_services,
    };
    use crate::timezone::HostTimezoneSource;

    struct Runner(RefCell<VecDeque<ToolOutput>>);

    impl ToolRunner for Runner {
        fn run(&self, _: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            Ok(self.0.borrow_mut().pop_front().unwrap_or(ToolOutput {
                returncode: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            }))
        }
    }

    struct Http;

    impl HttpTransport for Http {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("the composed maintenance fixture does not use HTTP")
        }
    }

    struct FixtureClock;

    impl Clock for FixtureClock {
        fn now_unix(&self) -> i64 {
            1_772_323_200
        }

        fn iso_week(&self) -> u8 {
            9
        }
    }

    struct Hooks;

    impl JournalMaintenance for Hooks {
        fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            panic!("maintenance routines do not restore")
        }

        fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            panic!("maintenance routines do not restore")
        }
    }

    struct Host;

    impl HostTimezoneSource for Host {
        fn usable_iana_key(&self) -> Option<String> {
            Some("UTC".to_owned())
        }
    }

    struct Picker;

    impl RollupPicker for Picker {
        fn pick(&self, _: &GenerateRequest) -> Result<String, String> {
            panic!("empty timeline fixtures must not generate")
        }
    }

    struct Model;

    impl GenerateModelResolver for Model {
        fn resolve(&self, _: &Map<String, Value>) -> Result<String, String> {
            Ok("fixture-model".to_owned())
        }
    }

    #[test]
    fn all_routines_compose_through_parser_registry_and_schedule_without_python() {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(journal.path().join("config")).unwrap();
        std::fs::write(
            journal.path().join("config/journal.json"),
            json!({
                "retention": {
                    "raw_media": "keep",
                    "journal_logs": {"enabled": false}
                }
            })
            .to_string(),
        )
        .unwrap();
        let runner = Runner(RefCell::new(VecDeque::new()));
        let http = Http;
        let clock = FixtureClock;
        let hooks = Hooks;
        let host = Host;
        let picker = Picker;
        let model = Model;
        let backup = BackupServices {
            runner: &runner,
            http: &http,
            clock: &clock,
            restic_path: Path::new("restic"),
            rclone_path: None,
            version: "test",
            journal_maintenance: &hooks,
        };
        let now = Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap();
        let health = HealthServices {
            now,
            host_timezone: &host,
        };
        let timeline = TimelineServices {
            now,
            host_timezone: &host,
            picker: &picker,
            model_resolver: &model,
        };
        let services = MaintenanceServices::new(registry::routines());
        let run = |arguments: &[&str]| {
            run_cli_with_all_services(
                &arguments
                    .iter()
                    .map(|argument| (*argument).to_owned())
                    .collect::<Vec<_>>(),
                journal.path(),
                &services,
                Some(&backup),
                Some(&health),
                Some(&timeline),
            )
        };

        assert!(run(&["list"]).stdout.contains("backup:run"));
        assert_eq!(run(&["sync"]).exit_code, 0);

        for (id, witness, expected_exit) in [
            ("backup:run", "backup:", 0),
            ("backup:prune", "backup prune:", 0),
            ("backup:verify", "backup verify:", 0),
            ("backup:offload", "backup offload:", 0),
            ("health:mark-raw", "keep all original media", 0),
            ("health:prune-logs", "prune-logs: disabled", 0),
            ("timeline:rollup-day", "no segment timeline.json", 66),
            ("timeline:rollup-master", "no day-level timeline.json", 66),
        ] {
            let mut args = vec!["run", id];
            if id == "backup:offload" {
                args.push("--dry-run");
            }
            if id == "timeline:rollup-day" {
                args.push("20260301");
            }
            let result = run(&args);
            assert_eq!(result.exit_code, expected_exit, "{id}: {result:?}");
            assert!(result.stdout.contains(witness), "{id}: {result:?}");
        }
    }
}
