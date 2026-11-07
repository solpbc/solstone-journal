// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Asking Oura for a range of days, page by page, into a run's staging.
//!
//! The fetch is the first phase of an Oura run (`body_segments.rs`). It writes
//! the staged format the scan reads (`solstone-core-import-sources/src/oura_run.rs`):
//! each page lands whole under `pages/<endpoint>/<chunk>-<n>.json`, and only then
//! is it added to `pages/index.json`. A run killed while fetching carries on from
//! the page after the last one the index names.
//!
//! The transport, its retries on 429 and 5xx, the reading of a 403 and the token
//! refresh are the old sync's own (`solstone-core-body-ingest`'s `oura_sync`).
//! Tokens are the journal's. They are read from config when the fetch starts, and
//! a 401 is answered by taking the old sync's lock, reading them again, and either
//! using a refresh the other path just made or making the one refresh this run
//! may make. A 401 after that ends the run.
//!
//! What was asked, and how each endpoint ended, is kept in the run state, so a run
//! resumed on a later day asks for the same days and reports the same results.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Days, NaiveDate, TimeZone as _, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_body_ingest::oura_sync::{
    API_BASE, HttpResponse, LiveOuraHttp, OuraHttp, OuraSettings, ResponseClass, classify_response,
    encode_form, refresh, request_with_retry, settings,
};
use solstone_core_body_ingest::{BodyIngestError, BodyIngestErrorKind};
use solstone_core_import_sources::oura::ENDPOINTS;
use solstone_core_journal_io::{
    AtomicWriteOptions, LockError, LockOptions, atomic_replace, hold_lock,
};

use crate::body_segments::Stop;
use crate::oura_coverage::{Claim, Coverage};

/// Sent with every request: Oura refuses requests without one.
pub const USER_AGENT: &str = concat!("solstone-journal/", env!("CARGO_PKG_VERSION"));
/// The old sync's lock, held here only around a token refresh. It is a sidecar
/// beside the old sync's cursor, whose bytes this path never reads.
const OLD_SYNC_LOCK: &str = "imports/oura.json";
/// How long a refresh waits for an old sync to finish: the old lock's own default.
pub const OLD_LOCK_TIMEOUT: Duration = solstone_core_journal_io::DEFAULT_LOCK_TIMEOUT;
const HEARTRATE: &str = "heartrate";
/// Oura documents no longest range for date endpoints; a year at a time is what
/// the old sync asks.
const DATE_CHUNK_DAYS: u64 = 364;
/// Oura refuses a heartrate span whose dates are more than 30 apart.
const HEARTRATE_CHUNK_DAYS: u64 = 30;
const MOST_HEARTRATE_DATES: i64 = 30;
const MOST_PAGES_PER_CHUNK: usize = 1_000;
/// How many times one run answers a 401, adopting another path's refresh or
/// making its own, before it stops.
const MOST_RENEWALS: u32 = 4;
/// A run with no range asks again from this many days before what it last covered.
const TRAILING_DAYS: u64 = 7;
/// And this many days back when nothing is covered yet.
const FIRST_DAYS: u64 = 30;
const INDEX_FILE: &str = "index.json";
const PRIVATE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };
/// Oura's scopes, as its API names them.
const SCOPES: [&str; 9] = [
    "email",
    "personal",
    "daily",
    "heartrate",
    "workout",
    "tag",
    "session",
    "spo2",
    "heart_health",
];

/// Whose tokens a fetch sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OuraAuth {
    /// The journal's own, from its config, refreshed as Oura asks.
    Journal,
    /// None: for a transport that serves recorded responses.
    None,
}

/// Why a whole run stopped before writing anything, each with what to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OuraStop {
    /// A 401 after the one refresh a run makes, or a refresh Oura refused.
    Authorization,
    /// Oura says the account's membership isn't active.
    Membership,
    /// The old sync holds its lock past the wait.
    AnotherSync,
}

impl OuraStop {
    #[must_use]
    pub fn words(self) -> &'static str {
        match self {
            Self::Authorization => "Oura's authorization needs renewing: connect Oura again.",
            Self::Membership => {
                "Oura says this account has no active membership, so it shares no data. Run this again once the membership is active."
            }
            Self::AnotherSync => "Another Oura sync is running; run this again when it finishes.",
        }
    }
}

/// How an Oura import ended short.
#[derive(Debug, PartialEq, Eq)]
pub enum OuraImportError {
    Stop(OuraStop),
    Failed(String),
}

impl fmt::Display for OuraImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stop(stop) => formatter.write_str(stop.words()),
            Self::Failed(error) => formatter.write_str(error),
        }
    }
}

/// How one endpoint ended: exactly one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultKind {
    Read,
    Empty,
    Refused,
    RefusedPartWay,
    FailedPartWay,
    Failed,
}

