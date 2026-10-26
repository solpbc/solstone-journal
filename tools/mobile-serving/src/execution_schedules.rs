// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner recurrence over the shared schedule authority. No process launcher.

use super::*;
use axum::extract::Path;
use axum::routing::put;
use solstone_core_system::partition::canonical_journal_command;
use solstone_core_system::schedule::{
    ScheduleMutation, mutate_schedule_entries_at_root_checked, validate_schedule_entry,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThinkRequest {
    args: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recurrence {
    every: String,
    enabled: bool,
    max_runtime: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThinkRecurrence {
    args: Vec<String>,
    every: String,
    enabled: bool,
    max_runtime: Option<String>,
}

pub(super) fn routes(sender: OwnerIngress) -> Router {
    Router::new()
        .route("/api/journal/think", post(think))
        .route("/api/journal/maintenance/{routine}", post(maintenance))
        .route("/api/journal/recurrences", get(list))
        .route(
            "/api/journal/recurrences/think/{name}",
            put(put_think).delete(remove_think),
        )
        .route(
            "/api/journal/recurrences/source/{source}",
            get(source_status).put(put_source).delete(remove_source),
        )
        .with_state(sender)
}

async fn think(State(sender): State<OwnerIngress>, Json(request): Json<ThinkRequest>) -> Response {
    submit_operation(sender, Operation::Think(request.args)).await
}
async fn maintenance(State(sender): State<OwnerIngress>, Path(routine): Path<String>) -> Response {
    submit_operation(sender, Operation::Maintenance(routine)).await
}
async fn list(State(sender): State<OwnerIngress>) -> Response {
    submit_operation(sender, Operation::ListRecurrences).await
}
async fn put_think(
    State(sender): State<OwnerIngress>,
    Path(name): Path<String>,
    Json(request): Json<ThinkRecurrence>,
) -> Response {
    let Some(name) = think_name(&name) else {
        return invalid();
    };
    submit_operation(
        sender,
        Operation::PutRecurrence {
            name,
            command: think_command(request.args),
            every: request.every,
            enabled: request.enabled,
            max_runtime: request.max_runtime,
        },
    )
    .await
}
async fn remove_think(State(sender): State<OwnerIngress>, Path(name): Path<String>) -> Response {
    let Some(name) = think_name(&name) else {
        return invalid();
    };
    submit_operation(sender, Operation::RemoveRecurrence(name)).await
}
async fn put_source(
    State(sender): State<OwnerIngress>,
    Path(source): Path<String>,
    Json(request): Json<Recurrence>,
) -> Response {
    if source_command(&source).is_none() {
        return invalid();
    }
    submit_operation(
        sender,
        Operation::Source(sources::SourceOperation {
            source,
            action: sources::SourceAction::Put {
                every: request.every,
                enabled: request.enabled,
                max_runtime: request.max_runtime,
            },
        }),
    )
    .await
}
async fn remove_source(State(sender): State<OwnerIngress>, Path(source): Path<String>) -> Response {
    if source_command(&source).is_none() {
        return invalid();
    }
    submit_operation(
        sender,
        Operation::Source(sources::SourceOperation {
            source,
            action: sources::SourceAction::Remove,
        }),
    )
    .await
}
async fn source_status(State(sender): State<OwnerIngress>, Path(source): Path<String>) -> Response {
    if source_command(&source).is_none() {
        return invalid();
    }
    submit_operation(
        sender,
        Operation::Source(sources::SourceOperation {
            source,
            action: sources::SourceAction::Status,
        }),
    )
    .await
}
fn think_name(name: &str) -> Option<String> {
    (!name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'))
    .then(|| format!("owner:think:{name}"))
}
fn think_command(args: Vec<String>) -> Vec<String> {
    canonical_journal_command(std::iter::once("think".to_owned()).chain(args))
}
pub(super) fn source_command(source: &str) -> Option<Vec<String>> {
    // Only the maintained auxiliary source connections. Body importers retain
    // their shared migration/approval prerequisite and receive no new schedule.
    matches!(source, "plaud" | "obsidian" | "audio")
        .then(|| canonical_journal_command(["importer", "--sync", source, "--save"]))
}
fn invalid() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":"recurrence_invalid"})),
    )
        .into_response()
}

fn valid_think(root: &JournalRoot, args: &[String]) -> bool {
    args.len() <= 128
        && args.iter().map(String::len).sum::<usize>() <= 64 * 1024
        && solstone_core_think_cli::hosted_command_day(args, chrono::Utc::now().date_naive())
            .is_ok()
        && crate::foreground::admit_command(root.canonical_path(), &think_command(args.to_vec()))
}

pub(super) fn execute(
    root: &Arc<JournalRoot>,
    operation: Operation,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> (StatusCode, Value) {
    if cancelled() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"execution_stopped"}),
        );
    }
    match operation {
        Operation::Think(args) => {
            if !valid_think(root, &args) {
                return (
                    StatusCode::BAD_REQUEST,
                    json!({"error":"think_request_invalid"}),
                );
            }
            if cancelled() {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    json!({"error":"execution_stopped"}),
                );
            }
            if let Some(result) =
                solstone_core_think_cli::hosted_inspection(root.canonical_path(), &args)
            {
                if cancelled() {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        json!({"error":"execution_stopped"}),
                    );
                }
                return (
                    if result.exit_code == 0 {
                        StatusCode::OK
                    } else {
                        StatusCode::UNPROCESSABLE_ENTITY
                    },
                    json!({"stdout":result.stdout,"stderr":result.stderr,"exit_code":result.exit_code}),
                );
            }
            match crate::foreground::submit_owner_think(root, args) {
                Ok(reference) => (
                    StatusCode::ACCEPTED,
                    json!({"submitted":true,"reference":reference}),
                ),
                Err(_) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    json!({"error":"thinking_queue_unavailable"}),
                ),
            }
        }
        Operation::Maintenance(routine) => {
            if !solstone_core_maintenance::registry::routines()
                .iter()
                .any(|row| row.id == routine)
            {
                return (StatusCode::BAD_REQUEST, json!({"error":"routine_unknown"}));
            }
            match crate::foreground::submit_owner_maintenance(root, &routine) {
                Ok(reference) => (
                    StatusCode::ACCEPTED,
                    json!({"submitted":true,"reference":reference}),
                ),
                Err(_) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    json!({"error":"maintenance_queue_unavailable"}),
                ),
            }
        }
        Operation::ListRecurrences => mutate(root, cancelled, |raw| {
            (StatusCode::OK, json!({"schedules":raw}))
        }),
        Operation::PutRecurrence {
            name,
            command,
            every,
            enabled,
            max_runtime,
        } => {
            if name.starts_with("owner:think:") && !valid_think(root, &command[3..]) {
                return (
                    StatusCode::BAD_REQUEST,
                    json!({"error":"think_request_invalid"}),
                );
            }
            let mut entry = json!({"cmd":command,"every":every,"enabled":enabled});
            if let Some(max) = max_runtime {
                entry["max_runtime"] = json!(max);
            }
            if validate_schedule_entry(&entry).is_err() {
                return (
                    StatusCode::BAD_REQUEST,
                    json!({"error":"recurrence_invalid"}),
                );
            }
            mutate(root, cancelled, move |raw| {
                raw.insert(name.clone(), entry.clone());
                (StatusCode::OK, json!({"name":name,"entry":entry}))
            })
        }
        Operation::RemoveRecurrence(name) => mutate(root, cancelled, move |raw| {
            (
                StatusCode::OK,
                json!({"name":name,"removed":raw.remove(&name).is_some()}),
            )
        }),
        Operation::Source(request) => sources::execute(root, request, cancelled),
        Operation::SettingsSync(updates) => sources::settings(root, updates, cancelled),
        Operation::Share { .. } => unreachable!("share preview has its own domain owner"),
    }
}
pub(super) fn mutate(
    root: &Arc<JournalRoot>,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
    mut apply: impl FnMut(&mut serde_json::Map<String, Value>) -> (StatusCode, Value),
) -> (StatusCode, Value) {
    let checkpoint = || {
        if cancelled() {
            Err("execution stopped".to_owned())
        } else {
            Ok(())
        }
    };
    let result = mutate_schedule_entries_at_root_checked(Arc::clone(root), &checkpoint, |raw| {
        if cancelled() || root.revalidate_canonical_binding().is_err() {
            return ScheduleMutation {
                changed: false,
                value: Err(()),
            };
        }
        let before = raw.clone();
        let value = apply(raw);
        ScheduleMutation {
            changed: *raw != before,
            value: Ok(value),
        }
    });
    match result {
        Ok(Ok(value)) => value,
        Ok(Err(())) => (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"execution_stopped"}),
        ),
        Err(error) => {
            // Retain publication/uncertainty facts even when Stop races the
            // refusal; cancellation must not erase the native diagnosis.
            log::warn!("owner recurrence: {error}");
            if cancelled() || root.revalidate_canonical_binding().is_err() {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    json!({"error":"execution_stopped"}),
                );
            }
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"error":"schedules_unavailable"}),
            )
        }
    }
}
