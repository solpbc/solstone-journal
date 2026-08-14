//! Native backup Convey routes. Engine operations are deliberately explicit 501
//! refusals until their restic/rclone owners have native implementations.

use std::path::PathBuf;

use axum::{
    Router,
    body::Bytes,
    routing::{get, post},
};
use serde_json::{Value, json};

mod assets;
mod callosum;
mod config;
mod keys;
mod measurement;
mod refuse;
mod response;
mod status;
mod validation;

use measurement::SharedMeasurementCache;

pub fn routes(journal_root: PathBuf) -> Router {
    let cache = measurement::new(&journal_root);
    routes_with_cache(journal_root, cache)
}

fn routes_with_cache(journal_root: PathBuf, cache: SharedMeasurementCache) -> Router {
    let status_root = journal_root.clone();
    let offload_root = journal_root.clone();
    let config_root = journal_root.clone();
    let enable_root = journal_root.clone();
    let disable_root = journal_root.clone();
    let keys_root = journal_root.clone();
    let reveal_root = journal_root.clone();
    let confirm_root = journal_root.clone();
    let retention_root = journal_root.clone();
    let backup_now_root = journal_root.clone();
    let enable_refusal_root = journal_root.clone();
    let hosted_refusal_root = journal_root.clone();
    let destination_refusal_root = journal_root.clone();
    let restore_refusal_root = journal_root.clone();
    let restore_hosted_refusal_root = journal_root;
    let status_cache = cache.clone();
    let config_cache = cache.clone();
    let enable_cache = cache.clone();
    let disable_cache = cache;
    Router::new()
        .route("/app/backup/", get(assets::shell))
        .route("/app/backup/workspace", get(assets::workspace))
        .route("/app/backup/background", get(assets::background))
        .route("/app/backup/static/{name}", get(assets::static_asset))
        .route(
            "/app/backup/status",
            get(move || get_status(status_root.clone())),
        )
        .route(
            "/app/backup/offload/status",
            get(move || get_offload(offload_root.clone(), status_cache.clone())),
        )
        .route(
            "/app/backup/offload/config",
            post(move |body| offload_config(config_root.clone(), config_cache.clone(), body)),
        )
        .route(
            "/app/backup/offload/enable",
            post(move || offload_enable(enable_root.clone(), enable_cache.clone())),
        )
        .route(
            "/app/backup/offload/disable",
            post(move || offload_disable(disable_root.clone(), disable_cache.clone())),
        )
        .route(
            "/app/backup/keys/generate",
            post(move || generate_keys(keys_root.clone())),
        )
        .route(
            "/app/backup/recovery-key/reveal",
            post(move || reveal(reveal_root.clone())),
        )
        .route(
            "/app/backup/confirm",
            post(move |body| confirm(confirm_root.clone(), body)),
        )
        .route(
            "/app/backup/retention",
            post(move |body| retention(retention_root.clone(), body)),
        )
        .route(
            "/app/backup/backup-now",
            post(move || backup_now(backup_now_root.clone())),
        )
        .route(
            "/app/backup/enable",
            post(move || enable_refusal(enable_refusal_root.clone())),
        )
        .route(
            "/app/backup/enable-hosted",
            post(move || hosted_refusal(hosted_refusal_root.clone())),
        )
        .route(
            "/app/backup/destination",
            post(move |body| destination_refusal(destination_refusal_root.clone(), body)),
        )
        .route(
            "/app/backup/recovery-key/rotate",
            post(|| async {
                refuse::native_refusal(refuse::BACKUP_RECOVERY_KEY_ROTATE_NOT_IMPLEMENTED_NATIVE)
            }),
        )
        .route(
            "/app/backup/teardown",
            post(|| async {
                refuse::native_refusal(refuse::BACKUP_TEARDOWN_NOT_IMPLEMENTED_NATIVE)
            }),
        )
        .route(
            "/app/backup/restore",
            post(move |body| restore_refusal(restore_refusal_root.clone(), body)),
        )
        .route(
            "/app/backup/restore-hosted",
            post(move |body| restore_hosted_refusal(restore_hosted_refusal_root.clone(), body)),
        )
        .route("/app/backup/offload/restore", post(offload_restore_refusal))
}

async fn get_status(root: PathBuf) -> axum::response::Response {
    status::status(&root)
        .map(response::success)
        .unwrap_or_else(|_| {
            response::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "I couldn't complete that request.",
                "internal_error",
                "",
            )
        })
}
async fn get_offload(root: PathBuf, cache: SharedMeasurementCache) -> axum::response::Response {
    status::offload(&root, &cache)
        .map(response::success)
        .unwrap_or_else(|_| {
            response::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "I couldn't complete that request.",
                "internal_error",
                "",
            )
        })
}

