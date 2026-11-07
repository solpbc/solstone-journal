// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An Oura run that asks Oura itself, against a stand-in for Oura's API held in
//! memory: its range, its coverage, each endpoint's result, the run's stops, kill
//! and resume, and the old sync beside it.

#![allow(clippy::disallowed_methods)]

use std::cell::RefCell;
use std::rc::Rc;

use chrono::TimeZone as _;
use solstone_core_body_ingest::oura_sync::{
    HttpResponse, OuraHttp, OuraSyncOptions, hold_oura_lock, sync_with_http,
};
use solstone_core_body_ingest::{BodyIngestError, BodyIngestErrorKind};

use super::*;
use crate::oura_fetch::asked_documents;

/// Which requests a rule answers, by their query.
type Matches = Box<dyn Fn(&BTreeMap<String, String>) -> bool>;
/// Run once before a numbered request is answered.
type Hook = Box<dyn FnOnce()>;

/// Oura's answer to one kind of request, in place of what its documents say.
struct Rule {
    endpoint: &'static str,
    matches: Matches,
    status: u16,
    body: String,
    /// How many more times it answers; `None` for every time.
    times: Option<usize>,
}

/// A stand-in for Oura's API: documents by endpoint, answered by the query as the
/// sandbox answers, in pages; access tokens it takes; one live refresh token it
/// rotates once and refuses after; and a log of every request.
#[derive(Default)]
struct Server {
    documents: BTreeMap<String, Vec<Value>>,
    page_size: BTreeMap<String, usize>,
    rules: Vec<Rule>,
    access: BTreeSet<String>,
    refresh: String,
    issued: u32,
    /// Tokens it issues are refused by its API all the same.
    refuse_issued: bool,
    /// Heartrate answers a reading at `end_datetime` too, as Oura may.
    heartrate_inclusive: bool,
    /// Page tokens carry this; one from another generation is refused.
    generation: u32,
    /// Run once, before the request with this number is answered.
    hooks: Vec<(usize, Hook)>,
    gets: Vec<(String, BTreeMap<String, String>, String)>,
    posts: Vec<BTreeMap<String, String>>,
}

#[derive(Clone)]
struct Oura(Rc<RefCell<Server>>);

fn answer(status: u16, body: Value) -> HttpResponse {
    HttpResponse {
        status,
        body: body.to_string(),
    }
}

impl Oura {
    fn new() -> Self {
        let server = Server {
            access: BTreeSet::from(["access-0".to_owned()]),
            refresh: "refresh-0".to_owned(),
            ..Server::default()
        };
        Self(Rc::new(RefCell::new(server)))
    }

    fn serve(&self, endpoint: &str, documents: Vec<Value>) -> &Self {
        self.0
            .borrow_mut()
            .documents
            .entry(endpoint.to_owned())
            .or_default()
            .extend(documents);
        self
    }

    fn rule(
        &self,
        endpoint: &'static str,
        matches: impl Fn(&BTreeMap<String, String>) -> bool + 'static,
        status: u16,
        body: Value,
        times: Option<usize>,
    ) -> &Self {
        self.0.borrow_mut().rules.push(Rule {
            endpoint,
            matches: Box::new(matches),
            status,
            body: body.to_string(),
            times,
        });
        self
    }

    fn expire(&self, token: &str) {
        self.0.borrow_mut().access.remove(token);
    }

    fn gets(&self) -> Vec<(String, BTreeMap<String, String>, String)> {
        self.0.borrow().gets.clone()
    }

    fn posts(&self) -> usize {
        self.0.borrow().posts.len()
    }

    /// The first request of each endpoint, with its query.
    fn first_asked(&self, endpoint: &str) -> BTreeMap<String, String> {
        self.gets()
            .into_iter()
            .find(|(name, ..)| name == endpoint)
            .map(|(_, query, _)| query)
            .unwrap_or_default()
    }
}

impl OuraHttp for Oura {
    fn get(
        &mut self,
        endpoint: &str,
        query: &BTreeMap<String, String>,
        authorization: &str,
    ) -> Result<HttpResponse, BodyIngestError> {
        let hook = {
            let mut server = self.0.borrow_mut();
            let number = server.gets.len();
            server
                .gets
                .push((endpoint.to_owned(), query.clone(), authorization.to_owned()));
            server
                .hooks
                .iter()
                .position(|(at, _)| *at == number)
                .map(|found| server.hooks.remove(found).1)
        };
        if let Some(hook) = hook {
            hook();
        }
        let mut server = self.0.borrow_mut();
        let token = authorization
            .split_once(' ')
            .map_or(authorization, |(_, token)| token);
        if !server.access.contains(token) || (server.refuse_issued && token != "access-0") {
            return Ok(answer(401, json!({"detail": "expired"})));
        }
        for rule in &mut server.rules {
            if rule.endpoint == endpoint && rule.times != Some(0) && (rule.matches)(query) {
                if let Some(times) = &mut rule.times {
                    *times -= 1;
                }
                return Ok(HttpResponse {
                    status: rule.status,
                    body: rule.body.clone(),
                });
            }
        }
        let offset = match query.get("next_token") {
            None => 0,
            Some(token) => {
                let parsed = token
                    .strip_prefix('g')
                    .and_then(|rest| rest.split_once(':'))
                    .and_then(|(generation, offset)| {
                        Some((
                            generation.parse::<u32>().ok()?,
                            offset.parse::<usize>().ok()?,
                        ))
                    });
                match parsed {
                    Some((generation, offset)) if generation == server.generation => offset,
                    _ => return Ok(answer(400, json!({"detail": "invalid next_token"}))),
                }
            }
        };
        let documents = server.documents.get(endpoint).cloned().unwrap_or_default();
        let mut asked: Vec<Value> = asked_documents(endpoint, query, &documents)
            .into_iter()
            .cloned()
            .collect();
        if server.heartrate_inclusive && endpoint == "heartrate" {
            let end = query
                .get("end_datetime")
                .and_then(|end| DateTime::parse_from_rfc3339(end).ok());
            asked.extend(
                documents
                    .iter()
                    .filter(|document| {
                        document
                            .get("timestamp")
                            .and_then(Value::as_str)
                            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                            == end
                    })
                    .cloned(),
            );
        }
        let size = server
            .page_size
            .get(endpoint)
            .copied()
            .unwrap_or(usize::MAX);
        let end = offset.saturating_add(size).min(asked.len());
        let next = (end < asked.len()).then(|| format!("g{}:{end}", server.generation));
        Ok(answer(
            200,
            json!({"data": asked[offset.min(end)..end], "next_token": next}),
        ))
    }

    fn post_form(
        &mut self,
        _url: &str,
        form: &BTreeMap<String, String>,
    ) -> Result<HttpResponse, BodyIngestError> {
        let mut server = self.0.borrow_mut();
        server.posts.push(form.clone());
        if form.get("refresh_token") != Some(&server.refresh) {
            // Refresh tokens are single-use.
            return Ok(answer(400, json!({"error": "invalid_grant"})));
        }
        server.issued += 1;
        let issued = server.issued;
        server.refresh = format!("refresh-{issued}");
        server.access.insert(format!("access-{issued}"));
        Ok(answer(
            200,
            json!({
                "access_token": format!("access-{issued}"),
                "refresh_token": format!("refresh-{issued}"),
                "token_type": "Bearer",
                "expires_in": 3600,
            }),
        ))
    }

    fn backoff(&mut self, _attempt: u32) {}
}