impl ResultKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Empty => "empty",
            Self::Refused => "refused",
            Self::RefusedPartWay => "refused part-way",
            Self::FailedPartWay => "failed part-way",
            Self::Failed => "failed",
        }
    }
}

/// One endpoint's result, in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointReport {
    pub endpoint: String,
    pub kind: ResultKind,
    pub words: String,
}

impl EndpointReport {
    /// The line the entry prints and the summary holds.
    #[must_use]
    pub fn line(&self) -> String {
        format!(
            "- {}: {}: {}",
            self.endpoint,
            self.kind.as_str(),
            self.words
        )
    }
}

/// Why Oura refused an endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Refusal {
    /// The scopes Oura's own answer names.
    Named { scopes: Vec<String> },
    /// The scope Oura documents for this endpoint, or its description implies.
    Documented { scope: String },
    /// Oura maps this endpoint to no scope, and its answer named none.
    NotNamed,
    /// A 403 that names neither a scope nor a membership: what Oura said.
    Said { detail: String },
}

impl Refusal {
    fn words(&self) -> String {
        match self {
            Self::Named { scopes } => format!(
                "Oura says this needs the {} permission; to bring it in, connect Oura again and allow it",
                scopes
                    .iter()
                    .map(|scope| format!("\"{scope}\""))
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
            Self::Documented { scope } => format!(
                "this needs the \"{scope}\" permission; to bring it in, connect Oura again and allow it"
            ),
            Self::NotNamed => "Oura didn't say which permission this needs".to_owned(),
            Self::Said { detail } => format!("Oura said: {detail}"),
        }
    }
}

/// How an endpoint's fetch ended. `whole` is how many of its chunks arrived whole
/// before the one that didn't.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub(crate) enum Outcome {
    Read { count: u64 },
    Empty,
    Refused { whole: usize, why: Refusal },
    Failed { whole: usize, error: String },
}

/// One endpoint's part of a fetch: the owner days asked, its chunks exactly as
/// sent, and how it ended once it has.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EndpointPlan {
    pub(crate) from: String,
    pub(crate) to: String,
    /// A date endpoint's Oura days, first and last; heartrate's instants, the
    /// first and the one after the last, with the fixed zone's offset.
    pub(crate) chunks: Vec<[String; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<Outcome>,
}

impl EndpointPlan {
    /// How many of its chunks arrived whole.
    fn whole(&self) -> usize {
        match &self.result {
            Some(Outcome::Read { .. } | Outcome::Empty) => self.chunks.len(),
            Some(Outcome::Refused { whole, .. } | Outcome::Failed { whole, .. }) => *whole,
            None => 0,
        }
    }
}

/// A run's fetch, kept in its state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FetchState {
    /// The range the request named, first and last owner day. `None` for a run
    /// that worked its range out from coverage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) asked: Option<[String; 2]>,
    pub(crate) endpoints: BTreeMap<String, EndpointPlan>,
}

/// What a run's request says about its fetch, for the writer.
pub(crate) struct FetchInput<'a> {
    pub(crate) range: Option<(NaiveDate, NaiveDate)>,
    pub(crate) transport: RefCell<&'a mut dyn OuraHttp>,
    pub(crate) auth: OuraAuth,
    pub(crate) old_lock_timeout: Duration,
}

fn day_text(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

fn parse_day(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

fn parse_instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|instant| instant.to_utc())
}

/// The first instant of an owner day on the fixed zone, which a zone that skips
/// midnight starts later.
fn start_of_day(date: NaiveDate, zone: Tz) -> DateTime<Tz> {
    (0..24)
        .find_map(|hour| {
            zone.from_local_datetime(&date.and_hms_opt(hour, 0, 0)?)
                .earliest()
        })
        .unwrap_or_else(|| zone.from_utc_datetime(&date.and_time(chrono::NaiveTime::MIN)))
}

