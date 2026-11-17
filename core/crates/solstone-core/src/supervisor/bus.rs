// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::Arc;

use serde_json::{Map, Value, json};
use solstone_core_callosum::{CallosumEnvelope, CallosumSocketServer};
use solstone_core_system::process::{OutputStream, ProcessEvent, ProcessEventSink};
use solstone_core_system::provider_runtime::{ProviderRuntimeEvent, ProviderRuntimeEventSink};
use solstone_core_system::queue::TaskQueue;
use solstone_core_system::queue::{TaskQueueEvent, TaskQueueEventSink};
use solstone_core_system::request::{ExecutionRequest, ScheduledRequest};
use solstone_core_system::schedule::ScheduleSubmissionSink;

pub(crate) fn emit(
    server: &CallosumSocketServer,
    tract: &str,
    event: &str,
    extra: Map<String, Value>,
) {
    let _ = server.broadcast(CallosumEnvelope {
        tract: tract.into(),
        event: event.into(),
        ts: None,
        extra,
    });
}

pub(crate) struct SupervisorTaskQueueSink(pub Arc<CallosumSocketServer>);
impl TaskQueueEventSink for SupervisorTaskQueueSink {
    fn emit(&self, event: TaskQueueEvent) {
        match event {
            TaskQueueEvent::QueueChanged {
                partition,
                running_reference,
                queued_depth,
                queue,
            } => emit(
                &self.0,
                "supervisor",
                "queue",
                Map::from_iter([
                    ("command".into(), json!(partition.as_str())),
                    ("running".into(), json!(running_reference)),
                    ("queued".into(), json!(queued_depth)),
                    (
                        "queue".into(),
                        json!(
                            queue
                                .iter()
                                .map(|item| &item.references)
                                .collect::<Vec<_>>()
                        ),
                    ),
                ]),
            ),
            TaskQueueEvent::Started {
                partition,
                reference,
                command,
            } => emit(
                &self.0,
                "supervisor",
                "started",
                Map::from_iter([
                    ("service".into(), json!(partition.as_str())),
                    ("ref".into(), json!(reference)),
                    ("cmd".into(), json!(command)),
                ]),
            ),
            TaskQueueEvent::Stopped {
                partition,
                reference,
                command,
                exit_code,
            } => emit(
                &self.0,
                "supervisor",
                "stopped",
                Map::from_iter([
                    ("service".into(), json!(partition.as_str())),
                    ("ref".into(), json!(reference)),
                    ("cmd".into(), json!(command)),
                    ("exit_code".into(), json!(exit_code)),
                ]),
            ),
            TaskQueueEvent::Held {
                partition,
                reference,
                command: _,
                reasons,
            } => emit(
                &self.0,
                "supervisor",
                "held",
                Map::from_iter([
                    ("service".into(), json!(partition.as_str())),
                    ("ref".into(), json!(reference)),
                    (
                        "reasons".into(),
                        json!(
                            reasons
                                .into_iter()
                                .map(|r| r.as_str().to_owned())
                                .collect::<Vec<_>>()
                        ),
                    ),
                ]),
            ),
        }
    }
}

pub(crate) struct SupervisorProcessSink {
    pub server: Arc<CallosumSocketServer>,
}

impl ProcessEventSink for SupervisorProcessSink {
    fn emit(&self, event: ProcessEvent) {
        match event {
            ProcessEvent::Spawned {
                reference,
                name,
                pid,
                ..
            } => emit(
                &self.server,
                "supervisor",
                "started",
                Map::from_iter([
                    ("service".into(), json!(name)),
                    ("ref".into(), json!(reference)),
                    ("pid".into(), json!(pid)),
                ]),
            ),
            ProcessEvent::Exited {
                reference,
                name,
                pid,
                exit_code,
                ..
            } => emit(
                &self.server,
                "supervisor",
                "stopped",
                Map::from_iter([
                    ("service".into(), json!(name)),
                    ("ref".into(), json!(reference)),
                    ("pid".into(), json!(pid)),
                    ("exit_code".into(), json!(exit_code)),
                ]),
            ),
            ProcessEvent::Line {
                reference,
                name,
                pid,
                stream,
                line,
            } => emit(
                &self.server,
                "logs",
                "line",
                Map::from_iter([
                    ("ref".into(), json!(reference)),
                    ("name".into(), json!(name)),
                    ("pid".into(), json!(pid)),
                    (
                        "stream".into(),
                        json!(match stream {
                            OutputStream::Stdout => "stdout",
                            OutputStream::Stderr => "stderr",
                        }),
                    ),
                    ("line".into(), json!(line)),
                ]),
            ),
        }
    }
}