/// A journal connected to Oura, on the zone a test names.
fn connected(zone: &str) -> tempfile::TempDir {
    let journal = journal();
    fs::write(
        journal.path().join("config/journal.json"),
        json!({
            "identity": {"timezone": zone},
            "oura": {
                "client_id": "synthetic-client",
                "tokens": {
                    "access_token": "access-0",
                    "refresh_token": "refresh-0",
                    "token_type": "Bearer",
                    "expires_at": 4_102_444_800.0_f64,
                },
            },
        })
        .to_string(),
    )
    .unwrap();
    journal
}

/// Noon in Denver, 2026-02-01: every run here reads this clock.
fn frozen() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 2, 1, 19, 0, 0).unwrap()
}

fn date(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
}

struct Ask<'a> {
    timestamp: &'a str,
    facet: Option<&'a str>,
    range: Option<(&'a str, &'a str)>,
    stop_at: Option<&'a dyn Fn(&str) -> bool>,
    /// The frozen clock unless a test names another.
    clock: Option<&'a dyn Fn() -> DateTime<Utc>>,
}

fn ask<'a>(timestamp: &'a str, range: Option<(&'a str, &'a str)>) -> Ask<'a> {
    Ask {
        timestamp,
        facet: None,
        range,
        stop_at: None,
        clock: None,
    }
}

fn import(root: &Path, oura: &Oura, ask: &Ask<'_>) -> Result<OuraImportOutcome, OuraImportError> {
    let mut transport = oura.clone();
    let mut request = OuraImportRequest::new(ask.timestamp, ask.facet, &mut transport);
    request.range = ask.range.map(|(from, to)| (date(from), date(to)));
    request.spill_bytes = 4_096;
    request.stop_at = ask.stop_at;
    request.clock = Some(ask.clock.unwrap_or(&frozen));
    // A lock someone else holds is refused at once, not waited for.
    request.old_lock_timeout = Duration::ZERO;
    import_oura(root, request)
}

fn finished(root: &Path, oura: &Oura, ask: &Ask<'_>) -> OuraImportOutcome {
    let outcome = import(root, oura, ask).unwrap();
    assert!(outcome.finished, "{}", outcome.summary);
    outcome
}

fn coverage_file(root: &Path) -> Value {
    fs::read(root.join(crate::oura_coverage::COVERAGE_REL))
        .map(|bytes| serde_json::from_slice(&bytes).unwrap())
        .unwrap_or(Value::Null)
}

fn covered(root: &Path, endpoint: &str) -> Value {
    coverage_file(root)["endpoints"][endpoint]["covered"].clone()
}

fn holes_of_file(root: &Path, endpoint: &str) -> Value {
    coverage_file(root)["endpoints"][endpoint]["holes"].clone()
}

fn result_of(outcome: &OuraImportOutcome, endpoint: &str) -> EndpointReport {
    outcome
        .results
        .iter()
        .find(|report| report.endpoint == endpoint)
        .cloned()
        .unwrap()
}

#[test]
fn an_uninterrupted_run_covers_exactly_the_oura_days_asked() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let oura = Oura::new();
    oura.serve(
        "daily_readiness",
        vec![
            readiness("r1", "2026-01-05", 80),
            readiness("r2", "2026-01-21", 70),
        ],
    )
    .serve(
        "heartrate",
        vec![heart("2026-01-05T17:00:00+00:00", 61, "awake")],
    );
    let outcome = finished(
        root,
        &oura,
        &ask("20261104_120000", Some(("2026-01-01", "2026-01-20"))),
    );
    for endpoint in solstone_core_import_sources::oura::ENDPOINTS {
        if endpoint == "heartrate" {
            assert_eq!(
                covered(root, endpoint),
                json!([["2026-01-01T07:00:00Z", "2026-01-21T07:00:00Z"]])
            );
        } else {
            assert_eq!(
                covered(root, endpoint),
                json!([["2026-01-01", "2026-01-20"]]),
                "{endpoint}"
            );
            // Asked one day past the last, which the sandbox reads as exclusive.
            let query = oura.first_asked(endpoint);
            assert_eq!(query["start_date"], "2026-01-01", "{endpoint}");
            assert_eq!(query["end_date"], "2026-01-21", "{endpoint}");
        }
    }
    assert_eq!(
        result_of(&outcome, "daily_readiness").kind,
        ResultKind::Read
    );
    assert_eq!(
        result_of(&outcome, "daily_readiness").words,
        "1 document for Oura's days 2026-01-01 to 2026-01-20"
    );
    assert_eq!(result_of(&outcome, "heartrate").kind, ResultKind::Read);
    assert_eq!(result_of(&outcome, "workout").kind, ResultKind::Empty);
    assert!(placed(root, "r2").is_empty(), "not asked for");
    assert!(
        outcome
            .summary
            .contains("## From Oura\n\n- daily_readiness: read: 1 document"),
        "{}",
        outcome.summary
    );
    // Every request names who is asking.
    assert!(
        oura.gets()
            .iter()
            .all(|(_, _, authorization)| authorization == "Bearer access-0")
    );
    assert!(!root.join("imports/oura.json").exists());
}

#[test]
fn when_chunk_3_of_5_fails_coverage_holds_chunks_1_and_2() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let oura = Oura::new();
    // Five chunks of 364 days, the last of ten.
    let first = date("2020-01-01");
    let last = first + chrono::Days::new(4 * 364 + 9);
    let third = (first + chrono::Days::new(2 * 364))
        .format("%Y-%m-%d")
        .to_string();
    let wanted = third.clone();
    oura.rule(
        "daily_sleep",
        move |query| query.get("start_date") == Some(&wanted),
        503,
        json!({"detail": "unavailable"}),
        None,
    );
    let to = last.format("%Y-%m-%d").to_string();
    let outcome = finished(
        root,
        &oura,
        &ask("20261104_120000", Some(("2020-01-01", &to))),
    );
    let second_last = (first + chrono::Days::new(2 * 364 - 1))
        .format("%Y-%m-%d")
        .to_string();
    assert_eq!(
        covered(root, "daily_sleep"),
        json!([["2020-01-01", second_last]])
    );
    assert_eq!(
        covered(root, "daily_readiness"),
        json!([["2020-01-01", to]])
    );
    let report = result_of(&outcome, "daily_sleep");
    assert_eq!(report.kind, ResultKind::FailedPartWay);
    assert_eq!(
        report.words,
        format!(
            "read whole through Oura's day {second_last}, then Oura was busy or unavailable each time it was asked"
        )
    );
    // No chunk after the one that failed is asked.
    assert!(
        oura.gets()
            .iter()
            .filter(|(endpoint, ..)| endpoint == "daily_sleep")
            .all(|(_, query, _)| query["start_date"] <= third)
    );
}

fn one_day_workout(id: &str, start: &str, end: &str) -> Value {
    workout(id, "running", start, end)
}

#[test]
fn a_deferral_takes_back_a_day_an_earlier_run_covered_and_names_it_and_its_twin_stays() {
    for revised in [true, false] {
        let journal = connected("America/Denver");
        let root = journal.path();
        let range = Some(("2026-01-01", "2026-01-10"));
        let first = Oura::new();
        first.serve(
            "workout",
            vec![one_day_workout(
                "w1",
                "2026-01-05T08:00:00.000-07:00",
                "2026-01-05T08:30:00.000-07:00",
            )],
        );
        finished(root, &first, &ask("20261104_120000", range));
        let second = Oura::new();
        let start = if revised { "08:40" } else { "08:00" };
        let end = if revised { "09:10" } else { "08:30" };
        second.serve(
            "workout",
            vec![one_day_workout(
                "w1",
                &format!("2026-01-05T{start}:00.000-07:00"),
                &format!("2026-01-05T{end}:00.000-07:00"),
            )],
        );
        let outcome = finished(root, &second, &ask("20261104_120001", range));
        if revised {
            assert_eq!(
                covered(root, "workout"),
                json!([["2026-01-01", "2026-01-04"], ["2026-01-06", "2026-01-10"]])
            );
            assert_eq!(
                holes_of_file(root, "workout"),
                json!([{"at": "2026-01-05", "reason": "deferred"}])
            );
            assert!(
                outcome.summary.contains(
                    "## Not written whole\n\nOura answered for these, and the journal couldn't write all of it."
                ) && outcome.summary.contains(
                    "- workout, 2026-01-05: changed in Oura since; the journal keeps it as first written"
                ),
                "{}",
                outcome.summary
            );
        } else {
            assert_eq!(
                covered(root, "workout"),
                json!([["2026-01-01", "2026-01-10"]])
            );
            assert_eq!(holes_of_file(root, "workout"), Value::Null);
            assert!(!outcome.summary.contains("Not written whole"));
        }
    }
}