fn instant_text(instant: DateTime<Tz>) -> String {
    instant.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

/// How many dates apart two instants are, as written with their offsets and in
/// UTC, whichever is more: Oura's documentation doesn't say which it counts.
fn dates_apart(from: DateTime<Tz>, to: DateTime<Tz>) -> i64 {
    let written = (to.date_naive() - from.date_naive()).num_days();
    let utc = (to.to_utc().date_naive() - from.to_utc().date_naive()).num_days();
    written.max(utc)
}

/// Heartrate's chunks over owner days `from` to `to`: instants from `from` 00:00
/// to `to` 24:00 on the fixed zone, in half-open spans of 30 days that meet end to
/// start, each a day shorter where a change of offset would put its ends more than
/// 30 dates apart.
fn heartrate_chunks(from: NaiveDate, to: NaiveDate, zone: Tz) -> Vec<[String; 2]> {
    let mut chunks = Vec::new();
    let end = to + Days::new(1);
    let mut day = from;
    while day < end {
        let mut next = (day + Days::new(HEARTRATE_CHUNK_DAYS)).min(end);
        while next > day + Days::new(1)
            && dates_apart(start_of_day(day, zone), start_of_day(next, zone)) > MOST_HEARTRATE_DATES
        {
            next = next - Days::new(1);
        }
        chunks.push([
            instant_text(start_of_day(day, zone)),
            instant_text(start_of_day(next, zone)),
        ]);
        day = next;
    }
    chunks
}

/// A date endpoint's chunks over Oura days `from` to `to`, a year at a time.
fn date_chunks(from: NaiveDate, to: NaiveDate) -> Vec<[String; 2]> {
    let mut chunks = Vec::new();
    let mut first = from;
    while first <= to {
        let last = (first + Days::new(DATE_CHUNK_DAYS - 1)).min(to);
        chunks.push([day_text(first), day_text(last)]);
        first = last + Days::new(1);
    }
    chunks
}

/// What a new run asks for: the range named, or each endpoint from seven days
/// before what it last covered through today, or the last 30 days when it has
/// covered nothing.
pub(crate) fn plan(
    range: Option<(NaiveDate, NaiveDate)>,
    coverage: &Coverage,
    today: NaiveDate,
    zone: Tz,
) -> Result<FetchState, String> {
    if let Some((from, to)) = range
        && from > to
    {
        return Err(format!(
            "the range ends on {}, before it starts on {}: put the earlier date first",
            day_text(to),
            day_text(from)
        ));
    }
    let mut endpoints = BTreeMap::new();
    for endpoint in ENDPOINTS {
        let (from, to) = range.unwrap_or_else(|| {
            let from = coverage
                .last_covered_day(endpoint, zone)
                .map_or(today - Days::new(FIRST_DAYS), |day| {
                    day - Days::new(TRAILING_DAYS)
                });
            (from.min(today), today)
        });
        let chunks = if endpoint == HEARTRATE {
            heartrate_chunks(from, to, zone)
        } else {
            date_chunks(from, to)
        };
        endpoints.insert(
            endpoint.to_owned(),
            EndpointPlan {
                from: day_text(from),
                to: day_text(to),
                chunks,
                result: None,
            },
        );
    }
    Ok(FetchState {
        asked: range.map(|(from, to)| [day_text(from), day_text(to)]),
        endpoints,
    })
}

impl FetchState {
    /// Whether a request carries this fetch on: one naming no range carries on
    /// whatever the stopped run asked; one naming a range, only the same range.
    pub(crate) fn resumed_by(&self, range: Option<(NaiveDate, NaiveDate)>) -> bool {
        range.is_none_or(|(from, to)| self.asked == Some([day_text(from), day_text(to)]))
    }

    /// What each endpoint fetched whole.
    pub(crate) fn claims(&self) -> BTreeMap<String, Vec<Claim>> {
        let mut claims = BTreeMap::new();
        for (endpoint, plan) in &self.endpoints {
            let chunks: Vec<Claim> = plan.chunks[..plan.whole()]
                .iter()
                .filter_map(|[from, to]| {
                    if endpoint == HEARTRATE {
                        Some(Claim::Instants(parse_instant(from)?, parse_instant(to)?))
                    } else {
                        Some(Claim::Days(parse_day(from)?, parse_day(to)?))
                    }
                })
                .collect();
            claims.insert(endpoint.clone(), chunks);
        }
        claims
    }

    /// Whether `pages` still holds every page of each chunk an endpoint finished,
    /// as the index recorded it. A restore carries the state and leaves the pages
    /// out, and a fetch skips an endpoint it has finished, so without them its
    /// documents would never be written and its days would still be claimed.
    pub(crate) fn staged_whole(&self, pages: &Path) -> bool {
        let Ok(index) = read_index(pages) else {
            return false;
        };
        self.endpoints.iter().all(|(endpoint, plan)| {
            (0..plan.whole()).all(|chunk| {
                let mut staged = index
                    .pages
                    .iter()
                    .filter(|entry| entry.endpoint == *endpoint && entry.chunk == chunk)
                    .peekable();
                staged.peek().is_some()
                    && staged.all(|entry| {
                        fs::read(pages.join(&entry.file))
                            .is_ok_and(|bytes| sha256_hex(&bytes) == entry.sha256)
                    })
            })
        })
    }

    /// Endpoints Oura refused or that failed, which make the import partly failed.
    pub(crate) fn not_read(&self) -> Vec<String> {
        self.endpoints
            .iter()
            .filter(|(_, plan)| {
                matches!(
                    plan.result,
                    Some(Outcome::Refused { .. } | Outcome::Failed { .. })
                )
            })
            .map(|(endpoint, _)| endpoint.clone())
            .collect()
    }

    /// Each endpoint's result, in the order Oura is asked.
    pub(crate) fn reports(&self, zone: Tz) -> Vec<EndpointReport> {
        ENDPOINTS
            .iter()
            .filter_map(|endpoint| {
                let plan = self.endpoints.get(*endpoint)?;
                Some(report(endpoint, plan, plan.result.as_ref()?, zone))
            })
            .collect()
    }

    /// The summary's section of results.
    pub(crate) fn summary_section(&self, zone: Tz) -> String {
        let mut text = String::from("## From Oura\n\n");
        for report in self.reports(zone) {
            let _ = writeln!(text, "{}", report.line());
        }
        text.push('\n');
        text
    }
}

/// The last day a chunk asked for, as its result names it.
fn chunk_last_day(endpoint: &str, chunk: &[String; 2], zone: Tz) -> String {
    if endpoint == HEARTRATE {
        parse_instant(&chunk[1])
            .map(|end| {
                day_text((end.with_timezone(&zone) - chrono::Duration::seconds(1)).date_naive())
            })
            .unwrap_or_else(|| chunk[1].clone())
    } else {
        format!("Oura's day {}", chunk[1])
    }
}

fn report(endpoint: &str, plan: &EndpointPlan, outcome: &Outcome, zone: Tz) -> EndpointReport {
    let days = if endpoint == HEARTRATE {
        format!("{} to {}", plan.from, plan.to)
    } else {
        format!("Oura's days {} to {}", plan.from, plan.to)
    };
    let through = |whole: usize| chunk_last_day(endpoint, &plan.chunks[whole - 1], zone);
    let (kind, words) = match outcome {
        Outcome::Read { count } => {
            let noun = match (endpoint == HEARTRATE, *count == 1) {
                (true, true) => "reading",
                (true, false) => "readings",
                (false, true) => "entry",
                (false, false) => "entries",
            };
            (ResultKind::Read, format!("{count} {noun} for {days}"))
        }
        Outcome::Empty => (ResultKind::Empty, format!("nothing for {days}")),
        Outcome::Refused { whole: 0, why } => (ResultKind::Refused, why.words()),
        Outcome::Refused { whole, why } => (
            ResultKind::RefusedPartWay,
            format!(
                "read in full through {}, then {}",
                through(*whole),
                why.words()
            ),
        ),
        Outcome::Failed { whole: 0, error } => (ResultKind::Failed, error.clone()),
        Outcome::Failed { whole, error } => (
            ResultKind::FailedPartWay,
            format!("read in full through {}, then {error}", through(*whole)),
        ),
    };
    EndpointReport {
        endpoint: endpoint.to_owned(),
        kind,
        words,
    }
}

/// The scope an endpoint needs, where Oura documents it or its scope's
/// description says so. Oura maps the rest to none.
fn documented_scope(endpoint: &str) -> Option<&'static str> {
    Some(match endpoint {
        "daily_readiness" | "daily_sleep" | "daily_activity" | "sleep" => "daily",
        "daily_spo2" => "spo2",
        "heartrate" => "heartrate",
        "workout" => "workout",
        "session" => "session",
        "enhanced_tag" => "tag",
        _ => return None,
    })
}