async fn generate_keys(root: PathBuf) -> axum::response::Response {
    let generated_daily = match keys::generated_key() {
        Ok(key) => key,
        Err(_) => return internal_error(),
    };
    let generated_recovery = match keys::generated_key() {
        Ok(key) => key,
        Err(_) => return internal_error(),
    };
    let outcome = config::mutate(&root, |backup| {
        let daily = backup.get("daily_key").cloned().unwrap_or(Value::Null);
        let recovery = backup.get("recovery_key").cloned().unwrap_or(Value::Null);
        let mut changed = false;
        let daily = if daily.is_null() {
            changed = true;
            Value::String(generated_daily)
        } else {
            daily
        };
        let recovery = if recovery.is_null() {
            changed = true;
            Value::String(generated_recovery)
        } else {
            recovery
        };
        backup.insert("daily_key".to_owned(), daily);
        backup.insert("recovery_key".to_owned(), recovery.clone());
        (changed, recovery.as_str().unwrap_or_default().to_owned())
    });
    match outcome.and_then(|recovery| keys::format(&recovery).map_err(|_| ())) {
        Ok(display) => response::success(json!({"success":true,"recovery_key_display":display})),
        Err(_) => response::error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't complete that request.",
            "internal_error",
            "",
        ),
    }
}