#[test]
fn a_failed_chunk_over_earlier_coverage_leaves_it_standing_and_a_write_hole_takes_it_back() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let range = Some(("2026-01-01", "2026-01-10"));
    finished(root, &Oura::new(), &ask("20261104_120000", range));
    assert_eq!(
        covered(root, "workout"),
        json!([["2026-01-01", "2026-01-10"]])
    );

    // Oura fails: what it said before still stands.
    let failing = Oura::new();
    failing.rule("workout", |_| true, 500, json!({}), None);
    let outcome = finished(root, &failing, &ask("20261104_120001", range));
    assert_eq!(result_of(&outcome, "workout").kind, ResultKind::Failed);
    assert_eq!(
        covered(root, "workout"),
        json!([["2026-01-01", "2026-01-10"]])
    );

    // Oura answers, and the journal can't write it whole: that day goes.
    let other = window(root, "20260104", "030000_3600");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join(BODY_RECORDS_FILE), "not rows\n").unwrap();
    let held = Oura::new();
    held.serve(
        "workout",
        vec![one_day_workout(
            "w1",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:20:00.000-07:00",
        )],
    );
    finished(root, &held, &ask("20261104_120002", range));
    assert_eq!(
        covered(root, "workout"),
        json!([["2026-01-01", "2026-01-04"], ["2026-01-06", "2026-01-10"]])
    );
    assert_eq!(
        holes_of_file(root, "workout"),
        json!([{"at": "2026-01-05", "reason": "held_back"}])
    );
}

#[test]
fn a_reading_held_back_by_an_owners_delete_is_no_hole_and_one_by_an_unreadable_place_is() {
    for deleted in [true, false] {
        let journal = connected("America/Denver");
        let root = journal.path();
        let range = Some(("2026-01-18", "2026-01-18"));
        // A workout's first window, at the reading's grid slot.
        let first = Oura::new();
        first.serve(
            "workout",
            vec![one_day_workout(
                "w1",
                "2026-01-18T14:20:00.000-07:00",
                "2026-01-18T14:25:00.000-07:00",
            )],
        );
        finished(root, &first, &ask("20261104_120000", range));
        if deleted {
            delete(root, "20260118", "142000_300");
        } else {
            fs::write(
                window(root, "20260118", "142000_300").join(BODY_RECORDS_FILE),
                "not rows\n",
            )
            .unwrap();
        }
        let second = Oura::new();
        second.serve(
            "heartrate",
            vec![heart("2026-01-18T21:22:00+00:00", 90, "workout")],
        );
        let outcome = finished(root, &second, &ask("20261104_120001", range));
        assert!(
            outcome.summary.contains("1 reading wasn't added"),
            "{deleted}: {}",
            outcome.summary
        );
        if deleted {
            assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
            assert_eq!(
                covered(root, "heartrate"),
                json!([["2026-01-18T07:00:00Z", "2026-01-19T07:00:00Z"]])
            );
        } else {
            assert_eq!(
                holes_of_file(root, "heartrate"),
                json!([{"at": "2026-01-18T21:00:00Z", "reason": "reading_held_back"}])
            );
            assert_eq!(
                covered(root, "heartrate"),
                json!([
                    ["2026-01-18T07:00:00Z", "2026-01-18T21:00:00Z"],
                    ["2026-01-18T22:00:00Z", "2026-01-19T07:00:00Z"]
                ])
            );
        }
    }
}

#[test]
fn a_heartrate_hour_that_cant_be_read_is_a_hole_until_a_later_run_writes_its_reading() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let range = Some(("2026-01-18", "2026-01-18"));
    let hour = window(root, "20260118", "100000_3600");
    fs::create_dir_all(&hour).unwrap();
    fs::write(hour.join(BODY_RECORDS_FILE), "not rows\n").unwrap();
    let oura = Oura::new();
    oura.serve(
        "heartrate",
        vec![heart("2026-01-18T17:20:00+00:00", 70, "awake")],
    );
    finished(root, &oura, &ask("20261104_120000", range));
    assert_eq!(
        holes_of_file(root, "heartrate"),
        json!([{"at": "2026-01-18T17:00:00Z", "reason": "unreadable"}])
    );
    fs::remove_file(hour.join(BODY_RECORDS_FILE)).unwrap();
    finished(root, &oura, &ask("20261104_120001", range));
    assert_eq!(holes_of_file(root, "heartrate"), Value::Null);
    assert_eq!(
        covered(root, "heartrate"),
        json!([["2026-01-18T07:00:00Z", "2026-01-19T07:00:00Z"]])
    );
    let rows = &stored(root)[&("20260118".to_owned(), "100000_3600".to_owned())];
    assert_eq!(rows.len(), 1);
}

#[test]
fn a_document_refused_for_size_is_named_under_refused_and_is_no_hole() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let mut big = one_day_workout(
        "w-big",
        "2026-01-05T08:00:00.000-07:00",
        "2026-01-05T08:30:00.000-07:00",
    );
    big["label"] = json!("x".repeat(40 * 1024));
    let oura = Oura::new();
    oura.serve("workout", vec![big]);
    let outcome = finished(
        root,
        &oura,
        &ask("20261104_120000", Some(("2026-01-01", "2026-01-10"))),
    );
    assert!(
        outcome
            .summary
            .contains("## Refused\n\n- workout: row_too_large: 1"),
        "{}",
        outcome.summary
    );
    assert!(outcome.holes.is_empty());
    assert_eq!(
        covered(root, "workout"),
        json!([["2026-01-01", "2026-01-10"]])
    );
}

#[test]
fn the_next_run_with_no_range_asks_from_seven_days_before_the_last_covered() {
    let journal = connected("America/Denver");
    let root = journal.path();
    finished(
        root,
        &Oura::new(),
        &ask("20261104_120000", Some(("2026-01-01", "2026-01-20"))),
    );
    let next = Oura::new();
    finished(root, &next, &ask("20261104_120001", None));
    for endpoint in solstone_core_import_sources::oura::ENDPOINTS {
        let query = next.first_asked(endpoint);
        if endpoint == "heartrate" {
            assert_eq!(query["start_datetime"], "2026-01-13T00:00:00-07:00");
        } else {
            assert_eq!(query["start_date"], "2026-01-13", "{endpoint}");
            // Through today, the frozen clock's.
            assert_eq!(query["end_date"], "2026-02-02", "{endpoint}");
        }
    }
    // With nothing covered, the last 30 days.
    let fresh = connected("America/Denver");
    let first = Oura::new();
    finished(fresh.path(), &first, &ask("20261104_120000", None));
    assert_eq!(first.first_asked("sleep")["start_date"], "2026-01-02");
}

