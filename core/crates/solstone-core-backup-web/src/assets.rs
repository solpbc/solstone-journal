// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::Response,
};

const WORKSPACE: &[u8] = include_bytes!("../assets/workspace.html");
const SHELL: &[u8] = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
const JS: &[u8] = include_bytes!("../assets/backup.js");
const CSS: &[u8] = include_bytes!("../assets/backup.css");
const NOT_FOUND: &str = "<!doctype html>\n<html lang=en>\n<title>404 Not Found</title>\n<h1>Not Found</h1>\n<p>The requested URL was not found on the server. If you entered the URL manually please check your spelling and try again.</p>\n";

fn bytes(status: StatusCode, bytes: &'static [u8], content_type: &'static str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("backup asset response")
}
pub async fn shell() -> Response {
    bytes(StatusCode::OK, SHELL, "text/html; charset=utf-8")
}
pub async fn workspace() -> Response {
    bytes(StatusCode::OK, WORKSPACE, "text/html; charset=utf-8")
}
pub async fn background() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(NOT_FOUND))
        .expect("backup background response")
}
pub async fn static_asset(axum::extract::Path(name): axum::extract::Path<String>) -> Response {
    match name.as_str() {
        "backup.js" => bytes(StatusCode::OK, JS, "text/javascript; charset=utf-8"),
        "backup.css" => bytes(StatusCode::OK, CSS, "text/css; charset=utf-8"),
        _ => background().await,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::JS;

    fn embedded_js_object(prefix: &str) -> Value {
        let source = std::str::from_utf8(JS).expect("embedded backup.js is UTF-8");
        let payload =
            &source[source.find(prefix).expect("embedded object prefix") + prefix.len()..];
        let end = payload.find(";\n").expect("embedded object terminator");
        serde_json::from_str(&payload[..end]).expect("embedded object JSON")
    }

    #[test]
    fn status_selection_table_is_complete_and_leaves_prune_outside_its_scope() {
        let actual = embedded_js_object("const STATUS_SELECTION_TABLE = ");
        let backup_never_run =
            json!({"copy_key":"status.last_backup.never_run","duration_source":null});
        let backup_ok =
            json!({"copy_key":"management.status_labels.ago","duration_source":"last_backup.time"});
        let backup_error =
            json!({"copy_key":"status.last_backup.failed","duration_source":"last_backup.time"});
        let verification_not_yet =
            json!({"copy_key":"management.status_labels.not_yet","duration_source":null});
        let verification_ok = json!({"copy_key":"status.last_verification.ok","duration_source":"last_verification.time"});
        let verification_skipped = json!({"copy_key":"status.last_verification.skipped","duration_source":"last_verification.time"});
        let verification_error = json!({"copy_key":"status.last_verification.failed","duration_source":"last_verification.time"});
        let expected = json!({
            "null|null":{"backup":backup_never_run,"verification":verification_not_yet},
            "null|ok":{"backup":{"copy_key":"status.last_backup.never_run","duration_source":null},"verification":{"copy_key":"management.status_labels.not_yet","duration_source":null}},
            "null|skipped":{"backup":{"copy_key":"status.last_backup.never_run","duration_source":null},"verification":{"copy_key":"management.status_labels.not_yet","duration_source":null}},
            "null|error":{"backup":{"copy_key":"status.last_backup.never_run","duration_source":null},"verification":{"copy_key":"management.status_labels.not_yet","duration_source":null}},
            "ok|null":{"backup":backup_ok,"verification":{"copy_key":"management.status_labels.not_yet","duration_source":null}},
            "ok|ok":{"backup":{"copy_key":"management.status_labels.ago","duration_source":"last_backup.time"},"verification":verification_ok},
            "ok|skipped":{"backup":{"copy_key":"management.status_labels.ago","duration_source":"last_backup.time"},"verification":verification_skipped},
            "ok|error":{"backup":{"copy_key":"management.status_labels.ago","duration_source":"last_backup.time"},"verification":verification_error},
            "error|null":{"backup":backup_error,"verification":{"copy_key":"management.status_labels.not_yet","duration_source":null}},
            "error|ok":{"backup":{"copy_key":"status.last_backup.failed","duration_source":"last_backup.time"},"verification":{"copy_key":"status.last_verification.ok","duration_source":"last_verification.time"}},
            "error|skipped":{"backup":{"copy_key":"status.last_backup.failed","duration_source":"last_backup.time"},"verification":{"copy_key":"status.last_verification.skipped","duration_source":"last_verification.time"}},
            "error|error":{"backup":{"copy_key":"status.last_backup.failed","duration_source":"last_backup.time"},"verification":{"copy_key":"status.last_verification.failed","duration_source":"last_verification.time"}}
        });
        assert_eq!(actual, expected);
        assert!(!actual.to_string().contains("last_prune"));
    }

    #[test]
    fn backup_status_reason_labels_are_closed() {
        let copy = embedded_js_object("  const BACKUP_COPY = ");
        let reasons = &copy["status"]["error_reasons"];
        assert_eq!(
            reasons
                .as_object()
                .expect("error reasons object")
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from([
                "_missing",
                "auth_failed",
                "backup_unavailable",
                "incomplete",
                "locked",
                "repo_missing",
                "rclone_unavailable",
                "restic_unavailable",
                "timeout",
            ])
        );
        assert!(reasons.get("integrity_failed").is_none());
    }

    #[test]
    fn teardown_gate_conservation_is_pinned() {
        let source = std::str::from_utf8(JS).expect("embedded backup.js is UTF-8");
        let copy = embedded_js_object("  const BACKUP_COPY = ");
        let management = &copy["management"];
        assert_eq!(
            management["teardown_gate_lead"],
            "{days} days of your journal ({size}) exist only in this backup. deleting the backup deletes them everywhere, forever."
        );
        assert_eq!(
            management["teardown_gate_unavailable_lead"],
            "can't verify what exists only in this backup right now. deleting the backup may destroy days of your journal that exist nowhere else."
        );
        assert_eq!(
            management["teardown_gate_zero_lead"],
            "nothing exists only in this backup right now. every day is still on your device."
        );
        let start = source
            .find("function backupOnlyTotalsForTeardown()")
            .expect("backupOnlyTotalsForTeardown");
        let end = source[start..]
            .find("function renderTeardownGate(")
            .expect("renderTeardownGate");
        let totals_fn = &source[start..start + end];
        assert!(
            totals_fn.contains("if (backupOnly.degraded !== false) return null;"),
            "{totals_fn}"
        );
        let render_start = start + end;
        let render_end = source[render_start..]
            .find("\n  function ")
            .expect("next function after renderTeardownGate");
        let render_fn = &source[render_start..render_start + render_end];
        assert!(
            render_fn.contains("if (totals.days === 0 && totals.bytes === 0)"),
            "{render_fn}"
        );
        assert!(
            !totals_fn.contains("if (totals.days > 0)")
                && !render_fn.contains("if (totals.days > 0)"),
            "banned days>0 gate form"
        );
    }
}
