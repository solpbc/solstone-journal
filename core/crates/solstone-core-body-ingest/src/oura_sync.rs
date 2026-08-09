// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;
use std::thread;
use std::time::Duration;

use chrono::{Days, NaiveDate, Utc};
use serde_json::{Map, Value, json};
use solstone_core_body_source::BodyRawRetention;
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};
use solstone_core_journal_io::{
    FileLock, JsonWriteOptions, LockOptions, create_directory_with_mode, hold_lock, write_json,
};

use crate::approval::{oura_approval, pin_journal_target};
use crate::bounded_file::read_bounded_regular;
use crate::bundle::{BodyIngestError, BodyIngestErrorKind};
use crate::oura::{
    OURA_SYNC_ENDPOINTS, OuraDocuments, OuraImportOptions, normalize_oura_documents, save_documents,
};

const API_BASE: &str = "https://api.ouraring.com/v2/usercollection";
const TOKEN_URL: &str = "https://api.ouraring.com/oauth/token";
const CURSOR_SCHEMA: &str = "solstone.import_sync.oura.v1";
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PAGES: usize = 100;
const MAX_TOTAL_RESPONSE_BYTES: usize = 128 * 1024 * 1024;
const MAX_TOTAL_ITEMS: usize = 100_000;
const MAX_TOTAL_PAGES: usize = 5_000;
const MAX_CURSOR_BYTES: usize = 1_048_576;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OuraSyncOptions {
    pub save: bool,
    pub confirm_body_save: bool,
    pub scheduled: bool,
    pub window_days: Option<u64>,
    pub today: Option<NaiveDate>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OuraEndpointIssueKind {
    Permission,
}

impl OuraEndpointIssueKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OuraEndpointIssue {
    endpoint: String,
    kind: OuraEndpointIssueKind,
}

impl OuraEndpointIssue {
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn kind(&self) -> OuraEndpointIssueKind {
        self.kind
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OuraSyncReport {
    bundle_id: Option<String>,
    rows: u64,
    days: Vec<String>,
    pages: u64,
    quiet_run: bool,
    dry_run: bool,
    endpoint_counts: BTreeMap<String, u64>,
    issues: Vec<OuraEndpointIssue>,
}

impl OuraSyncReport {
    pub fn bundle_id(&self) -> Option<&str> {
        self.bundle_id.as_deref()
    }

    pub fn rows(&self) -> u64 {
        self.rows
    }

    pub fn days(&self) -> &[String] {
        &self.days
    }

    pub fn pages(&self) -> u64 {
        self.pages
    }

    pub fn quiet_run(&self) -> bool {
        self.quiet_run
    }

    pub fn dry_run(&self) -> bool {
        self.dry_run
    }

    pub fn endpoint_counts(&self) -> &BTreeMap<String, u64> {
        &self.endpoint_counts
    }

    pub fn issues(&self) -> &[OuraEndpointIssue] {
        &self.issues
    }
}

struct HttpResponse {
    status: u16,
    body: String,
}

struct FetchBudget {
    bytes: usize,
    items: usize,
    pages: usize,
    max_bytes: usize,
    max_items: usize,
    max_pages: usize,
}

impl FetchBudget {
    fn production() -> Self {
        Self {
            bytes: 0,
            items: 0,
            pages: 0,
            max_bytes: MAX_TOTAL_RESPONSE_BYTES,
            max_items: MAX_TOTAL_ITEMS,
            max_pages: MAX_TOTAL_PAGES,
        }
    }

    fn add_response(&mut self, bytes: usize) -> Result<(), BodyIngestError> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|total| *total <= self.max_bytes)
            .ok_or_else(|| failure(BodyIngestErrorKind::Source, "response_budget"))?;
        Ok(())
    }

    fn add_page(&mut self, items: usize) -> Result<(), BodyIngestError> {
        self.pages = self
            .pages
            .checked_add(1)
            .filter(|total| *total <= self.max_pages)
            .ok_or_else(|| failure(BodyIngestErrorKind::Source, "page_budget"))?;
        self.items = self
            .items
            .checked_add(items)
            .filter(|total| *total <= self.max_items)
            .ok_or_else(|| failure(BodyIngestErrorKind::Source, "item_budget"))?;
        Ok(())
    }
}

trait OuraHttp {
    fn get(
        &mut self,
        endpoint: &str,
        query: &BTreeMap<String, String>,
        authorization: &str,
    ) -> Result<HttpResponse, BodyIngestError>;

    fn post_form(
        &mut self,
        url: &str,
        form: &BTreeMap<String, String>,
    ) -> Result<HttpResponse, BodyIngestError>;

    fn backoff(&mut self, attempt: u32) {
        thread::sleep(Duration::from_secs(1_u64 << attempt.min(4)));
    }
}

struct LiveOuraHttp {
    agent: ureq::Agent,
}

impl LiveOuraHttp {
    fn new() -> Self {
        let timeout = Duration::from_secs(30);
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(timeout))
            .timeout_recv_response(Some(timeout))
            .timeout_recv_body(Some(timeout))
            .timeout_global(Some(Duration::from_secs(90)))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }

    fn response(
        response: ureq::http::Response<ureq::Body>,
    ) -> Result<HttpResponse, BodyIngestError> {
        let status = response.status().as_u16();
        let mut reader = response
            .into_body()
            .into_reader()
            .take(MAX_RESPONSE_BYTES + 1);
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .map_err(|_| failure(BodyIngestErrorKind::Source, "http_read"))?;
        if bytes.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(failure(
                BodyIngestErrorKind::Source,
                "http_response_too_large",
            ));
        }
        let body = String::from_utf8(bytes)
            .map_err(|_| failure(BodyIngestErrorKind::Source, "http_utf8"))?;
        Ok(HttpResponse { status, body })
    }
}