/// The kill fixture: the slice's kill base and pages, served by Oura, plus a daily
/// document whose window can't be read.
fn fetch_kill_base(root: &Path, scratch: &Path) -> Oura {
    kill_base(root, scratch);
    let garbage = window(root, "20260120", "000000_86400");
    fs::create_dir_all(&garbage).unwrap();
    fs::write(garbage.join(BODY_RECORDS_FILE), "not rows\n").unwrap();
    let pages = kill_pages(scratch);
    let oura = Oura::new();
    let index: Value =
        serde_json::from_slice(&fs::read(pages.join("index.json")).unwrap()).unwrap();
    for entry in index["pages"].as_array().unwrap() {
        let page: Value =
            serde_json::from_slice(&fs::read(pages.join(entry["file"].as_str().unwrap())).unwrap())
                .unwrap();
        oura.serve(
            entry["endpoint"].as_str().unwrap(),
            page["data"].as_array().unwrap().clone(),
        );
    }
    // Heartrate's pages are two readings each, and Oura refuses the first page
    // token it is given once, so a chunk starts again.
    oura.0
        .borrow_mut()
        .page_size
        .insert("heartrate".to_owned(), 2);
    oura.rule(
        "heartrate",
        |query| query.contains_key("next_token"),
        400,
        json!({"detail": "invalid next_token"}),
        Some(1),
    );
    oura
}

/// Every file whose bytes say what the journal holds, for comparing a resumed run
/// with an uninterrupted one: the slice's snapshot, each day's `body.md`, the
/// coverage, and each import's record without its attempt's times and count.
fn fetched_snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    solstone_core_location::body_day::BodyDays::default().reconcile(root);
    let mut files = snapshot(root);
    for day in fs::read_dir(root.join("chronicle"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if let Ok(bytes) = fs::read(day.path().join("body.md")) {
            files.insert(
                format!("chronicle/{}/body.md", day.file_name().to_string_lossy()),
                bytes,
            );
        }
    }
    if let Ok(bytes) = fs::read(root.join(crate::oura_coverage::COVERAGE_REL)) {
        files.insert(crate::oura_coverage::COVERAGE_REL.to_owned(), bytes);
    }
    for import in fs::read_dir(root.join("imports")).unwrap().flatten() {
        let Ok(bytes) = fs::read(import.path().join("import.json")) else {
            continue;
        };
        let mut record: Value = serde_json::from_slice(&bytes).unwrap();
        if let Some(attempt) = record.get_mut("attempt").and_then(Value::as_object_mut) {
            for name in [
                "attempt_id",
                "generation",
                "started_at_ms",
                "finished_at_ms",
                "duration_ms",
            ] {
                attempt.remove(name);
            }
        }
        for name in ["task_id", "upload_timestamp", "imported_at"] {
            record.as_object_mut().unwrap().remove(name);
        }
        files.insert(
            format!(
                "imports/{}/import.json",
                import.file_name().to_string_lossy()
            ),
            record.to_string().into_bytes(),
        );
    }
    files
}

#[test]
fn a_fetched_run_stopped_anywhere_resumes_to_what_an_uninterrupted_run_writes() {
    let scratch = tempfile::tempdir().unwrap();
    let range = Some(("2026-01-01", "2026-01-31"));
    let whole = connected("America/Denver");
    let oura = fetch_kill_base(whole.path(), scratch.path());
    let uninterrupted = Ask {
        facet: Some("health"),
        ..ask("20261104_130000", range)
    };
    let done = finished(whole.path(), &oura, &uninterrupted);
    let expected = fetched_snapshot(whole.path());
    let reasons: BTreeSet<HoleReason> = done.holes.iter().map(|hole| hole.reason).collect();
    assert_eq!(
        reasons,
        BTreeSet::from([
            HoleReason::Deferred,
            HoleReason::HeldBack,
            HoleReason::Unreadable,
            HoleReason::RecordRefused,
            HoleReason::ReadingHeldBack,
        ]),
        "{:?}",
        done.holes
    );
    for hole in [
        ("heartrate", "2026-01-18T17:00:00Z", HoleReason::Unreadable),
        ("daily_readiness", "2026-01-20", HoleReason::Unreadable),
        (
            "heartrate",
            "2026-01-18T21:00:00Z",
            HoleReason::ReadingHeldBack,
        ),
        ("workout", "2026-01-24", HoleReason::RecordRefused),
    ] {
        assert!(
            done.holes.contains(&Hole {
                endpoint: hole.0.to_owned(),
                at: hole.1.to_owned(),
                reason: hole.2,
            }),
            "{hole:?}: {:?}",
            done.holes
        );
    }
    assert!(expected.contains_key(crate::oura_coverage::COVERAGE_REL));
    assert!(expected.keys().any(|path| path.ends_with("body.md")));

    // Seven date endpoints come before heartrate, so its pages land eighth on.
    for (point, nth) in [
        ("page-landed", 1),
        ("page-landed", 8),
        ("token-discarded", 1),
        ("page-landed", 10),
        ("fetched", 1),
        ("scanned", 1),
        ("placed", 1),
        ("pending", 1),
        ("pending", 3),
        ("written", 1),
        ("written", 2),
        ("published", 1),
        ("published", 4),
        ("written-all", 1),
        ("covered", 1),
        ("finished", 1),
    ] {
        let journal = connected("America/Denver");
        let root = journal.path();
        let oura = fetch_kill_base(root, scratch.path());
        let stop = stopping_at(point, nth);
        let stopped = Ask {
            stop_at: Some(&stop),
            ..uninterrupted
        };
        let error = import(root, &oura, &stopped).unwrap_err();
        assert_eq!(
            error,
            OuraImportError::Failed(format!("stopped at {point}")),
            "{point} {nth}"
        );
        let resumed = finished(
            root,
            &oura,
            &Ask {
                facet: Some("health"),
                ..ask("20261104_140000", range)
            },
        );
        assert_eq!(resumed.import_id, done.import_id, "{point} {nth}");
        assert!(!root.join("imports/20261104_140000").exists());
        assert!(!root.join(OURA.run_dir_rel).exists());
        assert_eq!(resumed.holes, done.holes, "{point} {nth}");
        assert_eq!(resumed.results, done.results, "{point} {nth}");
        let got = fetched_snapshot(root);
        assert_eq!(
            got.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>(),
            "{point} {nth}"
        );
        for (path, bytes) in &expected {
            assert_eq!(
                got.get(path).map(|bytes| String::from_utf8_lossy(bytes)),
                Some(String::from_utf8_lossy(bytes)),
                "{point} {nth}: {path}"
            );
        }
    }
}

#[test]
fn a_killed_run_claims_nothing_and_its_holes_are_taken_back_at_once() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let range = Some(("2026-01-01", "2026-01-10"));
    let first = Oura::new();
    first.serve(
        "workout",
        vec![one_day_workout(
            "w1",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:30:00.000-07:00",
        )],
    );
    finished(root, &first, &ask("20261104_120000", range));
    let revised = Oura::new();
    revised.serve(
        "workout",
        vec![one_day_workout(
            "w1",
            "2026-01-05T08:40:00.000-07:00",
            "2026-01-05T09:10:00.000-07:00",
        )],
    );
    // Stopped right after its scan's holes are saved: the deferred day is
    // already unclaimed.
    let stop = stopping_at("scanned", 1);
    import(
        root,
        &revised,
        &Ask {
            stop_at: Some(&stop),
            ..ask("20261104_120001", Some(("2026-01-01", "2026-01-20")))
        },
    )
    .unwrap_err();
    assert_eq!(
        covered(root, "workout"),
        json!([["2026-01-01", "2026-01-04"], ["2026-01-06", "2026-01-10"]])
    );
    assert_eq!(
        holes_of_file(root, "workout"),
        json!([{"at": "2026-01-05", "reason": "deferred"}])
    );

    // A run killed once its days are done adds nothing.
    let fresh = connected("America/Denver");
    let stop = stopping_at("written-all", 1);
    import(
        fresh.path(),
        &Oura::new(),
        &Ask {
            stop_at: Some(&stop),
            ..ask("20261104_120000", range)
        },
    )
    .unwrap_err();
    assert_eq!(coverage_file(fresh.path()), Value::Null);
}

