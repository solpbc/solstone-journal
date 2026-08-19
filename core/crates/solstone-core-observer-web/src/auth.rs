// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use axum::http::HeaderMap;
use solstone_core_observer::observer_prefix_for_stream;
use solstone_core_observer::store::record::ObserverRecord;
use solstone_core_observer::store::reload::load_observers;

pub(crate) enum AuthError {
    Required,
    InvalidKey,
    Revoked,
    FeatureUnavailable,
}

pub(crate) fn authorize(journal: &Path, headers: &HeaderMap) -> Result<ObserverRecord, AuthError> {
    let Some(handle) = extract_handle(headers) else {
        return Err(AuthError::Required);
    };
    let records = load_observers(journal).map_err(|_| AuthError::InvalidKey)?;
    let Some(record) = records.into_iter().find(|record| record.key() == handle) else {
        return Err(AuthError::InvalidKey);
    };
    if record.revoked() {
        return Err(AuthError::Revoked);
    }
    if !record.enabled().unwrap_or(true) {
        return Err(AuthError::FeatureUnavailable);
    }
    if let Ok(prefix) = observer_prefix_for_stream(journal, "location")
        && prefix != record.prefix()
    {
        return Err(AuthError::FeatureUnavailable);
    }
    Ok(record)
}

fn extract_handle(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers
        .get("x-solstone-observer")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(value.to_owned());
    }
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