impl OuraHttp for LiveOuraHttp {
    fn get(
        &mut self,
        endpoint: &str,
        query: &BTreeMap<String, String>,
        authorization: &str,
    ) -> Result<HttpResponse, BodyIngestError> {
        let url = format!("{API_BASE}/{endpoint}?{}", encode_form(query));
        let response = self
            .agent
            .get(&url)
            .header("Authorization", authorization)
            .header("Accept", "application/json")
            .call()
            .map_err(|_| failure(BodyIngestErrorKind::Source, "http_get"))?;
        Self::response(response)
    }

    fn post_form(
        &mut self,
        url: &str,
        form: &BTreeMap<String, String>,
    ) -> Result<HttpResponse, BodyIngestError> {
        let response = self
            .agent
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(encode_form(form))
            .map_err(|_| failure(BodyIngestErrorKind::Source, "http_post"))?;
        Self::response(response)
    }
}

pub(crate) fn post_oura_token_form(
    form: &BTreeMap<String, String>,
) -> Result<Value, BodyIngestError> {
    let mut http = LiveOuraHttp::new();
    let response = http.post_form(TOKEN_URL, form)?;
    if response.status != 200 {
        return Err(failure(
            BodyIngestErrorKind::Source,
            "authorization_exchange",
        ));
    }
    serde_json::from_str(&response.body)
        .map_err(|_| failure(BodyIngestErrorKind::Source, "authorization_response"))
}

#[derive(Clone)]
struct OuraSettings {
    client_id: String,
    client_secret: Option<String>,
    access_token: String,
    refresh_token: String,
    token_type: String,
    timezone: String,
}

pub fn sync_oura(
    journal: &Path,
    options: &OuraSyncOptions,
) -> Result<OuraSyncReport, BodyIngestError> {
    let mut http = LiveOuraHttp::new();
    sync_with_http(journal, options, &mut http)
}

fn sync_with_http(
    journal: &Path,
    options: &OuraSyncOptions,
    http: &mut dyn OuraHttp,
) -> Result<OuraSyncReport, BodyIngestError> {
    sync_with_http_before_lock(journal, options, http, &mut || {})
}

fn sync_with_http_before_lock(
    journal: &Path,
    options: &OuraSyncOptions,
    http: &mut dyn OuraHttp,
    before_lock: &mut dyn FnMut(),
) -> Result<OuraSyncReport, BodyIngestError> {
    let journal = pin_journal_target(journal)?;
    let journal = journal.as_path();
    if options.window_days == Some(0) {
        return Err(failure(BodyIngestErrorKind::Source, "window_days"));
    }
    if options.save {
        oura_approval(journal, options.confirm_body_save, options.scheduled)?;
    }
    if options.save {
        before_lock();
        let _lock = hold_oura_lock(journal)?;
        let retention = Some(oura_approval(
            journal,
            options.confirm_body_save,
            options.scheduled,
        )?);
        let settings = settings(journal)?;
        return sync_locked(journal, options, retention, settings, http);
    }
    let settings = settings(journal)?;
    sync_locked(journal, options, None, settings, http)
}

pub(crate) fn hold_oura_lock(journal: &Path) -> Result<FileLock, BodyIngestError> {
    create_directory_with_mode(&journal.join("imports"), 0o700)
        .map_err(|_| failure(BodyIngestErrorKind::Publication, "sync_lock"))?;
    hold_lock(
        journal.join("imports/oura.json"),
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|_| failure(BodyIngestErrorKind::Publication, "sync_lock"))
}