#[test]
fn a_takeover_writes_the_stopped_runs_record_hole_and_an_explicit_range_clears_it() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let workouts = |days: &[u32]| {
        days.iter()
            .map(|day| {
                one_day_workout(
                    &format!("w{day}"),
                    &format!("2026-01-{day:02}T08:00:00.000-07:00"),
                    &format!("2026-01-{day:02}T08:20:00.000-07:00"),
                )
            })
            .collect::<Vec<_>>()
    };
    let health = |timestamp, range| Ask {
        facet: Some("health"),
        ..ask(timestamp, Some(range))
    };
    // Run 0 covers days 1 to 10, with nothing yet on day 3.
    let zero = Oura::new();
    zero.serve("workout", workouts(&[1, 2, 4, 5, 6, 7, 8, 9, 10]));
    finished(
        root,
        &zero,
        &health("20261104_120000", ("2026-01-01", "2026-01-10")),
    );
    assert_eq!(
        covered(root, "workout"),
        json!([["2026-01-01", "2026-01-10"]])
    );

    // Run 1 reads them again, and day 3's record is refused. It is stopped once
    // day 10 is checkpointed.
    let activities = root.join("facets/health/activities");
    fs::create_dir_all(activities.join("20260103.jsonl.lock")).unwrap();
    let one = Oura::new();
    one.serve("workout", workouts(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]));
    let stop = stopping_at("written-all", 1);
    import(
        root,
        &one,
        &Ask {
            stop_at: Some(&stop),
            ..health("20261104_130000", ("2026-01-01", "2026-01-10"))
        },
    )
    .unwrap_err();

    // Day 3 was taken back at its own checkpoint, before any later run.
    assert_eq!(
        covered(root, "workout"),
        json!([["2026-01-01", "2026-01-02"], ["2026-01-04", "2026-01-10"]])
    );
    // The records this run wrote arrived on the run's own day, by its clock.
    let record: Value = serde_json::from_str(
        fs::read_to_string(activities.join("20260104.jsonl"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(record["account"]["arrived"], "20260201");

    // Run 2 takes over with other pages, over days that leave out day 3.
    let two = Oura::new();
    two.serve("workout", workouts(&[12]));
    let taken = finished(
        root,
        &two,
        &health("20261104_140000", ("2026-01-12", "2026-01-13")),
    );
    assert_ne!(taken.import_id, "20261104_130000");
    let hole = Hole {
        endpoint: "workout".to_owned(),
        at: "2026-01-03".to_owned(),
        reason: HoleReason::RecordRefused,
    };
    assert!(taken.holes.contains(&hole), "{:?}", taken.holes);
    assert!(
        taken
            .summary
            .contains("- workout, 2026-01-03: it couldn't be shown on its day"),
        "{}",
        taken.summary
    );
    assert_eq!(
        covered(root, "workout"),
        json!([
            ["2026-01-01", "2026-01-02"],
            ["2026-01-04", "2026-01-10"],
            ["2026-01-12", "2026-01-13"]
        ])
    );
    assert_eq!(
        holes_of_file(root, "workout"),
        json!([{"at": "2026-01-03", "reason": "record_refused"}])
    );

    // Once the record can be written, a range over day 3 writes it and clears it.
    fs::remove_dir(activities.join("20260103.jsonl.lock")).unwrap();
    finished(
        root,
        &one,
        &health("20261104_150000", ("2026-01-03", "2026-01-03")),
    );
    assert!(
        fs::read_to_string(activities.join("20260103.jsonl"))
            .unwrap()
            .contains("\"activity\":\"workout\"")
    );
    assert_eq!(holes_of_file(root, "workout"), Value::Null);
    assert_eq!(
        covered(root, "workout"),
        json!([["2026-01-01", "2026-01-10"], ["2026-01-12", "2026-01-13"]])
    );
}

#[test]
fn a_restored_run_carries_its_hole_and_with_none_the_earlier_claim_stands() {
    for revised in [true, false] {
        let journal = connected("America/Denver");
        let root = journal.path();
        let range = Some(("2026-01-01", "2026-01-10"));
        let first = Oura::new();
        first.serve(
            "workout",
            vec![one_day_workout(
                "w1",
                "2026-01-05T08:00:00.000-07:00",
                "2026-01-05T08:30:00.000-07:00",
            )],
        );
        finished(root, &first, &ask("20261104_120000", range));
        let second = Oura::new();
        let (start, end) = if revised {
            ("08:40", "09:10")
        } else {
            ("08:00", "08:30")
        };
        second.serve(
            "workout",
            vec![one_day_workout(
                "w1",
                &format!("2026-01-05T{start}:00.000-07:00"),
                &format!("2026-01-05T{end}:00.000-07:00"),
            )],
        );
        // Stopped in its write phase, its deferral saved.
        let stop = stopping_at("placed", 1);
        import(
            root,
            &second,
            &Ask {
                stop_at: Some(&stop),
                ..ask("20261104_120001", range)
            },
        )
        .unwrap_err();
        // A restore leaves the staging out.
        for folder in ["placed", "pages"] {
            fs::remove_dir_all(root.join(OURA.run_dir_rel).join(folder)).unwrap();
        }
        let repaired = finished(
            root,
            &Oura::new(),
            &ask("20261104_120002", Some(("2026-01-12", "2026-01-13"))),
        );
        assert!(
            repaired.summary.contains("published again"),
            "{}",
            repaired.summary
        );
        if revised {
            assert!(
                repaired
                    .summary
                    .contains("- workout, 2026-01-05: changed in Oura since"),
                "{}",
                repaired.summary
            );
            assert_eq!(
                covered(root, "workout"),
                json!([
                    ["2026-01-01", "2026-01-04"],
                    ["2026-01-06", "2026-01-10"],
                    ["2026-01-12", "2026-01-13"]
                ])
            );
            assert_eq!(
                holes_of_file(root, "workout"),
                json!([{"at": "2026-01-05", "reason": "deferred"}])
            );
        } else {
            assert!(!repaired.summary.contains("Not written whole"));
            assert_eq!(
                covered(root, "workout"),
                json!([["2026-01-01", "2026-01-10"], ["2026-01-12", "2026-01-13"]])
            );
        }
    }
}

#[test]
fn a_run_state_that_cant_be_read_takes_back_every_oura_claim() {
    let journal = connected("America/Denver");
    let root = journal.path();
    finished(
        root,
        &Oura::new(),
        &ask("20261104_120000", Some(("2026-01-01", "2026-01-10"))),
    );
    fs::create_dir_all(root.join(OURA.run_dir_rel)).unwrap();
    fs::write(root.join(OURA.run_dir_rel).join(STATE_FILE), "not a state").unwrap();
    finished(
        root,
        &Oura::new(),
        &ask("20261104_120001", Some(("2026-01-12", "2026-01-13"))),
    );
    for endpoint in solstone_core_import_sources::oura::ENDPOINTS {
        let expected = if endpoint == "heartrate" {
            json!([["2026-01-12T07:00:00Z", "2026-01-14T07:00:00Z"]])
        } else {
            json!([["2026-01-12", "2026-01-13"]])
        };
        assert_eq!(covered(root, endpoint), expected, "{endpoint}");
    }
}

#[test]
fn a_window_that_wont_publish_still_counts_and_saving_it_later_changes_no_coverage() {
    let range = Some(("2026-01-01", "2026-01-10"));
    let serve = || {
        let oura = Oura::new();
        oura.serve(
            "workout",
            vec![
                // A window bound before it, as the stream's first.
                one_day_workout(
                    "w0",
                    "2026-01-04T06:30:00.000-07:00",
                    "2026-01-04T06:35:00.000-07:00",
                ),
                one_day_workout(
                    "w1",
                    "2026-01-05T06:30:00.000-07:00",
                    "2026-01-05T06:35:00.000-07:00",
                ),
            ],
        );
        oura
    };
    let stuck = |root: &Path| {
        let dir = window(root, "20260105", "063000_300");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("stream.json"),
            json!({"stream": STREAM, "prev_day": null, "prev_segment": null, "seq": 999})
                .to_string(),
        )
        .unwrap();
        dir
    };
    let whole = connected("America/Denver");
    stuck(whole.path());
    let partly = import(whole.path(), &serve(), &ask("20261104_120000", range)).unwrap();
    assert!(!partly.finished);
    assert!(
        partly.summary.contains("20260105 063000_300: bind"),
        "{}",
        partly.summary
    );
    assert_eq!(
        covered(whole.path(), "workout"),
        json!([["2026-01-01", "2026-01-10"]])
    );
    let expected = fs::read(whole.path().join(crate::oura_coverage::COVERAGE_REL)).unwrap();

    // Stopped once its retry state is saved: the same coverage.
    let journal = connected("America/Denver");
    let root = journal.path();
    let marker = stuck(root);
    let stop = stopping_at("retry-saved", 1);
    let error = import(
        root,
        &serve(),
        &Ask {
            stop_at: Some(&stop),
            ..ask("20261104_120000", range)
        },
    )
    .unwrap_err();
    assert_eq!(
        error,
        OuraImportError::Failed("stopped at retry-saved".to_owned())
    );
    assert_eq!(
        fs::read(root.join(crate::oura_coverage::COVERAGE_REL)).unwrap(),
        expected
    );

    // The run that saves the window leaves the file as it was.
    fs::remove_file(marker.join("stream.json")).unwrap();
    finished(root, &serve(), &ask("20261104_120001", range));
    assert!(marker.join("stream.json").is_file());
    assert_eq!(
        fs::read(root.join(crate::oura_coverage::COVERAGE_REL)).unwrap(),
        expected
    );
}

#[test]
fn a_reading_oura_reclassifies_across_a_page_token_restart_is_one_row_with_the_later_source() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let range = Some(("2026-01-05", "2026-01-05"));
    let oura = Oura::new();
    oura.serve(
        "heartrate",
        vec![
            heart("2026-01-05T17:00:00+00:00", 61, "rest"),
            heart("2026-01-05T17:05:00+00:00", 62, "awake"),
            heart("2026-01-05T18:00:00+00:00", 63, "awake"),
            heart("2026-01-05T18:00:00+00:00", 74, "live"),
        ],
    );
    oura.0
        .borrow_mut()
        .page_size
        .insert("heartrate".to_owned(), 1);
    // Stopped as its second heartrate page lands; the first is staged with a
    // page token.
    let stop = stopping_at("page-landed", 9);
    import(
        root,
        &oura,
        &Ask {
            stop_at: Some(&stop),
            ..ask("20261104_120000", range)
        },
    )
    .unwrap_err();
    // Oura then refuses its old page tokens and calls the first reading a workout.
    {
        let mut server = oura.0.borrow_mut();
        server.generation += 1;
        server.documents.get_mut("heartrate").unwrap()[0]["source"] = json!("workout");
    }
    finished(root, &oura, &ask("20261104_120001", range));
    let rows: Vec<BodyRow> = stored(root).into_values().flatten().collect();
    let at = |instant: &str| -> Vec<&BodyRow> {
        rows.iter()
            .filter(|row| row.at.as_deref() == Some(instant))
            .collect()
    };
    let reclassified = at("2026-01-05T17:00:00+00:00");
    assert_eq!(reclassified.len(), 1, "{rows:?}");
    assert_eq!(reclassified[0].fields["source"], json!("workout"));
    // Twin: two real sources at one instant stay two.
    assert_eq!(at("2026-01-05T18:00:00+00:00").len(), 2);
    assert!(
        oura.gets()
            .iter()
            .any(|(endpoint, query, _)| endpoint == "heartrate" && query.contains_key("next_token"))
    );
}

