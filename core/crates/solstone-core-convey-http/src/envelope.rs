// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::{Json, Router};
use serde::Serialize;

use crate::gate::require_access;
use crate::identity::AccessBasis;

/// The legacy JSON error shape shared with the Python convey service.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub error: String,
    pub reason_code: String,
    pub detail: String,
}

/// Build a legacy-compatible JSON error response.
pub fn error_envelope(
    reason_code: impl Into<String>,
    message: impl Into<String>,
    detail: impl Into<String>,
    status: StatusCode,
) -> (StatusCode, Json<ErrorEnvelope>) {
    (
        status,
        Json(ErrorEnvelope {
            error: message.into(),
            reason_code: reason_code.into(),
            detail: detail.into(),
        }),
    )
}

/// Return the substrate's minimal route probe and fallback response.
pub async fn not_found_fallback(
    Extension(basis): Extension<AccessBasis>,
) -> (StatusCode, Json<ErrorEnvelope>) {
    require_access(&basis);
    error_envelope(
        "not_found",
        "Not Found",
        format!("{basis:?}"),
        StatusCode::NOT_FOUND,
    )
}

/// Build the only route surface owned by this transport substrate.
pub fn probe_router() -> Router {
    Router::new().fallback(not_found_fallback)
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    use super::error_envelope;

    #[tokio::test]
    async fn error_envelope_uses_the_legacy_json_shape() {
        let response =
            error_envelope("not_found", "Not Found", "", StatusCode::NOT_FOUND).into_response();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

        assert_eq!(
            body,
            r#"{"error":"Not Found","reason_code":"not_found","detail":""}"#
        );
    }
}