fn sync_locked(
    journal: &Path,
    options: &OuraSyncOptions,
    retention: Option<BodyRawRetention>,
    mut settings: OuraSettings,
    http: &mut dyn OuraHttp,
) -> Result<OuraSyncReport, BodyIngestError> {
    let cursor = read_cursor(journal)?;
    let today = options.today.unwrap_or_else(|| Utc::now().date_naive());
    let mut documents = OuraDocuments::new();
    let mut endpoint_counts = BTreeMap::new();
    let mut issues = Vec::new();
    let mut fetched = BTreeSet::new();
    let mut budget = FetchBudget::production();
    let mut refreshed = false;
    for endpoint in OURA_SYNC_ENDPOINTS {
        let (start, end) = window(&cursor, endpoint, today, options.window_days)?;
        let mut items = Vec::new();
        let mut pages = Vec::new();
        let mut endpoint_ok = true;
        for (chunk_start, chunk_end) in chunks(endpoint, start, end)? {
            let mut next_token: Option<String> = None;
            for page_index in 0..MAX_PAGES {
                let mut query = endpoint_query(endpoint, chunk_start, chunk_end);
                if let Some(token) = &next_token {
                    query.insert("next_token".to_owned(), token.clone());
                }
                let response = request_with_retry(
                    http,
                    endpoint,
                    &query,
                    &mut settings,
                    &mut refreshed,
                    journal,
                    options.save,
                )?;
                budget.add_response(response.body.len())?;
                match classify_response(endpoint, &response)? {
                    ResponseClass::Page(page) => {
                        let data = page
                            .get("data")
                            .and_then(Value::as_array)
                            .ok_or_else(|| failure(BodyIngestErrorKind::Source, "api_data"))?;
                        validate_api_items(endpoint, data)?;
                        budget.add_page(data.len())?;
                        items.extend(data.iter().cloned());
                        pages.push(Value::Object(page.clone()));
                        next_token = page
                            .get("next_token")
                            .and_then(Value::as_str)
                            .filter(|token| !token.is_empty())
                            .map(str::to_owned);
                        if next_token.is_none() {
                            break;
                        }
                        if page_index + 1 == MAX_PAGES {
                            return Err(failure(BodyIngestErrorKind::Source, "pagination_limit"));
                        }
                    }
                    ResponseClass::Permission => {
                        issues.push(OuraEndpointIssue {
                            endpoint: endpoint.to_owned(),
                            kind: OuraEndpointIssueKind::Permission,
                        });
                        endpoint_ok = false;
                        break;
                    }
                }
            }
            if !endpoint_ok {
                break;
            }
        }
        if endpoint_ok {
            fetched.insert(endpoint.to_owned());
        }
        endpoint_counts.insert(endpoint.to_owned(), items.len() as u64);
        documents.insert(endpoint.to_owned(), items, pages);
    }

    let rows = normalize_oura_documents(&documents, &settings.timezone)?;
    let page_count = u64::try_from(budget.pages)
        .map_err(|_| failure(BodyIngestErrorKind::Source, "page_budget"))?;
    let days = rows
        .iter()
        .filter_map(|row| {
            row.row()
                .get("day")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if !options.save {
        return Ok(OuraSyncReport {
            bundle_id: None,
            rows: rows.len() as u64,
            days,
            pages: page_count,
            quiet_run: false,
            dry_run: true,
            endpoint_counts,
            issues,
        });
    }

    if rows.is_empty() {
        write_cursor(
            journal, &cursor, &documents, &fetched, None, 0, true, page_count,
        )?;
        return Ok(OuraSyncReport {
            bundle_id: None,
            rows: 0,
            days,
            pages: page_count,
            quiet_run: true,
            dry_run: false,
            endpoint_counts,
            issues,
        });
    }

    let import = save_documents(
        journal,
        &documents,
        retention.expect("save mode has a checked retention"),
        &OuraImportOptions {
            timezone: settings.timezone,
            confirm_body_save: true,
            scheduled: options.scheduled,
            force: false,
        },
    )?;
    let published_bundle = (!import.skipped()).then(|| import.bundle_id()).flatten();
    write_cursor(
        journal,
        &cursor,
        &documents,
        &fetched,
        published_bundle,
        import.rows(),
        import.skipped(),
        page_count,
    )?;
    Ok(OuraSyncReport {
        bundle_id: published_bundle.map(str::to_owned),
        rows: import.rows(),
        days: import.days().to_vec(),
        pages: page_count,
        quiet_run: import.skipped(),
        dry_run: false,
        endpoint_counts,
        issues,
    })
}

fn settings(journal: &Path) -> Result<OuraSettings, BodyIngestError> {
    let read = read_journal_config(journal)
        .map_err(|_| failure(BodyIngestErrorKind::Source, "journal_config"))?;
    let config = read
        .config
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "journal_config_missing"))?;
    let section = config
        .get("oura")
        .and_then(Value::as_object)
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "oura_config_missing"))?;
    let tokens = section
        .get("tokens")
        .and_then(Value::as_object)
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "authorization_needed"))?;
    let required = |object: &Map<String, Value>, key: &str, stage| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| failure(BodyIngestErrorKind::Source, stage))
    };
    let timezone = config
        .get("identity")
        .and_then(Value::as_object)
        .and_then(|identity| identity.get("timezone"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("UTC")
        .to_owned();
    Ok(OuraSettings {
        client_id: required(section, "client_id", "client_id_missing")?,
        client_secret: section
            .get("client_secret")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        access_token: required(tokens, "access_token", "tokens_invalid")?,
        refresh_token: required(tokens, "refresh_token", "tokens_invalid")?,
        token_type: tokens
            .get("token_type")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("Bearer")
            .to_owned(),
        timezone,
    })
}

fn request_with_retry(
    http: &mut dyn OuraHttp,
    endpoint: &str,
    query: &BTreeMap<String, String>,
    settings: &mut OuraSettings,
    refreshed: &mut bool,
    journal: &Path,
    persist_refresh: bool,
) -> Result<HttpResponse, BodyIngestError> {
    let mut retryable = 0_u32;
    loop {
        let authorization = format!("{} {}", settings.token_type, settings.access_token);
        let response = http.get(endpoint, query, &authorization)?;
        if response.status == 401 && !*refreshed {
            if !persist_refresh {
                return Err(failure(
                    BodyIngestErrorKind::Source,
                    "authorization_refresh_requires_save",
                ));
            }
            refresh(http, settings, journal, persist_refresh)?;
            *refreshed = true;
            continue;
        }
        if response.status == 429 || (500..=599).contains(&response.status) {
            retryable += 1;
            if retryable >= 3 {
                return Err(failure(BodyIngestErrorKind::Source, "api_retry_exhausted"));
            }
            http.backoff(retryable - 1);
            continue;
        }
        return Ok(response);
    }
}

#[derive(Debug)]
enum ResponseClass {
    Page(Map<String, Value>),
    Permission,
}

fn classify_response(
    _endpoint: &str,
    response: &HttpResponse,
) -> Result<ResponseClass, BodyIngestError> {
    match response.status {
        200 => {
            let page: Value = serde_json::from_str(&response.body)
                .map_err(|_| failure(BodyIngestErrorKind::Source, "api_json"))?;
            page.as_object()
                .cloned()
                .map(ResponseClass::Page)
                .ok_or_else(|| failure(BodyIngestErrorKind::Source, "api_object"))
        }
        401 => Ok(ResponseClass::Permission),
        403 => {
            let lower = response.body.to_ascii_lowercase();
            if ["membership", "subscription", "not active", "expired member"]
                .iter()
                .any(|needle| lower.contains(needle))
            {
                Err(failure(BodyIngestErrorKind::Source, "membership_lost"))
            } else if ["scope", "permission", "not authorized", "access denied"]
                .iter()
                .any(|needle| lower.contains(needle))
            {
                Ok(ResponseClass::Permission)
            } else {
                Err(failure(BodyIngestErrorKind::Source, "unknown_forbidden"))
            }
        }
        _ => Err(failure(BodyIngestErrorKind::Source, "api_status")),
    }
}

fn refresh(
    http: &mut dyn OuraHttp,
    settings: &mut OuraSettings,
    journal: &Path,
    persist: bool,
) -> Result<(), BodyIngestError> {
    let mut form = BTreeMap::from([
        ("grant_type".to_owned(), "refresh_token".to_owned()),
        ("refresh_token".to_owned(), settings.refresh_token.clone()),
        ("client_id".to_owned(), settings.client_id.clone()),
    ]);
    if let Some(secret) = &settings.client_secret {
        form.insert("client_secret".to_owned(), secret.clone());
    }
    let response = http.post_form(TOKEN_URL, &form)?;
    if response.status != 200 {
        return Err(failure(
            BodyIngestErrorKind::Source,
            "authorization_refresh",
        ));
    }
    let payload: Value = serde_json::from_str(&response.body)
        .map_err(|_| failure(BodyIngestErrorKind::Source, "authorization_response"))?;
    let object = payload
        .as_object()
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "authorization_response"))?;
    let access_token = token_field(object, "access_token")?;
    let refresh_token = token_field(object, "refresh_token")?;
    let token_type = object
        .get("token_type")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("Bearer")
        .to_owned();
    let expires_at = object
        .get("expires_at")
        .and_then(Value::as_f64)
        .or_else(|| {
            object
                .get("expires_in")
                .and_then(Value::as_f64)
                .map(|seconds| Utc::now().timestamp() as f64 + seconds)
        })
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "authorization_response"))?;
    if persist {
        let next_access = access_token.clone();
        let next_refresh = refresh_token.clone();
        let next_type = token_type.clone();
        mutate_journal_config(journal, move |config| {
            let section = config
                .entry("oura".to_owned())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .expect("checked settings preserve an object section");
            let next = json!({
                "access_token": next_access,
                "refresh_token": next_refresh,
                "expires_at": expires_at,
                "token_type": next_type,
            });
            let changed = section.get("tokens") != Some(&next);
            section.insert("tokens".to_owned(), next);
            JournalConfigMutation { changed, value: () }
        })
        .map_err(|_| failure(BodyIngestErrorKind::Publication, "token_store"))?;
    }
    settings.access_token = access_token;
    settings.refresh_token = refresh_token;
    settings.token_type = token_type;
    Ok(())
}