#[test]
fn heartrate_is_never_asked_for_a_span_whose_dates_are_more_than_30_apart() {
    let journal = connected("Europe/London");
    let root = journal.path();
    let oura = Oura::new();
    finished(
        root,
        &oura,
        &ask("20261104_120000", Some(("2026-01-01", "2026-12-31"))),
    );
    let spans: Vec<(DateTime<chrono::FixedOffset>, DateTime<chrono::FixedOffset>)> = oura
        .gets()
        .into_iter()
        .filter(|(endpoint, ..)| endpoint == "heartrate")
        .map(|(_, query, _)| {
            (
                DateTime::parse_from_rfc3339(&query["start_datetime"]).unwrap(),
                DateTime::parse_from_rfc3339(&query["end_datetime"]).unwrap(),
            )
        })
        .collect();
    assert!(spans.len() >= 12);
    for (from, to) in &spans {
        let written = (to.date_naive() - from.date_naive()).num_days();
        let utc = (to.to_utc().date_naive() - from.to_utc().date_naive()).num_days();
        assert!(written <= 30 && utc <= 30, "{from} {to}");
    }
    // Half-open spans meeting end to start, over exactly the days asked.
    for pair in spans.windows(2) {
        assert_eq!(pair[0].1, pair[1].0);
    }
    assert_eq!(spans[0].0.to_rfc3339(), "2026-01-01T00:00:00+00:00");
    assert_eq!(
        spans.last().unwrap().1.to_rfc3339(),
        "2027-01-01T00:00:00+00:00"
    );
}

#[test]
fn each_endpoint_ends_in_exactly_one_result_and_a_refused_one_makes_the_import_partly_failed() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let oura = Oura::new();
    let second_chunk = |query: &BTreeMap<String, String>| {
        query.get("start_date").map(String::as_str) == Some("2025-12-31")
    };
    oura.serve("daily_readiness", vec![readiness("r1", "2026-01-05", 80)])
        .rule(
            "daily_stress",
            |_| true,
            403,
            json!({"detail": "Missing Scope Permission"}),
            None,
        )
        .rule(
            "heartrate",
            |_| true,
            403,
            json!({"detail": "Your token must have the scopes listed: heartrate"}),
            None,
        )
        .rule(
            "sleep",
            |_| true,
            403,
            json!({"detail": "Missing Scope Permission"}),
            None,
        )
        .rule("workout", |_| true, 403, json!({"detail": "nope"}), None)
        .rule(
            "session",
            second_chunk,
            403,
            json!({"detail": "missing permission: session"}),
            None,
        )
        .rule("daily_activity", second_chunk, 500, json!({}), None)
        .rule("daily_spo2", |_| true, 500, json!({}), None)
        .rule("daily_resilience", |_| true, 418, json!({}), None);
    let outcome = finished(
        root,
        &oura,
        &ask("20261104_120000", Some(("2025-01-01", "2026-01-31"))),
    );
    let expect = [
        (
            "daily_readiness",
            ResultKind::Read,
            "1 document for Oura's days 2025-01-01 to 2026-01-31",
        ),
        (
            "daily_sleep",
            ResultKind::Empty,
            "nothing for Oura's days 2025-01-01 to 2026-01-31",
        ),
        (
            "daily_stress",
            ResultKind::Refused,
            "Oura didn't say which permission this needs",
        ),
        (
            "heartrate",
            ResultKind::Refused,
            "Oura says this needs the heartrate permission",
        ),
        (
            "sleep",
            ResultKind::Refused,
            "this needs the daily permission",
        ),
        ("workout", ResultKind::Refused, "Oura said: nope"),
        (
            "session",
            ResultKind::RefusedPartWay,
            "read whole through Oura's day 2025-12-30, then refused: Oura says this needs the session permission",
        ),
        (
            "daily_activity",
            ResultKind::FailedPartWay,
            "read whole through Oura's day 2025-12-30, then Oura was busy or unavailable each time it was asked",
        ),
        (
            "daily_spo2",
            ResultKind::Failed,
            "Oura was busy or unavailable each time it was asked",
        ),
        (
            "daily_resilience",
            ResultKind::Failed,
            "Oura answered with error 418",
        ),
    ];
    for (endpoint, kind, words) in expect {
        let report = result_of(&outcome, endpoint);
        assert_eq!(
            (report.kind, report.words.as_str()),
            (kind, words),
            "{endpoint}"
        );
        assert!(
            outcome.summary.contains(&report.line()),
            "{}",
            outcome.summary
        );
    }
    assert_eq!(outcome.results.len(), 13);
    // A refused part-way endpoint keeps its whole chunk; a refused one, nothing.
    assert_eq!(
        covered(root, "session"),
        json!([["2025-01-01", "2025-12-30"]])
    );
    assert_eq!(covered(root, "sleep"), Value::Null);
    let projection = solstone_core_import::project_import_result(root, &outcome.import_id);
    assert!(projection.has_gaps, "{projection:?}");
    assert_eq!(
        projection.attempt.and_then(|facts| facts.input_failures),
        Some(8)
    );
}