fn internal_error() -> axum::response::Response {
    response::error(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "I couldn't complete that request.",
        "internal_error",
        "",
    )
}
async fn reveal(root: PathBuf) -> axum::response::Response {
    match config::backup(&root).and_then(|config| keys::keys(&config)) {
        Ok(Some((_, recovery))) => match keys::format(&recovery) {
            Ok(display) => {
                response::success(json!({"success":true,"recovery_key_display":display}))
            }
            Err(_) => response::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "I couldn't complete that request.",
                "internal_error",
                "",
            ),
        },
        Ok(None) => response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't take that action in the current state.",
            "invalid_operation_for_state",
            "no recovery key yet",
        ),
        Err(_) => response::error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't complete that request.",
            "internal_error",
            "",
        ),
    }
}
async fn confirm(root: PathBuf, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let entered = match validation::required_string(&payload, "recovery_key") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let recovery = match config::backup(&root).and_then(|config| keys::keys(&config)) {
        Ok(Some((_, key))) => key,
        Ok(None) => {
            return response::error(
                axum::http::StatusCode::BAD_REQUEST,
                "I couldn't take that action in the current state.",
                "invalid_operation_for_state",
                "no recovery key yet",
            );
        }
        Err(_) => {
            return response::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "I couldn't complete that request.",
                "internal_error",
                "",
            );
        }
    };
    if keys::parse(&entered).ok().as_deref() != Some(&recovery) {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't confirm that — it didn't match your recovery key.",
            "recovery_key_mismatch",
            "",
        );
    }
    if config::mutate(&root, |backup| {
        let changed = backup.get("confirmed_recovery_key") != Some(&Value::Bool(true));
        backup.insert("confirmed_recovery_key".to_owned(), Value::Bool(true));
        (changed, ())
    })
    .is_err()
    {
        return response::error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't complete that request.",
            "internal_error",
            "",
        );
    }
    get_status(root).await
}
async fn retention(root: PathBuf, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::invalid_config) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let next = match validation::retention(&payload) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if config::mutate(&root, |backup| {
        let value = Value::Object(next);
        let changed = backup.get("retention") != Some(&value);
        backup.insert("retention".to_owned(), value);
        (changed, ())
    })
    .is_err()
    {
        return response::error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't complete that request.",
            "internal_error",
            "",
        );
    }
    get_status(root).await
}
async fn offload_config(
    root: PathBuf,
    cache: SharedMeasurementCache,
    body: Bytes,
) -> axum::response::Response {
    let payload = match validation::object(&body, response::invalid_config) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let (budget, floor) = match validation::offload(&payload) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if config::mutate(&root, |backup| {
        let enabled = backup
            .get("offload")
            .and_then(Value::as_object)
            .and_then(|value| value.get("enabled"))
            .cloned()
            .unwrap_or(Value::Bool(false));
        let value = json!({"enabled":enabled,"budget_bytes":budget,"floor_bytes":floor});
        let changed = backup.get("offload") != Some(&value);
        backup.insert("offload".to_owned(), value);
        (changed, ())
    })
    .is_err()
    {
        return response::error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't complete that request.",
            "internal_error",
            "",
        );
    }
    measurement::invalidate(&cache);
    get_offload(root, cache).await
}
async fn offload_enable(root: PathBuf, cache: SharedMeasurementCache) -> axum::response::Response {
    let backup = match config::backup(&root) {
        Ok(value) => value,
        Err(_) => {
            return response::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "I couldn't complete that request.",
                "internal_error",
                "",
            );
        }
    };
    if backup.get("enabled") != Some(&Value::Bool(true)) {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't take that action in the current state.",
            "invalid_operation_for_state",
            "backup is disabled",
        );
    }
    if backup.get("confirmed_recovery_key") != Some(&Value::Bool(true)) {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't turn on backup until you confirm your recovery key.",
            "backup_not_confirmed",
            "",
        );
    }
    if !matches!(keys::keys(&backup), Ok(Some(_))) {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't take that action in the current state.",
            "invalid_operation_for_state",
            "backup keys are missing",
        );
    }
    let defaults = measurement::snapshot(&cache);
    if config::mutate(&root, |backup| { let old = backup.get("offload").and_then(Value::as_object); let value = json!({"enabled":true,"budget_bytes":old.and_then(|item| item.get("budget_bytes")).cloned().filter(|value| !value.is_null()).unwrap_or_else(|| defaults["suggested_defaults"]["budget_bytes"].clone()),"floor_bytes":old.and_then(|item| item.get("floor_bytes")).cloned().filter(|value| !value.is_null()).unwrap_or_else(|| defaults["suggested_defaults"]["floor_bytes"].clone())}); let changed = backup.get("offload") != Some(&value); backup.insert("offload".to_owned(), value); (changed, ()) }).is_err() { return response::invalid_config(""); }
    if backup["last_verification"]["status"].is_null() {
        let _ = callosum::request(&root, "backup:verify");
    }
    measurement::invalidate(&cache);
    get_offload(root, cache).await
}
async fn offload_disable(root: PathBuf, cache: SharedMeasurementCache) -> axum::response::Response {
    if config::mutate(&root, |backup| { let old = backup.get("offload").and_then(Value::as_object); let value = json!({"enabled":false,"budget_bytes":old.and_then(|item| item.get("budget_bytes")).cloned().unwrap_or(Value::Null),"floor_bytes":old.and_then(|item| item.get("floor_bytes")).cloned().unwrap_or(Value::Null)}); let changed = backup.get("offload") != Some(&value); backup.insert("offload".to_owned(), value); (changed, ()) }).is_err() { return response::invalid_config(""); }
    measurement::invalidate(&cache);
    get_offload(root, cache).await
}
async fn backup_now(root: PathBuf) -> axum::response::Response {
    if !callosum::request(&root, "backup:run") {
        return response::error(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "I couldn't start a backup because your journal's background service isn't running. Start it, then try again.",
            "backup_unavailable",
            "",
        );
    }
    get_status(root).await
}
async fn enable_refusal(root: PathBuf) -> axum::response::Response {
    let backup = match config::backup(&root) {
        Ok(value) => value,
        Err(_) => {
            return response::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "I couldn't complete that request.",
                "internal_error",
                "",
            );
        }
    };
    let destination = backup.get("destination").and_then(Value::as_object);
    if destination
        .and_then(|value| value.get("repository"))
        .is_none()
    {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't take that action in the current state.",
            "invalid_operation_for_state",
            "configure a destination first",
        );
    }
    if backup.get("confirmed_recovery_key") != Some(&Value::Bool(true)) {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't turn on backup until you confirm your recovery key.",
            "backup_not_confirmed",
            "",
        );
    }
    if !matches!(keys::keys(&backup), Ok(Some(_))) {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't take that action in the current state.",
            "invalid_operation_for_state",
            "no recovery key yet",
        );
    }
    refuse::native_refusal(refuse::BACKUP_ENABLE_NOT_IMPLEMENTED_NATIVE)
}
async fn hosted_refusal(root: PathBuf) -> axum::response::Response {
    let backup = match config::backup(&root) {
        Ok(value) => value,
        Err(_) => {
            return response::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "I couldn't complete that request.",
                "internal_error",
                "",
            );
        }
    };
    if backup.get("confirmed_recovery_key") != Some(&Value::Bool(true)) {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't turn on backup until you confirm your recovery key.",
            "backup_not_confirmed",
            "",
        );
    }
    if !matches!(keys::keys(&backup), Ok(Some(_))) {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't take that action in the current state.",
            "invalid_operation_for_state",
            "no recovery key yet",
        );
    }
    refuse::native_refusal(refuse::BACKUP_ENABLE_HOSTED_NOT_IMPLEMENTED_NATIVE)
}
async fn destination_refusal(_root: PathBuf, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    match validation::destination(&payload) {
        Ok(()) => refuse::native_refusal(refuse::BACKUP_DESTINATION_NOT_IMPLEMENTED_NATIVE),
        Err(error) => error,
    }
}
async fn restore_refusal(_root: PathBuf, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Err(error) = validation::required_string(&payload, "recovery_key") {
        return error;
    }
    match validation::destination(&payload) {
        Ok(()) => refuse::native_refusal(refuse::BACKUP_RESTORE_NOT_IMPLEMENTED_NATIVE),
        Err(error) => error,
    }
}
async fn restore_hosted_refusal(_root: PathBuf, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Err(error) = validation::required_string(&payload, "recovery_key") {
        return error;
    }
    refuse::native_refusal(refuse::BACKUP_RESTORE_HOSTED_NOT_IMPLEMENTED_NATIVE)
}
async fn offload_restore_refusal(body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    match validation::restore_day(&payload) {
        Ok(()) => refuse::native_refusal(refuse::BACKUP_OFFLOAD_RESTORE_NOT_IMPLEMENTED_NATIVE),
        Err(error) => error,
    }
}

#[cfg(test)]
mod corpus;
#[cfg(test)]
mod test_support;