fn token_field(object: &Map<String, Value>, key: &str) -> Result<String, BodyIngestError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "authorization_response"))
}

fn validate_api_items(endpoint: &str, data: &[Value]) -> Result<(), BodyIngestError> {
    for item in data {
        let object = item
            .as_object()
            .ok_or_else(|| failure(BodyIngestErrorKind::Source, "api_item"))?;
        if endpoint == "heartrate" {
            if object.get("timestamp").and_then(Value::as_str).is_none()
                || object.get("bpm").is_none_or(Value::is_null)
            {
                return Err(failure(BodyIngestErrorKind::Source, "api_item"));
            }
        } else {
            let day = if endpoint == "enhanced_tag" {
                "start_day"
            } else {
                "day"
            };
            if object.get("id").and_then(Value::as_str).is_none()
                || object.get(day).and_then(Value::as_str).is_none()
            {
                return Err(failure(BodyIngestErrorKind::Source, "api_item"));
            }
        }
    }
    Ok(())
}

fn endpoint_query(endpoint: &str, start: NaiveDate, end: NaiveDate) -> BTreeMap<String, String> {
    if endpoint == "heartrate" {
        BTreeMap::from([
            (
                "start_datetime".to_owned(),
                format!("{}T00:00:00+00:00", start.format("%Y-%m-%d")),
            ),
            (
                "end_datetime".to_owned(),
                format!("{}T23:59:59+00:00", end.format("%Y-%m-%d")),
            ),
        ])
    } else {
        BTreeMap::from([
            (
                "start_date".to_owned(),
                start.format("%Y-%m-%d").to_string(),
            ),
            ("end_date".to_owned(), end.format("%Y-%m-%d").to_string()),
        ])
    }
}

fn chunks(
    endpoint: &str,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<(NaiveDate, NaiveDate)>, BodyIngestError> {
    let limit = if endpoint == "heartrate" { 31 } else { 364 };
    let mut cursor = start;
    let mut chunks = Vec::new();
    while cursor <= end {
        let chunk_end = cursor
            .checked_add_days(Days::new(limit - 1))
            .map(|candidate| candidate.min(end))
            .ok_or_else(|| failure(BodyIngestErrorKind::Source, "window"))?;
        chunks.push((cursor, chunk_end));
        cursor = chunk_end
            .checked_add_days(Days::new(1))
            .ok_or_else(|| failure(BodyIngestErrorKind::Source, "window"))?;
    }
    Ok(chunks)
}

fn window(
    cursor: &Option<Map<String, Value>>,
    endpoint: &str,
    today: NaiveDate,
    explicit_days: Option<u64>,
) -> Result<(NaiveDate, NaiveDate), BodyIngestError> {
    let start = if let Some(days) = explicit_days {
        today.checked_sub_days(Days::new(days))
    } else if let Some(watermark) = watermark(cursor, endpoint)? {
        let next = watermark.checked_add_days(Days::new(1));
        let trailing = today.checked_sub_days(Days::new(7));
        next.zip(trailing)
            .map(|(next, trailing)| next.min(trailing))
    } else if cursor.is_some() && !backfill_complete(cursor, endpoint) {
        NaiveDate::from_ymd_opt(2015, 1, 1)
    } else {
        today.checked_sub_days(Days::new(30))
    }
    .ok_or_else(|| failure(BodyIngestErrorKind::Source, "window"))?;
    Ok((start.min(today), today))
}

fn read_cursor(journal: &Path) -> Result<Option<Map<String, Value>>, BodyIngestError> {
    let bytes = match read_bounded_regular(journal, "imports/oura.json", MAX_CURSOR_BYTES) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(failure(BodyIngestErrorKind::Source, "cursor_read")),
    };
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| failure(BodyIngestErrorKind::Source, "cursor_invalid"))?;
    let object = value
        .as_object()
        .cloned()
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "cursor_invalid"))?;
    if object.get("schema").and_then(Value::as_str) != Some(CURSOR_SCHEMA) {
        return Err(failure(BodyIngestErrorKind::Source, "cursor_schema"));
    }
    validate_cursor_endpoints(&object)?;
    Ok(Some(object))
}