/// What a 403's body says, as text.
fn detail_of(body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("detail")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| body.trim().to_owned());
    detail.chars().take(300).collect()
}

/// The scopes a 403's detail names, as whole words.
fn named_scopes(detail: &str) -> Vec<String> {
    let lower = detail.to_ascii_lowercase();
    let words: BTreeSet<&str> = lower
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .collect();
    SCOPES
        .iter()
        .filter(|scope| words.contains(**scope))
        .map(|scope| (*scope).to_owned())
        .collect()
}

fn refusal(endpoint: &str, permission: bool, body: &str) -> Refusal {
    let detail = detail_of(body);
    if !permission {
        return Refusal::Said { detail };
    }
    let scopes = named_scopes(&detail);
    if !scopes.is_empty() {
        return Refusal::Named { scopes };
    }
    match documented_scope(endpoint) {
        Some(scope) => Refusal::Documented {
            scope: scope.to_owned(),
        },
        None => Refusal::NotNamed,
    }
}

/// An error the transport, its retries or Oura's answer gave, in words.
fn failure_words(stage: &str) -> String {
    match stage {
        "api_retry_exhausted" => "Oura was busy or unavailable each time it was asked".to_owned(),
        "http_get" | "http_post" => "Oura couldn't be reached".to_owned(),
        "http_read" => "Oura's answer was cut off".to_owned(),
        "http_response_too_large" => {
            "Oura's answer was larger than the journal reads at once".to_owned()
        }
        "http_utf8" | "api_json" | "api_object" | "api_data" => {
            "Oura's answer couldn't be read".to_owned()
        }
        other => format!("Oura's answer couldn't be used ({other})"),
    }
}

