// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    photos,
    router::{RouterState, admitted},
};
use axum::{
    Json,
    extract::{Extension, Path as RoutePath, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use solstone_core_convey_http::{
    coalesce::ResponseLocale,
    envelope::error_envelope,
    identity::AccessBasis,
    media::{MediaBody, media_response},
    window_catalog::{response_locale, text},
};
use solstone_core_entity::faceprints as fp;
use std::{io::Cursor, path::Path, sync::Arc};

fn error(locale: &str, error: photos::Error) -> Response {
    use fp::ActionError as A;
    let (code, message_id, status) = match &error {
        photos::Error::EntityNotFound => (
            "entity_not_found",
            "journal.window.entities.response.photo.entity.not.found",
            StatusCode::NOT_FOUND,
        ),
        photos::Error::KeyNotFound | photos::Error::Action(A::UnknownKey | A::NoEvidence) => (
            "face_not_found",
            "journal.window.entities.response.photo.face.not.found",
            StatusCode::NOT_FOUND,
        ),
        photos::Error::Disabled => (
            "faces_disabled",
            "journal.window.entities.response.photo.faces.disabled",
            StatusCode::CONFLICT,
        ),
        photos::Error::Action(A::FacesOn) => (
            "faces_enabled",
            "journal.window.entities.response.photo.faces.enabled",
            StatusCode::CONFLICT,
        ),
        photos::Error::Action(A::BusySource(_)) => (
            "faces_busy",
            "journal.window.entities.response.photo.faces.busy",
            StatusCode::CONFLICT,
        ),
        photos::Error::Action(A::StaleAction) => (
            "face_action_changed",
            "journal.window.entities.response.photo.face.action.changed",
            StatusCode::CONFLICT,
        ),
        _ => (
            "faces_read_failed",
            "journal.window.entities.response.photo.faces.read.failed",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    };
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        log::warn!("face request failed: {error:?}");
    }
    error_envelope(code, text(locale, message_id), "", status).into_response()
}

async fn result<T: serde::Serialize + Send + 'static>(
    locale: &str,
    work: impl FnOnce() -> Result<T, photos::Error> + Send + 'static,
) -> Response {
    match solstone_core_serving::seam::run_blocking(work).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(e)) => error(locale, e),
        Err(e) => error(locale, photos::Error::Read(e.to_string())),
    }
}

pub(crate) async fn panel(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(id): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let locale = response_locale(locale);
    result(locale, move || photos::panel(&root, &id)).await
}

async fn image(
    locale: &str,
    work: impl FnOnce() -> Result<(Vec<u8>, Option<photos::State>), photos::Error> + Send + 'static,
) -> Response {
    match solstone_core_serving::seam::run_blocking(move || {
        let (bytes, state) = work()?;
        let length = bytes.len();
        let body = MediaBody::from_reader(Cursor::new(bytes), 0, length)
            .map_err(|e| photos::Error::Read(e.to_string()))?;
        Ok::<_, photos::Error>((body, state))
    })
    .await
    {
        Ok(Ok((body, state))) => {
            let mut response = media_response(
                StatusCode::OK,
                "image/jpeg",
                Path::new(fp::PHOTO),
                body,
                None,
            );
            if let Some(state) = state {
                let value = match state {
                    photos::State::Confirmed => "confirmed",
                    _ => "suggested",
                };
                response
                    .headers_mut()
                    .insert("x-photo-state", value.parse().expect("static header"));
            }
            response
        }
        Ok(Err(e)) => error(locale, e),
        Err(e) => error(locale, photos::Error::Read(e.to_string())),
    }
}

pub(crate) async fn photo(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(id): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let locale = response_locale(locale);
    image(locale, move || {
        let photo = photos::photo(&root, &id, None)?;
        Ok((
            photo.jpeg.ok_or(photos::Error::KeyNotFound)?,
            Some(photo.state),
        ))
    })
    .await
}
pub(crate) async fn crop(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath((id, key)): RoutePath<(String, String)>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let locale = response_locale(locale);
    image(locale, move || {
        Ok((photos::crop(&root, &id, &photos::decode_key(&key)?)?, None))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyBody {
    key: fp::FaceKey,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UndoBody {
    token: String,
}

async fn body<T: serde::de::DeserializeOwned>(
    locale: &str,
    request: Request,
) -> Result<T, Response> {
    let bytes = axum::body::to_bytes(request.into_body(), 16 * 1024)
        .await
        .map_err(|_| invalid_body(locale))?;
    serde_json::from_slice(&bytes).map_err(|_| invalid_body(locale))
}
fn invalid_body(locale: &str) -> Response {
    error_envelope(
        "invalid_request",
        text(
            locale,
            "journal.window.entities.response.photo.request.invalid",
        ),
        "",
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}
fn action_id(root: &Path, id: &str) -> Result<String, photos::Error> {
    let id = photos::entity_id(root, id)?;
    if !fp::enabled(root)? {
        return Err(photos::Error::Disabled);
    }
    Ok(id)
}

pub(crate) async fn choose(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(id): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
    request: Request,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let locale = response_locale(locale);
    let body: KeyBody = match body(locale, request).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    result(locale, move || {
        let id = action_id(&root, &id)?;
        let token = fp::choose(&root, &id, &body.key)?;
        Ok(json!({"token": token}))
    })
    .await
}
pub(crate) async fn reject(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(id): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
    request: Request,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let locale = response_locale(locale);
    let body: KeyBody = match body(locale, request).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    result(locale, move || {
        let id = action_id(&root, &id)?;
        let token = fp::reject(&root, &id, &body.key)?;
        Ok(json!({"token": token}))
    })
    .await
}
pub(crate) async fn dismiss(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(id): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let locale = response_locale(locale);
    result(locale, move || {
        let id = action_id(&root, &id)?;
        let token = fp::dismiss(&root, &id)?;
        Ok(json!({"token": token}))
    })
    .await
}
pub(crate) async fn undo(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(id): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
    request: Request,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let locale = response_locale(locale);
    let body: UndoBody = match body(locale, request).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    result(locale, move || {
        let id = action_id(&root, &id)?;
        fp::undo(&root, &id, &body.token)?;
        Ok(json!({"state": "undone"}))
    })
    .await
}
pub(crate) async fn clear(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    locale: Option<Extension<ResponseLocale>>,
    request: Request,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let locale = response_locale(locale);
    let surface = solstone_core_removals::Surface::of_request(
        solstone_core_removals::Page::Settings,
        request
            .headers()
            .get("user-agent")
            .and_then(|v| v.to_str().ok()),
    );
    result(locale, move || {
        let removed = fp::accumulation::clear(&root, surface)?;
        Ok(json!({"removed": removed}))
    })
    .await
}