fn validate_cursor_endpoints(cursor: &Map<String, Value>) -> Result<(), BodyIngestError> {
    let Some(endpoints) = cursor.get("endpoints") else {
        return Ok(());
    };
    let endpoints = endpoints
        .as_object()
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "cursor_invalid"))?;
    for name in OURA_SYNC_ENDPOINTS {
        let Some(endpoint) = endpoints.get(name) else {
            continue;
        };
        let endpoint = endpoint
            .as_object()
            .ok_or_else(|| failure(BodyIngestErrorKind::Source, "cursor_invalid"))?;
        match endpoint.get("high_water_day") {
            None | Some(Value::Null) => {}
            Some(Value::String(day)) => {
                NaiveDate::parse_from_str(day, "%Y-%m-%d")
                    .map_err(|_| failure(BodyIngestErrorKind::Source, "cursor_watermark"))?;
            }
            Some(_) => return Err(failure(BodyIngestErrorKind::Source, "cursor_invalid")),
        }
        if endpoint
            .get("backfill_complete")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(failure(BodyIngestErrorKind::Source, "cursor_invalid"));
        }
    }
    Ok(())
}

fn watermark(
    cursor: &Option<Map<String, Value>>,
    endpoint: &str,
) -> Result<Option<NaiveDate>, BodyIngestError> {
    let Some(value) = cursor
        .as_ref()
        .and_then(|cursor| cursor.get("endpoints"))
        .and_then(Value::as_object)
        .and_then(|endpoints| endpoints.get(endpoint))
        .and_then(Value::as_object)
        .and_then(|endpoint| endpoint.get("high_water_day"))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(Some)
        .map_err(|_| failure(BodyIngestErrorKind::Source, "cursor_watermark"))
}

fn backfill_complete(cursor: &Option<Map<String, Value>>, endpoint: &str) -> bool {
    cursor
        .as_ref()
        .and_then(|cursor| cursor.get("endpoints"))
        .and_then(Value::as_object)
        .and_then(|endpoints| endpoints.get(endpoint))
        .and_then(Value::as_object)
        .and_then(|endpoint| endpoint.get("backfill_complete"))
        == Some(&Value::Bool(true))
}

#[allow(clippy::too_many_arguments)]
fn write_cursor(
    journal: &Path,
    previous: &Option<Map<String, Value>>,
    documents: &OuraDocuments,
    fetched: &BTreeSet<String>,
    import_id: Option<&str>,
    rows: u64,
    quiet_run: bool,
    pages: u64,
) -> Result<(), BodyIngestError> {
    let mut endpoints = Map::new();
    for endpoint in OURA_SYNC_ENDPOINTS {
        let prior = watermark(previous, endpoint)?;
        let day_field = if endpoint == "enhanced_tag" {
            "start_day"
        } else {
            "day"
        };
        let next = documents
            .endpoint_items(endpoint)
            .iter()
            .filter_map(|item| {
                let object = item.as_object()?;
                let value = if endpoint == "heartrate" {
                    object.get("timestamp")?.as_str()?.get(..10)?
                } else {
                    object.get(day_field)?.as_str()?
                };
                NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
            })
            .chain(prior)
            .max();
        endpoints.insert(
            endpoint.to_owned(),
            json!({
                "high_water_day": next.map(|day| day.format("%Y-%m-%d").to_string()),
                "backfill_complete": fetched.contains(endpoint)
                    || backfill_complete(previous, endpoint),
                "next_token": null,
            }),
        );
    }
    write_json(
        journal.join("imports/oura.json"),
        &json!({
            "schema": CURSOR_SCHEMA,
            "last_sync": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "trailing_refetch_days": 7,
            "endpoints": endpoints,
            "last_result": {
                "import_id": import_id,
                "quiet_run": quiet_run,
                "rows": rows,
                "pages": pages,
            }
        }),
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    )
    .map_err(|_| failure(BodyIngestErrorKind::Publication, "cursor_write"))
}