pub(crate) struct SupervisorScheduleSink {
    pub queue: TaskQueue,
    pub server: Arc<CallosumSocketServer>,
}

/// Match a direct invocation of the retired importer command, using the
/// importer grammar to distinguish a sync from an argument containing its name.
pub(super) fn is_retired_audio_schedule(command: &[String]) -> bool {
    let Some((program, args)) = command.split_first() else {
        return false;
    };
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let lower_name = name.to_ascii_lowercase();
    let executable = lower_name.strip_suffix(".exe").unwrap_or(&lower_name);
    let (verb, args) = match (executable, args) {
        ("solstone" | "solstone-core-sol", [family, args @ ..]) if family == "journal" => {
            ("importer", strip_journal_verbose(args))
        }
        ("solstone-core-journal", args) => ("importer", strip_journal_verbose(args)),
        ("journal", args) if program == name || installed_journal_alias(program) => {
            ("importer", strip_journal_verbose(args))
        }
        ("solstone-core", args) => ("importer", args),
        ("sol", args) if program == name => ("import", args),
        _ => return false,
    };
    matches!(args, [command, args @ ..] if command == verb
        && solstone_core_import_host::cli_argv::selects_audio_sync(args))
}

fn strip_journal_verbose(args: &[String]) -> &[String] {
    match args {
        [flag, args @ ..] if matches!(flag.as_str(), "-v" | "--verbose") => args,
        _ => args,
    }
}

fn installed_journal_alias(program: &str) -> bool {
    #[cfg(windows)]
    {
        std::env::current_exe()
            .ok()
            .map(|path| path.with_file_name("journal.exe"))
            .and_then(|path| path.to_str().map(|path| path.replace('\\', "/")))
            .is_some_and(|path| path.eq_ignore_ascii_case(&program.replace('\\', "/")))
    }
    #[cfg(not(windows))]
    {
        let _ = program;
        false
    }
}

impl ScheduleSubmissionSink for SupervisorScheduleSink {
    fn submit(&self, request: ScheduledRequest) -> bool {
        if is_retired_audio_schedule(request.cmd.as_wire()) {
            return false;
        }
        let name = request.scheduler_name.clone();
        let reference = request.reference.clone();
        let accepted = !matches!(
            self.queue.submit(ExecutionRequest::Scheduled(request)),
            solstone_core_system::queue::SubmitOutcome::Rejected
        );
        if accepted {
            emit(
                &self.server,
                "supervisor",
                "scheduled",
                Map::from_iter([
                    ("scheduler_name".into(), json!(name)),
                    ("ref".into(), json!(reference)),
                ]),
            );
        }
        accepted
    }
}

pub(crate) struct SupervisorProviderSink(pub Arc<CallosumSocketServer>);
impl ProviderRuntimeEventSink for SupervisorProviderSink {
    fn emit(&mut self, event: ProviderRuntimeEvent) {
        let (kind, provider, operation) = match event {
            ProviderRuntimeEvent::Step(_) => return,
            ProviderRuntimeEvent::Dispatched {
                operation,
                provider,
            } => ("dispatched", Some(provider), Some(operation)),
            ProviderRuntimeEvent::StaleResultDiscarded {
                operation,
                provider,
            } => ("stale_result_discarded", Some(provider), Some(operation)),
            ProviderRuntimeEvent::RetryScheduled { provider } => {
                ("retry_scheduled", Some(provider), None)
            }
            ProviderRuntimeEvent::RetryExhausted { provider } => {
                ("retry_exhausted", Some(provider), None)
            }
            ProviderRuntimeEvent::StopDeferred { provider } => {
                ("stop_deferred", Some(provider), None)
            }
            ProviderRuntimeEvent::CleanupRetry { provider } => {
                ("cleanup_retry", Some(provider), None)
            }
            ProviderRuntimeEvent::RecycleRequested { provider } => {
                ("recycle_requested", Some(provider), None)
            }
            ProviderRuntimeEvent::GateReleased => ("gate_released", None, None),
        };
        let mut extra = Map::from_iter([("kind".into(), json!(kind))]);
        if let Some(provider) = provider {
            extra.insert("provider".into(), json!(provider.as_str()));
        }
        if let Some(operation) = operation {
            extra.insert("operation".into(), json!(operation));
        }
        emit(&self.0, "supervisor", "provider_runtime", extra);
    }
}