#[test]
fn a_lapsed_membership_stops_the_run_and_writes_nothing() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let oura = Oura::new();
    oura.serve(
        "workout",
        vec![one_day_workout(
            "w1",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:20:00.000-07:00",
        )],
    )
    .rule(
        "daily_sleep",
        |_| true,
        403,
        json!({"detail": "The user's Oura membership has expired"}),
        None,
    );
    let error = import(
        root,
        &oura,
        &ask("20261104_120000", Some(("2026-01-01", "2026-01-10"))),
    )
    .unwrap_err();
    assert_eq!(error, OuraImportError::Stop(OuraStop::Membership));
    assert!(stored(root).is_empty());
    assert_eq!(coverage_file(root), Value::Null);
}

#[test]
fn a_401_after_the_one_refresh_is_the_authorization_stop_and_no_endpoint_reads_refused() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let oura = Oura::new();
    oura.expire("access-0");
    oura.0.borrow_mut().refuse_issued = true;
    let error = import(
        root,
        &oura,
        &ask("20261104_120000", Some(("2026-01-01", "2026-01-10"))),
    )
    .unwrap_err();
    assert_eq!(error, OuraImportError::Stop(OuraStop::Authorization));
    assert_eq!(
        error.to_string(),
        "Oura's authorization needs renewing: connect Oura again."
    );
    assert_eq!(oura.posts(), 1);
    let state = fs::read_to_string(root.join(OURA.run_dir_rel).join(STATE_FILE)).unwrap();
    let state: Value = serde_json::from_str(&state).unwrap();
    assert!(
        state["fetch"]["endpoints"]
            .as_object()
            .unwrap()
            .values()
            .all(|plan| plan.get("result").is_none()),
        "{state}"
    );
    assert_eq!(coverage_file(root), Value::Null);
    assert!(stored(root).is_empty());
}

#[test]
fn an_old_sync_holding_its_lock_stops_a_run_that_must_refresh_and_it_carries_on_later() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let oura = Oura::new();
    oura.expire("access-0");
    let range = Some(("2026-01-01", "2026-01-10"));
    {
        let _old_sync = hold_oura_lock(root).unwrap();
        let error = import(root, &oura, &ask("20261104_120000", range)).unwrap_err();
        assert_eq!(error, OuraImportError::Stop(OuraStop::AnotherSync));
        assert_eq!(oura.posts(), 0, "no refresh without the lock");
    }
    let resumed = finished(root, &oura, &ask("20261104_120001", range));
    assert_eq!(resumed.import_id, "20261104_120000");
    assert_eq!(oura.posts(), 1);
}

fn approve(root: &Path) {
    fs::create_dir_all(root.join("imports/_approvals")).unwrap();
    fs::write(
        root.join("imports/_approvals/oura_sync_preflight.json"),
        json!({
            "schema": "solstone.oura_sync_preflight.v1",
            "checklist_version": "solstone.oura_sync_preflight.checklist.v2",
            "journal_root": root.canonicalize().unwrap(),
            "requires_per_run_confirmation": true,
            "replication_destinations": {
                "time_machine": {"decision": "excluded"},
                "icloud": {"decision": "excluded"},
                "solbase": {"decision": "excluded"},
                "hosted_backup": {"decision": "excluded"},
                "other": {"decision": "excluded"}
            },
            "raw_retention": {"decision": "discard"}
        })
        .to_string(),
    )
    .unwrap();
}

/// The old sync, saving, as the owner's command runs it.
fn old_sync(root: &Path, oura: &Oura) -> Result<(), BodyIngestError> {
    let mut transport = oura.clone();
    sync_with_http(
        root,
        &OuraSyncOptions {
            save: true,
            confirm_body_save: true,
            today: Some(date("2026-02-01")),
            ..OuraSyncOptions::default()
        },
        &mut transport,
    )
    .map(|_| ())
}

#[test]
fn a_refresh_whose_tokens_cant_be_saved_loses_the_connection_on_both_paths() {
    let journal = connected("America/Denver");
    let root = journal.path();
    approve(root);
    let oura = Oura::new();
    oura.expire("access-0");
    let range = Some(("2026-01-01", "2026-01-10"));
    let config = fs::read(root.join("config/journal.json")).unwrap();
    // Oura answers the refresh, and the journal can't write its config: what a
    // kill between the answer and the write leaves.
    let blocked = root.join("config/journal.json.lock");
    fs::create_dir_all(&blocked).unwrap();
    let error = import(root, &oura, &ask("20261104_120000", range)).unwrap_err();
    assert!(
        matches!(&error, OuraImportError::Failed(words) if words.contains("couldn't save it")),
        "{error:?}"
    );
    assert_eq!(oura.posts(), 1);
    assert_eq!(fs::read(root.join("config/journal.json")).unwrap(), config);
    fs::remove_dir(&blocked).unwrap();

    // The next run offers the spent refresh token, which Oura refuses.
    let error = import(root, &oura, &ask("20261104_120001", range)).unwrap_err();
    assert_eq!(error, OuraImportError::Stop(OuraStop::Authorization));
    assert!(stored(root).is_empty());
    assert_eq!(coverage_file(root), Value::Null);

    // The old sync, in the same state, fails as it does today.
    let old = old_sync(root, &oura).unwrap_err();
    assert_eq!(old.stage(), "authorization_refresh");
    assert_eq!(old.kind(), BodyIngestErrorKind::Source);
}

#[test]
fn the_new_path_never_reads_the_old_cursor_poisoned_or_not() {
    let mut first_asks = Vec::new();
    for cursor in [
        None,
        Some(
            json!({
                "schema": "solstone.import_sync.oura.v1",
                "endpoints": {"sleep": {"high_water_day": "2099-12-31", "backfill_complete": true}},
            })
            .to_string(),
        ),
        Some("not json at all".to_owned()),
    ] {
        let journal = connected("America/Denver");
        let root = journal.path();
        fs::create_dir_all(root.join("imports")).unwrap();
        if let Some(cursor) = &cursor {
            fs::write(root.join("imports/oura.json"), cursor).unwrap();
        }
        let oura = Oura::new();
        finished(root, &oura, &ask("20261104_120000", None));
        first_asks.push((oura.first_asked("sleep"), oura.first_asked("heartrate")));
        match &cursor {
            Some(cursor) => assert_eq!(
                fs::read_to_string(root.join("imports/oura.json")).unwrap(),
                *cursor
            ),
            None => assert!(!root.join("imports/oura.json").exists()),
        }
    }
    assert_eq!(first_asks[0].0["start_date"], "2026-01-02");
    assert!(first_asks.iter().all(|asked| *asked == first_asks[0]));
}