fn encode_form(values: &BTreeMap<String, String>) -> String {
    values
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else if byte == b' ' {
            encoded.push('+');
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

const fn failure(kind: BodyIngestErrorKind, stage: &'static str) -> BodyIngestError {
    BodyIngestError::new(kind, stage)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = env::temp_dir().join(format!("solstone-oura-sync-{stamp}"));
            fs::create_dir(&path).expect("temporary directory");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Default)]
    struct FakeHttp {
        gets: VecDeque<HttpResponse>,
        posts: VecDeque<HttpResponse>,
        calls: Vec<(String, BTreeMap<String, String>, String)>,
        post_calls: Vec<(String, BTreeMap<String, String>)>,
        backoffs: Vec<u32>,
    }

    impl OuraHttp for FakeHttp {
        fn get(
            &mut self,
            endpoint: &str,
            query: &BTreeMap<String, String>,
            authorization: &str,
        ) -> Result<HttpResponse, BodyIngestError> {
            self.calls
                .push((endpoint.to_owned(), query.clone(), authorization.to_owned()));
            self.gets
                .pop_front()
                .ok_or_else(|| failure(BodyIngestErrorKind::Source, "test_get"))
        }

        fn post_form(
            &mut self,
            url: &str,
            form: &BTreeMap<String, String>,
        ) -> Result<HttpResponse, BodyIngestError> {
            self.post_calls.push((url.to_owned(), form.clone()));
            self.posts
                .pop_front()
                .ok_or_else(|| failure(BodyIngestErrorKind::Source, "test_post"))
        }

        fn backoff(&mut self, attempt: u32) {
            self.backoffs.push(attempt);
        }
    }

    fn response(status: u16, body: Value) -> HttpResponse {
        HttpResponse {
            status,
            body: serde_json::to_string(&body).expect("response JSON"),
        }
    }

    fn empty_pages() -> VecDeque<HttpResponse> {
        OURA_SYNC_ENDPOINTS
            .iter()
            .map(|_| response(200, json!({"data": [], "next_token": null})))
            .collect()
    }

    fn one_readiness_pages() -> VecDeque<HttpResponse> {
        let mut pages = empty_pages();
        pages[0] = response(
            200,
            json!({
                "data": [{"id": "ready-1", "day": "2026-01-02", "score": 81}],
                "next_token": null
            }),
        );
        pages
    }

    fn journal() -> TempDir {
        let temporary = TempDir::new();
        fs::create_dir_all(temporary.0.join("config")).expect("config directory");
        fs::write(
            temporary.0.join("config/journal.json"),
            serde_json::to_vec(&json!({
                "identity": {"timezone": "America/Denver"},
                "oura": {
                    "client_id": "synthetic-client",
                    "tokens": {
                        "access_token": "synthetic-access",
                        "refresh_token": "synthetic-refresh",
                        "token_type": "Bearer",
                        "expires_at": 4102444800.0
                    }
                }
            }))
            .expect("journal config"),
        )
        .expect("write journal config");
        temporary
    }

    fn approve(journal: &Path) {
        fs::create_dir_all(journal.join("imports/_approvals")).expect("approvals directory");
        fs::write(
            journal.join("imports/_approvals/oura_sync_preflight.json"),
            serde_json::to_vec(&json!({
                "schema": "solstone.oura_sync_preflight.v1",
                "checklist_version": "solstone.oura_sync_preflight.checklist.v2",
                "journal_root": journal.canonicalize().expect("journal root"),
                "requires_per_run_confirmation": true,
                "replication_destinations": {
                    "time_machine": {"decision": "excluded"},
                    "icloud": {"decision": "excluded"},
                    "solbase": {"decision": "excluded"},
                    "hosted_backup": {"decision": "excluded"},
                    "other": {"decision": "excluded"}
                },
                "raw_retention": {"decision": "discard"}
            }))
            .expect("approval"),
        )
        .expect("write approval");
    }

    fn options(save: bool) -> OuraSyncOptions {
        OuraSyncOptions {
            save,
            confirm_body_save: save,
            today: NaiveDate::from_ymd_opt(2026, 1, 3),
            ..OuraSyncOptions::default()
        }
    }

    #[test]
    fn dry_run_fetches_every_endpoint_without_writing_and_reports_permission_gaps() {
        let journal = journal();
        let mut pages = empty_pages();
        pages[0] = response(403, json!({"error": "missing permission scope"}));
        let mut http = FakeHttp {
            gets: pages,
            ..FakeHttp::default()
        };
        let report = sync_with_http(&journal.0, &options(false), &mut http).expect("dry run");
        assert!(report.dry_run());
        assert_eq!(report.rows(), 0);
        assert_eq!(report.issues().len(), 1);
        assert_eq!(report.issues()[0].endpoint(), "daily_readiness");
        assert_eq!(report.issues()[0].kind(), OuraEndpointIssueKind::Permission);
        assert_eq!(http.calls.len(), OURA_SYNC_ENDPOINTS.len());
        assert!(
            http.calls
                .iter()
                .all(|(_, _, authorization)| authorization == "Bearer synthetic-access")
        );
        assert!(!journal.0.join("imports/oura.json").exists());
        assert!(!journal.0.join("imports").exists());
    }

    #[test]
    fn save_gate_runs_before_transport_or_lock_and_unknown_forbidden_fails_closed() {
        let journal = journal();
        let mut http = FakeHttp {
            gets: empty_pages(),
            ..FakeHttp::default()
        };
        let error = sync_with_http(&journal.0, &options(true), &mut http).unwrap_err();
        assert_eq!(error.kind(), BodyIngestErrorKind::Gate);
        assert!(http.calls.is_empty());
        assert!(!journal.0.join("imports/oura.json.lock").exists());

        assert_eq!(
            classify_response(
                "daily_sleep",
                &response(403, json!({"error": "opaque forbidden"}))
            )
            .unwrap_err()
            .stage(),
            "unknown_forbidden"
        );
        assert_eq!(
            classify_response(
                "daily_sleep",
                &response(403, json!({"error": "membership expired"}))
            )
            .unwrap_err()
            .stage(),
            "membership_lost"
        );
    }

    #[test]
    fn dry_run_refuses_rotating_refresh_without_consuming_the_grant() {
        let journal = journal();
        let pages = VecDeque::from([response(401, json!({"error": "expired"}))]);
        let mut http = FakeHttp {
            gets: pages,
            posts: VecDeque::from([response(
                200,
                json!({
                    "access_token": "refreshed-access",
                    "refresh_token": "refreshed-refresh",
                    "token_type": "Synthetic",
                    "expires_in": 3600
                }),
            )]),
            ..FakeHttp::default()
        };
        let original = fs::read(journal.0.join("config/journal.json")).expect("original config");
        let failure = sync_with_http(&journal.0, &options(false), &mut http)
            .expect_err("dry run must not consume a rotating refresh grant");
        assert_eq!(failure.kind(), BodyIngestErrorKind::Source);
        assert_eq!(failure.stage(), "authorization_refresh_requires_save");
        assert!(http.post_calls.is_empty());
        assert_eq!(http.calls[0].2, "Bearer synthetic-access");
        assert_eq!(
            fs::read(journal.0.join("config/journal.json")).expect("config after dry run"),
            original
        );
    }

    #[test]
    fn save_persists_rotated_refresh_token_before_retrying_the_request() {
        let journal = journal();
        approve(&journal.0);
        let mut gets = one_readiness_pages();
        gets.push_front(response(401, json!({"error": "expired"})));
        let mut http = FakeHttp {
            gets,
            posts: VecDeque::from([response(
                200,
                json!({
                    "access_token": "refreshed-access",
                    "refresh_token": "refreshed-refresh",
                    "token_type": "Synthetic",
                    "expires_in": 3600
                }),
            )]),
            ..FakeHttp::default()
        };
        sync_with_http(&journal.0, &options(true), &mut http).expect("save after refresh");
        assert_eq!(http.post_calls.len(), 1);
        assert_eq!(http.calls[0].2, "Bearer synthetic-access");
        assert_eq!(http.calls[1].2, "Synthetic refreshed-access");
        let config: Value = serde_json::from_slice(
            &fs::read(journal.0.join("config/journal.json")).expect("refreshed config"),
        )
        .expect("parse refreshed config");
        assert_eq!(
            config["oura"]["tokens"]["refresh_token"],
            "refreshed-refresh"
        );
    }

    #[test]
    fn waiting_save_reads_tokens_only_after_acquiring_the_shared_lock() {
        use std::sync::mpsc;

        let journal = journal();
        approve(&journal.0);
        let held = hold_oura_lock(&journal.0).expect("hold shared Oura lock");
        let path = journal.0.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let mut http = FakeHttp {
                gets: empty_pages(),
                ..FakeHttp::default()
            };
            let result = sync_with_http_before_lock(&path, &options(true), &mut http, &mut || {
                ready_tx.send(()).expect("signal lock wait")
            });
            (result, http)
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("waiter reaches lock boundary");

        let config_path = journal.0.join("config/journal.json");
        let mut config: Value =
            serde_json::from_slice(&fs::read(&config_path).expect("read original token config"))
                .expect("parse original token config");
        config["oura"]["tokens"]["access_token"] = Value::String("new-access".to_owned());
        config["oura"]["tokens"]["refresh_token"] = Value::String("new-refresh".to_owned());
        fs::write(
            &config_path,
            serde_json::to_vec(&config).expect("serialize replacement token config"),
        )
        .expect("replace token config while lock is held");
        drop(held);

        let (result, http) = waiter.join().expect("join waiting save");
        result.expect("waiting save succeeds with replacement tokens");
        assert!(
            http.calls
                .iter()
                .all(|(_, _, authorization)| authorization == "Bearer new-access")
        );
    }

    #[cfg(unix)]
    #[test]
    fn sync_pins_the_approved_journal_before_a_symlink_can_be_retargeted() {
        use std::os::unix::fs::symlink;

        let approved = journal();
        approve(&approved.0);
        let holder = TempDir::new();
        let alternate = holder.0.join("alternate-journal");
        let selected = holder.0.join("selected-journal");
        fs::create_dir(&alternate).expect("create alternate journal");
        symlink(&approved.0, &selected).expect("link selected journal");
        let mut http = FakeHttp {
            gets: one_readiness_pages(),
            ..FakeHttp::default()
        };

        let report = sync_with_http_before_lock(&selected, &options(true), &mut http, &mut || {
            fs::remove_file(&selected).expect("remove selected link");
            symlink(&alternate, &selected).expect("retarget selected journal");
        })
        .expect("sync continues against its approved pinned journal");

        assert!(report.bundle_id().is_some());
        assert!(approved.0.join("imports/oura.json").is_file());
        assert!(
            !alternate.join("imports").exists(),
            "retargeted journal must receive no lock, token, cursor, or body state"
        );
    }

    #[test]
    fn aggregate_fetch_budget_refuses_before_retaining_an_unbounded_sync() {
        let mut bytes = FetchBudget {
            bytes: 0,
            items: 0,
            pages: 0,
            max_bytes: 5,
            max_items: 3,
            max_pages: 2,
        };
        bytes.add_response(5).expect("inclusive byte boundary");
        assert_eq!(
            bytes.add_response(1).unwrap_err().stage(),
            "response_budget"
        );

        let mut pages = FetchBudget {
            bytes: 0,
            items: 0,
            pages: 0,
            max_bytes: 10,
            max_items: 3,
            max_pages: 2,
        };
        pages.add_page(2).expect("first page");
        pages.add_page(1).expect("inclusive item/page boundary");
        assert_eq!(pages.add_page(0).unwrap_err().stage(), "page_budget");

        let mut items = FetchBudget {
            bytes: 0,
            items: 0,
            pages: 0,
            max_bytes: 10,
            max_items: 3,
            max_pages: 3,
        };
        items.add_page(3).expect("inclusive item boundary");
        assert_eq!(items.add_page(1).unwrap_err().stage(), "item_budget");
    }

    #[test]
    fn save_publishes_native_bundle_then_cursor_and_identical_replay_is_quiet() {
        let journal = journal();
        approve(&journal.0);
        let mut first_http = FakeHttp {
            gets: one_readiness_pages(),
            ..FakeHttp::default()
        };
        let first = sync_with_http(&journal.0, &options(true), &mut first_http).expect("save");
        assert_eq!(first.rows(), 1);
        assert!(!first.quiet_run());
        let bundle_id = first.bundle_id().expect("bundle id");
        assert!(
            journal
                .0
                .join("imports")
                .join(bundle_id)
                .join("body-ledger.jsonl")
                .is_file()
        );
        let cursor: Value =
            serde_json::from_slice(&fs::read(journal.0.join("imports/oura.json")).expect("cursor"))
                .expect("cursor JSON");
        assert_eq!(cursor["last_result"]["import_id"], bundle_id);
        assert_eq!(cursor["last_result"]["rows"], 1);

        let mut second_http = FakeHttp {
            gets: one_readiness_pages(),
            ..FakeHttp::default()
        };
        let second =
            sync_with_http(&journal.0, &options(true), &mut second_http).expect("quiet save");
        assert!(second.quiet_run());
        assert_eq!(second.bundle_id(), None);
        let bundles = fs::read_dir(journal.0.join("imports"))
            .expect("imports")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("body-"))
            .count();
        assert_eq!(bundles, 1);
    }

    #[test]
    fn empty_save_writes_only_cursor_and_normalization_failure_cannot_advance_it() {
        let journal = journal();
        approve(&journal.0);
        let mut empty_http = FakeHttp {
            gets: empty_pages(),
            ..FakeHttp::default()
        };
        let report =
            sync_with_http(&journal.0, &options(true), &mut empty_http).expect("empty save");
        assert!(report.quiet_run());
        assert_eq!(report.bundle_id(), None);
        assert!(journal.0.join("imports/oura.json").is_file());
        assert_eq!(
            fs::read_dir(journal.0.join("imports"))
                .expect("imports")
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("body-"))
                .count(),
            0
        );

        fs::remove_file(journal.0.join("imports/oura.json")).expect("remove cursor");
        let mut config: Value = serde_json::from_slice(
            &fs::read(journal.0.join("config/journal.json")).expect("config"),
        )
        .expect("config JSON");
        config["identity"]["timezone"] = Value::String("not/a-timezone".to_owned());
        fs::write(
            journal.0.join("config/journal.json"),
            serde_json::to_vec(&config).expect("config bytes"),
        )
        .expect("bad timezone config");
        let mut bad_http = FakeHttp {
            gets: one_readiness_pages(),
            ..FakeHttp::default()
        };
        let error = sync_with_http(&journal.0, &options(true), &mut bad_http).unwrap_err();
        assert_eq!(error.kind(), BodyIngestErrorKind::Normalize);
        assert!(!journal.0.join("imports/oura.json").exists());
    }

    #[test]
    fn cursor_read_refuses_symlinks_fifos_and_oversized_documents() {
        use std::os::unix::fs::symlink;

        use nix::sys::stat::Mode;
        use nix::unistd::mkfifo;

        let journal = journal();
        fs::create_dir_all(journal.0.join("imports")).expect("imports directory");
        let cursor = journal.0.join("imports/oura.json");
        let outside = journal.0.join("outside-cursor.json");
        fs::write(&outside, b"{}").expect("outside cursor");
        symlink(&outside, &cursor).expect("cursor symlink");
        assert_eq!(read_cursor(&journal.0).unwrap_err().stage(), "cursor_read");

        fs::remove_file(&cursor).expect("remove cursor symlink");
        mkfifo(&cursor, Mode::S_IRUSR | Mode::S_IWUSR).expect("cursor fifo");
        assert_eq!(read_cursor(&journal.0).unwrap_err().stage(), "cursor_read");

        fs::remove_file(&cursor).expect("remove cursor fifo");
        fs::write(&cursor, vec![b'x'; MAX_CURSOR_BYTES + 1]).expect("oversized cursor");
        assert_eq!(read_cursor(&journal.0).unwrap_err().stage(), "cursor_read");
    }

    #[test]
    fn malformed_schema_valid_cursor_fails_before_http_or_mutation() {
        let journal = journal();
        approve(&journal.0);
        let cursor = journal.0.join("imports/oura.json");
        for (endpoints, stage) in [
            (json!([]), "cursor_invalid"),
            (json!({"daily_readiness": []}), "cursor_invalid"),
            (
                json!({"daily_readiness": {"high_water_day": 123}}),
                "cursor_invalid",
            ),
            (
                json!({"daily_readiness": {"high_water_day": "not-a-day"}}),
                "cursor_watermark",
            ),
            (
                json!({"daily_readiness": {"backfill_complete": "yes"}}),
                "cursor_invalid",
            ),
        ] {
            fs::write(
                &cursor,
                serde_json::to_vec(&json!({
                    "schema": CURSOR_SCHEMA,
                    "endpoints": endpoints,
                }))
                .expect("cursor matrix bytes"),
            )
            .expect("cursor matrix case");
            assert_eq!(read_cursor(&journal.0).unwrap_err().stage(), stage);
        }
        let malformed = serde_json::to_vec(&json!({
            "schema": CURSOR_SCHEMA,
            "endpoints": {
                "daily_readiness": {
                    "high_water_day": 123,
                    "backfill_complete": true
                }
            }
        }))
        .expect("malformed cursor bytes");
        fs::write(&cursor, &malformed).expect("malformed cursor");
        let config = fs::read(journal.0.join("config/journal.json")).expect("config before sync");
        let mut http = FakeHttp::default();

        let error = sync_with_http(&journal.0, &options(true), &mut http).unwrap_err();

        assert_eq!(error.stage(), "cursor_invalid");
        assert!(http.calls.is_empty());
        assert!(http.post_calls.is_empty());
        assert_eq!(fs::read(&cursor).expect("cursor after sync"), malformed);
        assert_eq!(
            fs::read(journal.0.join("config/journal.json")).expect("config after sync"),
            config
        );
        assert!(
            fs::read_dir(journal.0.join("imports"))
                .expect("imports")
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().starts_with("body-"))
        );
    }
}