/// One staged page, as the index names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct IndexEntry {
    endpoint: String,
    chunk: usize,
    n: usize,
    file: String,
    sha256: String,
    /// A heartrate chunk's instants, so the scan reads only its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    to: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    pages: Vec<IndexEntry>,
}

fn read_index(pages: &Path) -> Result<Index, String> {
    let path = pages.join(INDEX_FILE);
    match fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Index::default()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn save_index(pages: &Path, index: &Index) -> Result<(), String> {
    let bytes = serde_json::to_vec(index).map_err(|error| error.to_string())?;
    atomic_replace(pages.join(INDEX_FILE), &bytes, PRIVATE).map_err(|error| error.to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A page's `data`, or `None` when the answer isn't a page.
fn page_data(body: &str) -> Option<(Vec<Value>, Option<String>)> {
    let Value::Object(mut page) = serde_json::from_str::<Value>(body).ok()? else {
        return None;
    };
    let Value::Array(data) = page.remove("data")? else {
        return None;
    };
    let next = page
        .get("next_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_owned);
    Some((data, next))
}

/// How a request ended short.
enum Fault {
    /// The run stops.
    Stop(OuraStop),
    /// The run can't go on, for every endpoint alike.
    Run(String),
    /// This endpoint can't go on.
    Endpoint(String),
}

impl From<Fault> for Stop {
    fn from(fault: Fault) -> Self {
        match fault {
            Fault::Stop(stop) => Stop::Oura(stop),
            Fault::Run(words) | Fault::Endpoint(words) => Stop::Failed(words),
        }
    }
}

const TOKEN_STORE_WORDS: &str =
    "Oura renewed its authorization and the journal couldn't save it: connect Oura again.";

/// The journal's tokens, as config holds them.
fn load_settings(journal: &Path) -> Result<OuraSettings, Fault> {
    settings(journal).map_err(|error| match error.stage() {
        "oura_config_missing" | "authorization_needed" | "journal_config_missing" => {
            Fault::Run("Oura isn't connected to this journal: connect Oura first.".to_owned())
        }
        "journal_config" => Fault::Run("the journal's settings couldn't be read".to_owned()),
        _ => Fault::Stop(OuraStop::Authorization),
    })
}

/// The tokens a fetch sends, and how it answers a 401.
struct Tokens<'j> {
    journal: &'j Path,
    settings: OuraSettings,
    /// This run made its one refresh.
    refreshed: bool,
    renewals: u32,
    lock_timeout: Duration,
}

impl<'j> Tokens<'j> {
    fn load(journal: &'j Path, input: &FetchInput<'_>, zone: Tz) -> Result<Self, Fault> {
        let (settings, refreshed) = match input.auth {
            OuraAuth::Journal => (load_settings(journal)?, false),
            // Nothing to renew: a 401 from a recorded answer ends the run.
            OuraAuth::None => (
                OuraSettings {
                    client_id: String::new(),
                    client_secret: None,
                    access_token: "none".to_owned(),
                    refresh_token: String::new(),
                    token_type: "Bearer".to_owned(),
                    timezone: zone.name().to_owned(),
                },
                true,
            ),
        };
        Ok(Self {
            journal,
            settings,
            refreshed,
            renewals: 0,
            lock_timeout: input.old_lock_timeout,
        })
    }

    /// One request, through the old sync's retries. That function refreshes on a
    /// 401 by itself, without the lock and without reading config again, so it is
    /// never let to: asked not to persist, it says a refresh is needed instead, and
    /// this answers that under the lock. After this run's refresh, it hands back a
    /// 401 as it is, which ends the run.
    fn get(
        &mut self,
        http: &mut dyn OuraHttp,
        endpoint: &str,
        query: &BTreeMap<String, String>,
    ) -> Result<HttpResponse, Fault> {
        loop {
            match request_with_retry(
                http,
                endpoint,
                query,
                &mut self.settings,
                &mut self.refreshed,
                self.journal,
                false,
            ) {
                Err(error) if error.stage() == "authorization_refresh_requires_save" => {
                    self.renew(http)?;
                }
                Err(error) => return Err(Fault::Endpoint(failure_words(error.stage()))),
                // Not a permission: the authorization itself.
                Ok(response) if response.status == 401 => {
                    return Err(Fault::Stop(OuraStop::Authorization));
                }
                Ok(response) => return Ok(response),
            }
        }
    }

    /// Answer a 401: under the old sync's lock, read the tokens again, and use
    /// them if another path changed them; otherwise make this run's one refresh,
    /// which writes the rotated tokens to config.
    fn renew(&mut self, http: &mut dyn OuraHttp) -> Result<(), Fault> {
        self.renewals += 1;
        if self.renewals > MOST_RENEWALS {
            return Err(Fault::Stop(OuraStop::Authorization));
        }
        let _old_sync = hold_lock(
            self.journal.join(OLD_SYNC_LOCK),
            LockOptions {
                timeout: self.lock_timeout,
                mode: Some(0o600),
                ..LockOptions::default()
            },
        )
        .map_err(|error| match error {
            LockError::Timeout(_) => Fault::Stop(OuraStop::AnotherSync),
            other => Fault::Run(format!("the Oura sync lock couldn't be taken: {other}")),
        })?;
        let mut current = load_settings(self.journal)?;
        if current.access_token != self.settings.access_token
            || current.refresh_token != self.settings.refresh_token
        {
            self.settings = current;
            return Ok(());
        }
        refresh(http, &mut current, self.journal, true).map_err(|error| match error.stage() {
            "authorization_refresh" | "authorization_response" => {
                Fault::Stop(OuraStop::Authorization)
            }
            "token_store" => Fault::Run(TOKEN_STORE_WORDS.to_owned()),
            _ => Fault::Run(
                "Oura couldn't be reached to renew its authorization; run this again.".to_owned(),
            ),
        })?;
        self.refreshed = true;
        self.settings = current;
        Ok(())
    }
}

fn chunk_query(endpoint: &str, chunk: &[String; 2]) -> BTreeMap<String, String> {
    if endpoint == HEARTRATE {
        BTreeMap::from([
            ("start_datetime".to_owned(), chunk[0].clone()),
            ("end_datetime".to_owned(), chunk[1].clone()),
        ])
    } else {
        // One day past the chunk's last: right whether Oura reads the end as
        // inclusive or not, since a document seen twice is read once.
        let end = parse_day(&chunk[1])
            .map_or_else(|| chunk[1].clone(), |last| day_text(last + Days::new(1)));
        BTreeMap::from([
            ("start_date".to_owned(), chunk[0].clone()),
            ("end_date".to_owned(), end),
        ])
    }
}

/// How many documents an endpoint's staged pages hold, each id once, or how many
/// readings fall in their chunks.
fn staged_count(endpoint: &str, index: &Index, pages: &Path) -> Result<u64, String> {
    let mut ids = BTreeSet::new();
    let mut readings = 0_u64;
    for entry in index
        .pages
        .iter()
        .filter(|entry| entry.endpoint == endpoint)
    {
        let path = pages.join(&entry.file);
        let body =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let Some((data, _)) = page_data(&body) else {
            continue;
        };
        for item in data {
            if endpoint == HEARTRATE {
                let at = item
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(parse_instant);
                let from = entry.from.as_deref().and_then(parse_instant);
                let to = entry.to.as_deref().and_then(parse_instant);
                if let (Some(at), Some(from), Some(to)) = (at, from, to)
                    && (at < from || at >= to)
                {
                    continue;
                }
                readings += 1;
            } else if let Some(id) = item.get("id").and_then(Value::as_str) {
                ids.insert(id.to_owned());
            }
        }
    }
    Ok(if endpoint == HEARTRATE {
        readings
    } else {
        ids.len() as u64
    })
}

/// What one fetch needs from the run it is part of.
pub(crate) struct FetchRun<'r> {
    pub(crate) journal: &'r Path,
    pub(crate) pages: PathBuf,
    pub(crate) zone: Tz,
    /// Called at each named point; the run stops there when it says so.
    pub(crate) point: &'r dyn Fn(&str) -> Result<(), Stop>,
}

/// Fetch every endpoint the state hasn't finished, into the run's staging. `save`
/// keeps the state each time an endpoint ends.
pub(crate) fn fetch(
    run: &FetchRun<'_>,
    state: &mut FetchState,
    input: &FetchInput<'_>,
    save: &mut dyn FnMut(&FetchState) -> Result<(), String>,
) -> Result<(), Stop> {
    let pending: Vec<&str> = ENDPOINTS
        .iter()
        .copied()
        .filter(|endpoint| {
            state
                .endpoints
                .get(*endpoint)
                .is_some_and(|plan| plan.result.is_none())
        })
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(&run.pages).map_err(|error| error.to_string())?;
    let mut index = read_index(&run.pages)?;
    let mut http = input.transport.borrow_mut();
    let mut tokens = Tokens::load(run.journal, input, run.zone)?;
    for endpoint in pending {
        let plan = state.endpoints[endpoint].clone();
        let outcome = fetch_endpoint(run, endpoint, &plan, &mut index, &mut tokens, &mut **http)?;
        if let Some(plan) = state.endpoints.get_mut(endpoint) {
            plan.result = Some(outcome);
        }
        save(state)?;
    }
    Ok(())
}

fn fetch_endpoint(
    run: &FetchRun<'_>,
    endpoint: &str,
    plan: &EndpointPlan,
    index: &mut Index,
    tokens: &mut Tokens<'_>,
    http: &mut dyn OuraHttp,
) -> Result<Outcome, Stop> {
    let pages = run.pages.as_path();
    for (chunk, span) in plan.chunks.iter().enumerate() {
        // Where a stopped run left this chunk: whole, or a page to go on from.
        let staged: Vec<IndexEntry> = index
            .pages
            .iter()
            .filter(|entry| entry.endpoint == endpoint && entry.chunk == chunk)
            .cloned()
            .collect();
        let mut next = match staged.last() {
            Some(last) => {
                let path = pages.join(&last.file);
                let body = fs::read_to_string(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                match page_data(&body) {
                    Some((_, None)) => continue,
                    Some((_, token)) => token,
                    None => return Err(Stop::Failed(format!("{}: not a page", path.display()))),
                }
            }
            None => None,
        };
        let mut n = staged.len();
        let mut restarted = false;
        loop {
            if n >= MOST_PAGES_PER_CHUNK {
                return Ok(Outcome::Failed {
                    whole: chunk,
                    error: "Oura sent more pages than the journal reads for one request".to_owned(),
                });
            }
            let mut query = chunk_query(endpoint, span);
            if let Some(token) = &next {
                query.insert("next_token".to_owned(), token.clone());
            }
            let response = match tokens.get(http, endpoint, &query) {
                Ok(response) => response,
                Err(Fault::Endpoint(error)) => {
                    return Ok(Outcome::Failed {
                        whole: chunk,
                        error,
                    });
                }
                Err(fault) => return Err(fault.into()),
            };
            // A page token Oura no longer takes: this chunk starts again from its
            // first page, so every document and reading in it comes from one
            // fetch of it.
            if next.is_some() && matches!(response.status, 400 | 422) && !restarted {
                restarted = true;
                let (dropped, kept): (Vec<IndexEntry>, Vec<IndexEntry>) =
                    std::mem::take(&mut index.pages)
                        .into_iter()
                        .partition(|entry| entry.endpoint == endpoint && entry.chunk == chunk);
                index.pages = kept;
                save_index(pages, index)?;
                for entry in dropped {
                    let _ = fs::remove_file(pages.join(&entry.file));
                }
                (run.point)("token-discarded")?;
                next = None;
                n = 0;
                continue;
            }
            match response.status {
                200 => {
                    let Some((_, token)) = page_data(&response.body) else {
                        return Ok(Outcome::Failed {
                            whole: chunk,
                            error: failure_words("api_data"),
                        });
                    };
                    let file = format!("{endpoint}/{chunk}-{n}.json");
                    let path = pages.join(&file);
                    fs::create_dir_all(path.parent().expect("a page has a folder"))
                        .map_err(|error| error.to_string())?;
                    atomic_replace(&path, response.body.as_bytes(), PRIVATE)
                        .map_err(|error| error.to_string())?;
                    (run.point)("page-landed")?;
                    let heartrate = endpoint == HEARTRATE;
                    index.pages.push(IndexEntry {
                        endpoint: endpoint.to_owned(),
                        chunk,
                        n,
                        file,
                        sha256: sha256_hex(response.body.as_bytes()),
                        from: heartrate.then(|| span[0].clone()),
                        to: heartrate.then(|| span[1].clone()),
                    });
                    save_index(pages, index)?;
                    n += 1;
                    next = token;
                    if next.is_none() {
                        break;
                    }
                }
                403 => {
                    let permission = match classify_response(endpoint, &response) {
                        Ok(ResponseClass::Permission) => true,
                        Err(error) if error.stage() == "membership_lost" => {
                            return Err(Stop::Oura(OuraStop::Membership));
                        }
                        Ok(ResponseClass::Page(_)) | Err(_) => false,
                    };
                    return Ok(Outcome::Refused {
                        whole: chunk,
                        why: refusal(endpoint, permission, &response.body),
                    });
                }
                status => {
                    return Ok(Outcome::Failed {
                        whole: chunk,
                        error: format!("Oura answered with error {status}"),
                    });
                }
            }
        }
    }
    let count = staged_count(endpoint, index, pages)?;
    Ok(if count == 0 {
        Outcome::Empty
    } else {
        Outcome::Read { count }
    })
}

/// Oura over the network, as the old sync reaches it, sending a User-Agent.
pub struct LiveOuraTransport {
    agent: ureq::Agent,
}

impl Default for LiveOuraTransport {
    fn default() -> Self {
        let timeout = Duration::from_secs(30);
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(timeout))
            .timeout_recv_response(Some(timeout))
            .timeout_recv_body(Some(timeout))
            .timeout_global(Some(Duration::from_secs(90)))
            .user_agent(USER_AGENT)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

fn transport_failure(stage: &'static str) -> BodyIngestError {
    BodyIngestError::new(BodyIngestErrorKind::Source, stage)
}

impl OuraHttp for LiveOuraTransport {
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
            .map_err(|_| transport_failure("http_get"))?;
        LiveOuraHttp::response(response)
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
            .map_err(|_| transport_failure("http_post"))?;
        LiveOuraHttp::response(response)
    }
}

/// The documents of `endpoint` a query asks for, as Oura's sandbox answers: date
/// endpoints by Oura's `day` (a tag's `start_day`) from `start_date` up to, not
/// including, `end_date`; heartrate by instant from `start_datetime` up to, not
/// including, `end_datetime`.
pub(crate) fn asked_documents<'d>(
    endpoint: &str,
    query: &BTreeMap<String, String>,
    documents: &'d [Value],
) -> Vec<&'d Value> {
    if endpoint == HEARTRATE {
        let bound = |key: &str| query.get(key).and_then(|text| parse_instant(text));
        let (Some(from), Some(to)) = (bound("start_datetime"), bound("end_datetime")) else {
            return Vec::new();
        };
        documents
            .iter()
            .filter(|document| {
                document
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(parse_instant)
                    .is_some_and(|at| from <= at && at < to)
            })
            .collect()
    } else {
        let bound = |key: &str| query.get(key).and_then(|text| parse_day(text));
        let (Some(from), Some(to)) = (bound("start_date"), bound("end_date")) else {
            return Vec::new();
        };
        let key = if endpoint == "enhanced_tag" {
            "start_day"
        } else {
            "day"
        };
        documents
            .iter()
            .filter(|document| {
                document
                    .get(key)
                    .and_then(Value::as_str)
                    .and_then(parse_day)
                    .is_some_and(|day| from <= day && day < to)
            })
            .collect()
    }
}

/// A folder of recorded responses, in the staged format, served as Oura would
/// answer each request: every recorded document the query asks for, on one page.
pub struct RecordedOura {
    documents: BTreeMap<String, Vec<Value>>,
}

impl RecordedOura {
    /// Read every page the folder's index names.
    pub fn open(folder: &Path) -> Result<Self, String> {
        let index = read_index(folder)?;
        if index.pages.is_empty() && !folder.join(INDEX_FILE).is_file() {
            return Err(format!("{}: no {INDEX_FILE}", folder.display()));
        }
        let mut documents: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for entry in &index.pages {
            let path = folder.join(&entry.file);
            let body = fs::read_to_string(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let (data, _) =
                page_data(&body).ok_or_else(|| format!("{}: not a page", path.display()))?;
            documents
                .entry(entry.endpoint.clone())
                .or_default()
                .extend(data);
        }
        Ok(Self { documents })
    }
}

impl OuraHttp for RecordedOura {
    fn get(
        &mut self,
        endpoint: &str,
        query: &BTreeMap<String, String>,
        _authorization: &str,
    ) -> Result<HttpResponse, BodyIngestError> {
        let documents = self.documents.get(endpoint).map_or(&[][..], Vec::as_slice);
        let data: Vec<&Value> = asked_documents(endpoint, query, documents);
        Ok(HttpResponse {
            status: 200,
            body: serde_json::json!({"data": data, "next_token": null}).to_string(),
        })
    }

    fn post_form(
        &mut self,
        _url: &str,
        _form: &BTreeMap<String, String>,
    ) -> Result<HttpResponse, BodyIngestError> {
        Err(transport_failure("http_post"))
    }

    fn backoff(&mut self, _attempt: u32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_request_sends_the_journals_user_agent() {
        let transport = LiveOuraTransport::default();
        match transport.agent.config().user_agent() {
            ureq::config::AutoHeaderValue::Provided(agent) => {
                assert!(agent.starts_with("solstone-journal/"), "{agent}");
            }
            other => panic!("no user agent of its own: {other:?}"),
        }
    }

    #[test]
    fn a_scope_is_read_from_oura_s_words_only_as_a_whole_word() {
        assert_eq!(
            named_scopes("Missing scope: heartrate. Your token needs heartrate."),
            ["heartrate"]
        );
        assert!(named_scopes("daily_sleep is not allowed").is_empty());
        assert_eq!(
            refusal(
                "daily_stress",
                true,
                r#"{"detail": "Missing Scope Permission"}"#
            ),
            Refusal::NotNamed
        );
        assert_eq!(
            refusal("sleep", true, r#"{"detail": "Missing Scope Permission"}"#),
            Refusal::Documented {
                scope: "daily".to_owned()
            }
        );
    }
}