#[test]
fn the_old_sync_never_reads_the_new_paths_files() {
    let mut asked = Vec::new();
    for poisoned in [false, true] {
        let journal = connected("America/Denver");
        let root = journal.path();
        approve(root);
        if poisoned {
            fs::create_dir_all(root.join(OURA.run_dir_rel).join("pages")).unwrap();
            fs::write(root.join(crate::oura_coverage::COVERAGE_REL), "not json").unwrap();
            fs::write(root.join(OURA.run_dir_rel).join(STATE_FILE), "not json").unwrap();
            fs::write(
                root.join(OURA.run_dir_rel).join("pages/index.json"),
                "not json",
            )
            .unwrap();
        }
        let oura = Oura::new();
        old_sync(root, &oura).unwrap();
        asked.push(oura.gets());
        if poisoned {
            assert_eq!(
                fs::read_to_string(root.join(crate::oura_coverage::COVERAGE_REL)).unwrap(),
                "not json"
            );
        }
    }
    assert_eq!(asked[0], asked[1]);
}

#[test]
fn a_path_whose_tokens_another_path_refreshed_uses_them_with_no_second_refresh() {
    let journal = connected("America/Denver");
    let root = journal.path().to_path_buf();
    approve(&root);
    let oura = Oura::new();
    // The new path has loaded its tokens. Before its first request is answered,
    // the old sync runs, finds them expired and refreshes.
    let during = oura.clone();
    let path = root.clone();
    oura.0.borrow_mut().hooks.push((
        0,
        Box::new(move || {
            during.expire("access-0");
            old_sync(&path, &during).unwrap();
        }),
    ));
    let outcome = finished(
        &root,
        &oura,
        &ask("20261104_120000", Some(("2026-01-01", "2026-01-10"))),
    );
    assert_eq!(oura.posts(), 1, "one refresh, the old sync's");
    assert!(
        outcome
            .results
            .iter()
            .all(|report| report.kind == ResultKind::Empty)
    );
    let config: Value =
        serde_json::from_slice(&fs::read(root.join("config/journal.json")).unwrap()).unwrap();
    assert_eq!(config["oura"]["tokens"]["refresh_token"], "refresh-1");
}

#[test]
fn an_old_sync_that_loaded_first_refreshes_alone_and_the_new_path_then_uses_its_tokens() {
    let journal = connected("America/Denver");
    let root = journal.path().to_path_buf();
    approve(&root);
    let oura = Oura::new();
    oura.expire("access-0");
    // The old sync holds its lock and has loaded its tokens. Before its first
    // request is answered, the new path runs: it must refresh, and can't.
    let during = oura.clone();
    let path = root.clone();
    let waited = Rc::new(RefCell::new(None));
    let seen = Rc::clone(&waited);
    oura.0.borrow_mut().hooks.push((
        0,
        Box::new(move || {
            *seen.borrow_mut() = Some(import(
                &path,
                &during,
                &ask("20261104_120000", Some(("2026-01-01", "2026-01-10"))),
            ));
        }),
    ));
    old_sync(&root, &oura).unwrap();
    assert!(matches!(
        waited.borrow().as_ref(),
        Some(Err(OuraImportError::Stop(OuraStop::AnotherSync)))
    ));
    assert_eq!(oura.posts(), 1, "only the old sync refreshed");
    // Run again, the new path carries on with the old sync's rotated tokens.
    finished(
        &root,
        &oura,
        &ask("20261104_120001", Some(("2026-01-01", "2026-01-10"))),
    );
    assert_eq!(oura.posts(), 1);
}

#[test]
fn a_new_path_refresh_is_what_the_old_sync_loads_next() {
    let journal = connected("America/Denver");
    let root = journal.path();
    approve(root);
    let oura = Oura::new();
    oura.expire("access-0");
    finished(
        root,
        &oura,
        &ask("20261104_120000", Some(("2026-01-01", "2026-01-10"))),
    );
    assert_eq!(oura.posts(), 1);
    // The rotated access token expires too: the old sync refreshes with the
    // rotated refresh token, which Oura still takes.
    oura.expire("access-1");
    old_sync(root, &oura).unwrap();
    assert_eq!(oura.posts(), 2);
    assert_eq!(oura.0.borrow().posts[1]["refresh_token"], "refresh-1");
}

#[test]
fn a_reading_at_a_chunks_end_comes_from_one_chunk_whichever_way_oura_reads_the_end() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let oura = Oura::new();
    // The instant the first 30-day chunk ends and the second begins.
    oura.serve(
        "heartrate",
        vec![heart("2026-01-31T07:00:00+00:00", 64, "awake")],
    );
    oura.0.borrow_mut().heartrate_inclusive = true;
    let outcome = finished(
        root,
        &oura,
        &ask("20261104_120000", Some(("2026-01-01", "2026-02-05"))),
    );
    let rows: Vec<BodyRow> = stored(root)
        .into_values()
        .flatten()
        .filter(|row| row.kind == RowKind::Reading)
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        result_of(&outcome, "heartrate").words,
        "1 reading for 2026-01-01 to 2026-02-05"
    );
}

/// A day later than the frozen clock's.
fn later() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 2, 4, 19, 0, 0).unwrap()
}

#[test]
fn a_run_with_no_range_resumed_on_a_later_day_asks_for_the_days_it_first_worked_out() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let oura = Oura::new();
    let stop = stopping_at("page-landed", 3);
    import(
        root,
        &oura,
        &Ask {
            stop_at: Some(&stop),
            ..ask("20261104_120000", None)
        },
    )
    .unwrap_err();
    let resumed = finished(
        root,
        &oura,
        &Ask {
            clock: Some(&later),
            ..ask("20261104_130000", None)
        },
    );
    assert_eq!(resumed.import_id, "20261104_120000");
    assert!(
        oura.gets()
            .iter()
            .filter(|(endpoint, ..)| endpoint != "heartrate")
            .all(|(_, query, _)| query["start_date"] == "2026-01-02"
                && query["end_date"] == "2026-02-02"),
        "{:?}",
        oura.gets()
    );
    assert_eq!(
        covered(root, "sleep"),
        json!([["2026-01-02", "2026-02-01"]])
    );
}

#[test]
fn a_fetched_run_whose_pages_went_before_its_scan_repairs_and_finishes() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let oura = Oura::new();
    oura.serve(
        "workout",
        vec![one_day_workout(
            "w1",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:20:00.000-07:00",
        )],
    );
    let range = Some(("2026-01-01", "2026-01-10"));
    let stop = stopping_at("fetched", 1);
    import(
        root,
        &oura,
        &Ask {
            stop_at: Some(&stop),
            ..ask("20261104_120000", range)
        },
    )
    .unwrap_err();
    fs::remove_dir_all(root.join(OURA.run_dir_rel).join("pages")).unwrap();
    let repaired = finished(root, &oura, &ask("20261104_130000", range));
    assert_ne!(repaired.import_id, "20261104_120000");
    assert_eq!(placed(root, "w1").len(), 1);
    assert_eq!(
        covered(root, "workout"),
        json!([["2026-01-01", "2026-01-10"]])
    );
}
