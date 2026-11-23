use axum::{
    Json,
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use solstone_core_convey_http::{
    coalesce::ResponseLocale,
    window_catalog::{ENGLISH, strings, text},
};

#[derive(Clone)]
pub(crate) enum Wording {
    Raw(String),
    Catalog(&'static str),
    MissingField(String),
}

impl From<&str> for Wording {
    fn from(value: &str) -> Self {
        Self::Raw(value.to_owned())
    }
}

impl From<&String> for Wording {
    fn from(value: &String) -> Self {
        Self::from(value.as_str())
    }
}

impl Wording {
    fn render(&self, locale: &str) -> String {
        match self {
            Self::Raw(value) => value.clone(),
            Self::Catalog(id) => text(locale, id),
            Self::MissingField(name) => strings(
                locale,
                "journal.window.backup.response.detail.missing.field",
                [("name", name)],
            ),
        }
    }
}

#[derive(Clone)]
struct Refusal {
    message: Wording,
    reason: String,
    detail: Wording,
    guidance: Option<Wording>,
}

impl Refusal {
    fn payload(&self, locale: &str) -> Value {
        let mut payload = json!({
            "error": self.message.render(locale),
            "reason_code": self.reason,
            "detail": self.detail.render(locale),
        });
        if let Some(guidance) = &self.guidance {
            payload["guidance"] = json!(guidance.render(locale));
        }
        payload
    }
}

#[derive(Clone)]
struct ResultCopy {
    value: Value,
    messages: Vec<(&'static str, &'static str)>,
}

impl ResultCopy {
    fn payload(&self, locale: &str) -> Value {
        let mut value = self.value.clone();
        for &(path, id) in &self.messages {
            *value.pointer_mut(path).expect("known backup result field") = json!(text(locale, id));
        }
        value
    }
}

pub(crate) async fn localize(request: Request, next: Next) -> Response {
    let locale = request
        .extensions()
        .get::<ResponseLocale>()
        .map_or(ENGLISH, |locale| locale.0);
    let mut response = next.run(request).await;
    let refusal = response.extensions_mut().remove::<Refusal>();
    let result = response.extensions_mut().remove::<ResultCopy>();
    if locale != ENGLISH {
        let payload = refusal
            .map(|copy| copy.payload(locale))
            .or_else(|| result.map(|copy| copy.payload(locale)));
        if let Some(payload) = payload {
            *response.body_mut() = Json(payload).into_response().into_body();
            response.headers_mut().remove(header::CONTENT_LENGTH);
        }
    }
    response
}

const KNOWN_RESULT_IDS: &[&str] = &[
    "journal.window.backup.response.offload.change.partial",
    "journal.window.backup.response.destination.result.message.backup.repository.is.reachable",
    "journal.window.backup.response.destination.result.message.these.credentials.can.t.open.a",
    "journal.window.backup.response.destination.result.message.backup.destination.is.reachable.and.needs",
    "journal.window.backup.response.destination.result.message.repository.password.was.rejected",
    "journal.window.backup.response.destination.result.message.repository.is.locked.try.again.shortly",
    "journal.window.backup.response.destination.result.message.could.not.reach.the.backup.destination",
    "journal.window.backup.response.destination.result.message.the.destination.rejected.the.credentials",
    "journal.window.backup.response.destination.result.message.a.backup.there.doesn.t.open",
    "journal.window.backup.response.storage.retention.result.your.storage.keeps.earlier.versions.of",
    "journal.window.backup.response.storage.retention.result.your.bucket.can.lock.files.against",
    "journal.window.backup.response.storage.retention.result.your.b2.bucket.only.hides.deleted",
    "journal.window.backup.response.storage.retention.result.your.journal.couldn.t.check.whether",
];

fn result_message(value: &str) -> Option<&'static str> {
    use solstone_core_installed_payload::guidance;
    match value {
        guidance::PACKAGE_MISMATCH => Some("journal.window.backup.response.tool.package.mismatch"),
        guidance::RESTART_UPDATE => Some("journal.window.backup.response.tool.restart.update"),
        guidance::MANIFEST_MISSING => Some("journal.window.backup.response.tool.manifest.missing"),
        guidance::MANIFEST_INVALID => Some("journal.window.backup.response.tool.manifest.invalid"),
        guidance::UNSUPPORTED_LOCATION => {
            Some("journal.window.backup.response.tool.unsupported.location")
        }
        guidance::WRONG_PRODUCT => Some("journal.window.backup.response.tool.wrong.product"),
        guidance::WRONG_TARGET => Some("journal.window.backup.response.tool.wrong.target"),
        guidance::MEMBER_UNREADABLE => {
            Some("journal.window.backup.response.tool.member.unreadable")
        }
        guidance::UNSAFE_PATH => Some("journal.window.backup.response.tool.unsafe.path"),
        guidance::SCREEN_OCR_UNAVAILABLE => {
            Some("journal.window.backup.response.tool.screen.ocr.unavailable")
        }
        _ => KNOWN_RESULT_IDS.iter().copied().find(|id| *id == value),
    }
}

pub fn success(value: Value) -> Response {
    payload(StatusCode::OK, value)
}

pub(crate) fn payload(status: StatusCode, value: Value) -> Response {
    let messages = [
        "/error",
        "/destination_status/message",
        "/destination_status/guidance",
        "/operation/guidance",
    ]
    .into_iter()
    .filter_map(|path| Some((path, result_message(value.pointer(path)?.as_str()?)?)))
    .collect::<Vec<_>>();
    if messages.is_empty() {
        return (status, Json(value)).into_response();
    }
    let copy = ResultCopy { value, messages };
    let mut response = (status, Json(copy.payload(ENGLISH))).into_response();
    response.extensions_mut().insert(copy);
    response
}

pub fn error(
    status: StatusCode,
    message: impl Into<Wording>,
    reason_code: &str,
    detail: impl Into<Wording>,
) -> Response {
    error_with_guidance(status, message, reason_code, detail, None)
}

pub fn error_with_guidance(
    status: StatusCode,
    message: impl Into<Wording>,
    reason_code: &str,
    detail: impl Into<Wording>,
    guidance: Option<&str>,
) -> Response {
    let copy = Refusal {
        message: message.into(),
        reason: reason_code.to_owned(),
        detail: detail.into(),
        guidance: guidance.map(|value| {
            result_message(value).map_or_else(|| Wording::from(value), Wording::Catalog)
        }),
    };
    let mut response = (status, Json(copy.payload(ENGLISH))).into_response();
    response.extensions_mut().insert(copy);
    response
}

pub fn invalid_config(detail: Wording) -> Response {
    error(
        StatusCode::BAD_REQUEST,
        Wording::Catalog(
            "journal.window.backup.response.backup.refusal.that.setting.couldn.t.be.saved",
        ),
        "invalid_config_value",
        detail,
    )
}

pub fn missing(detail: Wording) -> Response {
    error(
        StatusCode::BAD_REQUEST,
        Wording::Catalog(
            "journal.window.backup.response.backup.refusal.a.required.field.is.missing",
        ),
        "missing_required_field",
        detail,
    )
}
