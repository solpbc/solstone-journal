// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! `GET /app/open/<t>`: what a journal link opens, and what it says when it
//! can't.

use std::fs;
use std::path::Path;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use solstone_core_convey_shell::open_link::open_link_router;
use solstone_core_convey_shell::router;
use solstone_core_journal_link::{Place, seal};
use tempfile::{TempDir, tempdir};
use tower::ServiceExt;

const DAY: &str = "20260901";
const WORK: &str = "a0000000-0000-4000-8000-000000000001";
const HOME: &str = "a0000000-0000-4000-8000-000000000002";
const NOT_OPENED: &str = "this link can't be opened here.";
const ABSENT: &str = "it isn't in your journal.";

fn write_file(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().expect("parent exists")).expect("parent created");
    fs::write(path, content).expect("file written");
}

fn write_json(path: &Path, value: &Value) {
    write_file(path, &format!("{value}\n"));
}

fn write_root_key(journal: &Path, hex_byte: &str) {
    write_file(
        &journal.join("link/root.key"),
        &format!("{}\n", hex_byte.repeat(32)),
    );
}

fn segment(journal: &Path, day: &str, stream: Option<&str>, key: &str) {
    let mut dir = journal.join("chronicle").join(day);
    if let Some(stream) = stream {
        dir = dir.join(stream);
    }
    write_file(&dir.join(key).join("audio.jsonl"), "{\"start\":0.0}\n");
}

fn person(journal: &Path, id: &str, extra: Value) {
    let mut identity = json!({"id": id, "name": id, "type": "Person"});
    identity
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().cloned().unwrap_or_default());
    write_json(
        &journal.join("entities").join(id).join("entity.json"),
        &identity,
    );
}

/// A journal with one of each kind of place, and its own key.
fn journal() -> TempDir {
    let temp = tempdir().unwrap();
    let root = temp.path();
    write_root_key(root, "0f");
    segment(root, DAY, Some("desk"), "100000_300");
    segment(root, DAY, Some("phone"), "100000_300");
    segment(root, DAY, None, "110000_300");
    write_json(&root.join("facets/work/facet.json"), &json!({"id": WORK}));
    write_json(&root.join("facets/home/facet.json"), &json!({"id": HOME}));
    write_file(
        &root.join(format!("facets/work/activities/{DAY}.jsonl")),
        &[
            json!({"id": "meeting_100000_300", "segments": ["100000_300"]}),
            json!({"id": "walk_1", "hidden": true}),
            json!({"id": "plan_1", "source": "anticipated"}),
            json!({"id": "owner_made_1", "source": "user"}),
        ]
        .iter()
        .map(|row| format!("{row}\n"))
        .collect::<String>(),
    );
    person(root, "benvolio_montague", json!({}));
    person(root, "tybalt_capulet", json!({"blocked": true}));
    temp
}

fn day(day: &str) -> Place {
    Place::Day {
        day: day.to_owned(),
    }
}

fn recording(stream: &str, segment: &str) -> Place {
    Place::Recording {
        day: DAY.to_owned(),
        stream: stream.to_owned(),
        segment: segment.to_owned(),
    }
}

fn activity(facet: &str, id: &str) -> Place {
    Place::Activity {
        facet: facet.to_owned(),
        day: DAY.to_owned(),
        id: id.to_owned(),
    }
}

fn person_place(id: &str) -> Place {
    Place::Person { id: id.to_owned() }
}

async fn get(app: axum::Router, path: &str) -> (StatusCode, String, Option<String>) {
    let request = Request::builder().uri(path).body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        String::from_utf8_lossy(&bytes).into_owned(),
        location,
    )
}

async fn open(journal: &Path, link: &str) -> (StatusCode, String, Option<String>) {
    get(
        open_link_router(journal.to_path_buf()),
        &format!("/app/open/{link}"),
    )
    .await
}

async fn opens_to(journal: &Path, place: &Place, moment: &str) -> String {
    let link = format!("{}{moment}", seal(journal, place).unwrap());
    let (status, body, location) = open(journal, &link).await;
    assert_eq!(status, StatusCode::FOUND, "{place:?}{moment}: {body}");
    location.expect("a redirect names its place")
}

/// The whole page, so two refusals can be compared for sameness.
async fn refuses(journal: &Path, place: &Place) -> (StatusCode, String) {
    let (status, body, location) = open(journal, &seal(journal, place).unwrap()).await;
    assert_eq!(location, None, "{place:?}");
    (status, body)
}

async fn is_absent(journal: &Path, place: &Place) {
    let (status, body) = refuses(journal, place).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{place:?}");
    assert!(body.contains(ABSENT), "{place:?}: {body}");
}

#[tokio::test]
async fn each_kind_of_place_opens_its_own_page() {
    let journal = journal();
    let root = journal.path();
    assert_eq!(
        opens_to(root, &recording("desk", "100000_300"), "").await,
        format!("/app/transcripts/{DAY}?stream=desk#100000_300")
    );
    // A segment key is unique only within a stream: the link names which.
    assert_eq!(
        opens_to(root, &recording("phone", "100000_300"), "").await,
        format!("/app/transcripts/{DAY}?stream=phone#100000_300")
    );
    assert_eq!(
        opens_to(root, &recording("_default", "110000_300"), "").await,
        format!("/app/transcripts/{DAY}?stream=_default#110000_300")
    );
    assert_eq!(
        opens_to(root, &activity("work", "meeting_100000_300"), "").await,
        format!("/app/activities/{DAY}?facet=work#work/meeting_100000_300")
    );
    // One the owner made has no recording, and still opens.
    assert_eq!(
        opens_to(root, &activity("work", "owner_made_1"), "").await,
        format!("/app/activities/{DAY}?facet=work#work/owner_made_1")
    );
    assert_eq!(
        opens_to(root, &person_place("benvolio_montague"), "").await,
        "/app/entities/#benvolio_montague"
    );
}

#[tokio::test]
async fn a_day_opens_on_its_activities_or_else_its_recordings() {
    let journal = journal();
    let root = journal.path();
    assert_eq!(
        opens_to(root, &day(DAY), "").await,
        format!("/app/activities/{DAY}")
    );
    segment(root, "20260902", Some("desk"), "090000_300");
    assert_eq!(
        opens_to(root, &day("20260902"), "").await,
        "/app/transcripts/20260902"
    );
    is_absent(root, &day("20260903")).await;
    is_absent(root, &day("20260931")).await;
    // A day whose only activity is hidden is no more there than an empty one.
    write_file(
        &root.join("facets/work/activities/20260904.jsonl"),
        &format!("{}\n", json!({"id": "nap_1", "hidden": true})),
    );
    assert_eq!(
        refuses(root, &day("20260904")).await,
        refuses(root, &day("20260903")).await
    );
}

#[tokio::test]
async fn a_moment_inside_the_recording_cues_it_and_any_other_is_dropped() {
    let journal = journal();
    let root = journal.path();
    let place = recording("desk", "100000_300");
    let plain = format!("/app/transcripts/{DAY}?stream=desk#100000_300");
    for (moment, cue) in [
        ("~100000", Some("100000")),
        ("~100212", Some("100212")),
        ("~100500", Some("100500")),
    ] {
        assert_eq!(
            opens_to(root, &place, moment).await,
            format!("{plain}/transcript/{}", cue.unwrap()),
            "{moment}"
        );
    }
    // Outside the recording, not a time, not six digits: the recording
    // still opens, without a cue.
    for moment in [
        "~095959",
        "~100501",
        "~250000",
        "~1002",
        "~10021299",
        "~abcdef",
        "~",
    ] {
        assert_eq!(opens_to(root, &place, moment).await, plain, "{moment}");
    }
    // A moment means nothing on any other kind of place.
    assert_eq!(
        opens_to(root, &person_place("benvolio_montague"), "~100212").await,
        "/app/entities/#benvolio_montague"
    );
}

#[tokio::test]
async fn hidden_deleted_and_absent_places_give_one_page() {
    let journal = journal();
    let root = journal.path();
    let absent = refuses(root, &activity("work", "never_was")).await;
    assert_eq!(absent.0, StatusCode::NOT_FOUND);
    assert!(absent.1.contains(ABSENT));
    assert!(
        absent
            .1
            .contains(r#"<a href="/" id="link-action">open your journal</a>"#)
    );

    // An activity the owner hid, a retired plan, a facet that isn't there.
    assert_eq!(refuses(root, &activity("work", "walk_1")).await, absent);
    assert_eq!(refuses(root, &activity("work", "plan_1")).await, absent);
    assert_eq!(
        refuses(root, &activity("nowhere", "meeting_100000_300")).await,
        absent
    );
    // A person the owner hid, and one who was never there.
    assert_eq!(refuses(root, &person_place("tybalt_capulet")).await, absent);
    assert_eq!(refuses(root, &person_place("nobody")).await, absent);
    // A recording under another stream, on another day, or never made.
    assert_eq!(
        refuses(root, &recording("laptop", "100000_300")).await,
        absent
    );
    assert_eq!(
        refuses(root, &recording("desk", "120000_300")).await,
        absent
    );
    // A recording the owner deleted, whatever is still on disk.
    write_file(
        &root.join(format!("chronicle/{DAY}/desk/100000_300/tombstone.json")),
        "{}\n",
    );
    assert_eq!(
        refuses(root, &recording("desk", "100000_300")).await,
        absent
    );
    // The page names nothing about the place.
    for word in ["walk_1", "tybalt", "desk", "work", DAY] {
        assert!(!absent.1.contains(word), "{word}");
    }
}

#[tokio::test]
async fn a_renamed_or_merged_facet_is_followed_to_where_its_activities_are() {
    let journal = journal();
    let root = journal.path();
    const GONE: &str = "a0000000-0000-4000-8000-000000000009";
    const STALE: &str = "a0000000-0000-4000-8000-000000000007";
    write_json(
        &root.join("facets/retired.json"),
        &json!({"names": {
            // `job` was renamed to `work`.
            "job": {"state": "renamed", "successor": WORK},
            // `errands` was merged into a facet that was later merged into `work`.
            "errands": {"state": "merged", "successor": GONE},
            "chores": {"state": "merged", "id": GONE, "successor": WORK},
            "trashed": {"state": "deleted"},
            // A stale record of the same id that names itself sorts first and
            // is passed over for the one that leads on.
            "aardvark": {"state": "renamed", "id": STALE, "successor": STALE},
            "started": {"state": "merged", "successor": STALE},
            "zebra": {"state": "merged", "id": STALE, "successor": WORK},
            // A record that leads only to itself opens nothing, and ends.
            "loop": {"state": "merged", "id": "a0000000-0000-4000-8000-000000000008",
                     "successor": "a0000000-0000-4000-8000-000000000008"},
        }}),
    );
    let landed = format!("/app/activities/{DAY}?facet=work#work/meeting_100000_300");
    for facet in ["job", "errands", "chores", "started"] {
        assert_eq!(
            opens_to(root, &activity(facet, "meeting_100000_300"), "").await,
            landed,
            "{facet}"
        );
        // Following a facet shows no more than the facet itself would.
        is_absent(root, &activity(facet, "walk_1")).await;
    }
    is_absent(root, &activity("trashed", "meeting_100000_300")).await;
    is_absent(root, &activity("loop", "meeting_100000_300")).await;
}

#[tokio::test]
async fn a_merged_person_is_followed_to_who_they_became() {
    let journal = journal();
    let root = journal.path();
    write_json(
        &root.join("entities/retired.json"),
        &json!({"ids": {
            "ben": {"state": "merged", "dir": "ben", "successor": "benvolio"},
            "benvolio": {"state": "merged", "dir": "benvolio", "successor": "benvolio_montague"},
            "ty": {"state": "merged", "dir": "ty", "successor": "tybalt_capulet"},
            "erased": {"state": "deleted", "dir": "erased"},
        }}),
    );
    for id in ["ben", "benvolio"] {
        assert_eq!(
            opens_to(root, &person_place(id), "").await,
            "/app/entities/#benvolio_montague",
            "{id}"
        );
    }
    // Merged into someone the owner hid, or erased outright.
    is_absent(root, &person_place("ty")).await;
    is_absent(root, &person_place("erased")).await;
}

#[tokio::test]
async fn text_this_journal_did_not_seal_gets_one_page_and_changes_nothing() {
    let journal = journal();
    let root = journal.path();
    let other = tempdir().unwrap();
    write_root_key(other.path(), "1e");
    let foreign = seal(other.path(), &day(DAY)).unwrap();
    let own = seal(root, &day(DAY)).unwrap();
    let mut altered = own.clone().into_bytes();
    altered[10] = if altered[10] == b'A' { b'B' } else { b'A' };
    let altered = String::from_utf8(altered).unwrap();

    let (status, expected, location) = open(root, &foreign).await;
    assert_eq!((status, location), (StatusCode::NOT_FOUND, None));
    assert!(expected.contains(NOT_OPENED));
    assert!(expected.contains("it may be for another journal, or the link may have changed."));
    assert!(expected.contains(r#"<a href="/" id="link-action">open your journal</a>"#));
    // It never says whose link it is.
    assert!(!expected.contains(ABSENT));

    let long = "A".repeat(513);
    for link in [
        altered.as_str(),
        "AQ",
        "not-a-link",
        long.as_str(),
        &format!("{foreign}~100212"),
        &format!("{own}.{own}"),
        // The moment is what follows the last `~`: the rest is not base64url.
        &format!("{own}~x~100212"),
        &format!("~{own}"),
        "~100212",
        "%3Cscript%3E",
        "%22%3E%3Cscript%3E",
    ] {
        let (status, body, location) = open(root, link).await;
        assert_eq!((status, location), (StatusCode::NOT_FOUND, None), "{link}");
        assert_eq!(body, expected, "{link}");
    }

    // A journal that never sealed anything opens nothing, and opening made
    // it no key.
    let fresh = tempdir().unwrap();
    let (status, body, _) = open(fresh.path(), &own).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, expected);
    assert_eq!(fs::read_dir(fresh.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn a_key_that_cant_be_read_says_so_and_offers_the_same_link_again() {
    let journal = journal();
    let root = journal.path();
    let link = seal(root, &recording("desk", "100000_300")).unwrap();
    let other = tempdir().unwrap();
    fs::create_dir_all(other.path().join("link")).unwrap();
    fs::write(other.path().join("link/root.key"), "damaged").unwrap();

    let (status, body, location) = open(other.path(), &format!("{link}~100212")).await;
    assert_eq!(
        (status, location),
        (StatusCode::INTERNAL_SERVER_ERROR, None)
    );
    assert!(body.contains("your journal couldn't check this source."));
    assert!(body.contains(&format!(
        r#"<a href="/app/open/{link}~100212" id="link-action">try again</a>"#
    )));
    // Text that could never be a link is refused before the key is read,
    // so nothing from the request reaches the page.
    let (status, body, _) = open(other.path(), "%22%3E%3Cscript%3Ealert(1)%3C%2Fscript%3E").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains(NOT_OPENED));
    assert!(!body.contains("script"), "{body}");
    // A moment that wasn't one is not carried into the page.
    let (_, body, _) = open(other.path(), &format!("{link}~nope")).await;
    assert!(body.contains(&format!(r#"<a href="/app/open/{link}" id="link-action">"#)));
    assert_eq!(
        fs::read_to_string(other.path().join("link/root.key")).unwrap(),
        "damaged"
    );
}

#[tokio::test]
async fn opening_a_link_writes_nothing() {
    fn listing(dir: &Path, out: &mut Vec<(String, u64)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                listing(&entry.path(), out);
            } else {
                out.push((entry.path().display().to_string(), meta.len()));
            }
        }
    }
    let journal = journal();
    let root = journal.path();
    let links: Vec<String> = [
        day(DAY),
        recording("desk", "100000_300"),
        activity("work", "meeting_100000_300"),
        activity("work", "walk_1"),
        person_place("benvolio_montague"),
        person_place("nobody"),
    ]
    .iter()
    .map(|place| seal(root, place).unwrap())
    .collect();
    let mut before = Vec::new();
    listing(root, &mut before);
    before.sort();
    for link in &links {
        open(root, link).await;
        open(root, &format!("{link}~100212")).await;
    }
    open(root, "not-a-link").await;
    let mut after = Vec::new();
    listing(root, &mut after);
    after.sort();
    assert_eq!(before, after);
}

/// The route is one of the dashboard's own: it answers through the whole
/// shell router, with a redirect that stays on this journal.
#[tokio::test]
async fn the_shell_router_serves_it_with_a_relative_redirect() {
    let journal = journal();
    let root = journal.path();
    write_json(
        &root.join("config/journal.json"),
        &json!({
            "setup": {"completed_at": 1_700_000_000_000i64},
            "identity": {"name": "Test Owner", "timezone": "UTC"}
        }),
    );
    let link = seal(root, &recording("desk", "100000_300")).unwrap();
    let (status, _, location) = get(
        router(root.to_path_buf()),
        &format!("/app/open/{link}~100212"),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        location.as_deref(),
        Some(format!("/app/transcripts/{DAY}?stream=desk#100000_300/transcript/100212").as_str())
    );
    let (status, body, _) = get(router(root.to_path_buf()), "/app/open/not-a-link").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains(NOT_OPENED));
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The published vector, `tests/fixtures/journal-link-vector.json`, is what
/// the app builders test against. Every link in it does what it says on a
/// copy of the fixture journal, which holds the key that sealed them.
#[tokio::test]
async fn the_fixture_journal_opens_the_published_vector() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures");
    let vector: Value = serde_json::from_str(
        &fs::read_to_string(fixtures.join("journal-link-vector.json")).unwrap(),
    )
    .unwrap();
    let journal = tempdir().unwrap();
    copy_tree(&fixtures.join("journal"), journal.path());
    assert_eq!(
        fs::read_to_string(journal.path().join("link/root.key"))
            .unwrap()
            .trim(),
        vector["sealed_by"]["root_key_hex"].as_str().unwrap()
    );

    let links = vector["links"].as_array().unwrap();
    assert!(links.len() >= 13);
    for row in links {
        let id = row["id"].as_str().unwrap();
        let link = row["t"].as_str().unwrap();
        let (status, body, location) = open(journal.path(), link).await;
        assert_eq!(
            u64::from(status.as_u16()),
            row["status"].as_u64().unwrap(),
            "{id}"
        );
        assert_eq!(location.as_deref(), row["location"].as_str(), "{id}");
        if let Some(reason) = row["reason"].as_str() {
            assert!(body.contains(reason), "{id}: {body}");
        }
        // What an app checks before it asks the journal: `<sealed>` or
        // `<sealed>~HHMMSS`, base64url, at most 512 characters.
        let (sealed, moment) = link
            .split_once('~')
            .map_or((link, None), |(s, m)| (s, Some(m)));
        let in_grammar = !sealed.is_empty()
            && sealed.len() <= 512
            && sealed
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            && moment.is_none_or(|m| m.len() == 6 && m.bytes().all(|b| b.is_ascii_digit()));
        assert_eq!(in_grammar, row["app_grammar"].as_bool().unwrap(), "{id}");
    }
    // The good links were sealed by another implementation of the same
    // construction; this journal seals the same text.
    assert_eq!(
        seal(journal.path(), &day("20260305")).as_deref(),
        links[0]["t"].as_str()
    );
}

/// The key has leaked: someone can seal any text a link's grammar allows.
/// It still opens only what the journal itself lists.
#[tokio::test]
async fn a_forged_place_reaches_nothing_the_journal_doesnt_list() {
    let journal = journal();
    let root = journal.path();
    // A scratch folder that looks like a facet, as a facet merge leaves.
    write_json(
        &root.join("facets/.scratch/facet.json"),
        &json!({"id": HOME}),
    );
    write_file(
        &root.join(format!("facets/.scratch/activities/{DAY}.jsonl")),
        &format!("{}\n", json!({"id": "meeting_100000_300"})),
    );
    for place in [
        activity(".scratch", "meeting_100000_300"),
        activity("work.", "meeting_100000_300"),
        recording("desk", "100000_300.bak"),
        person_place("retired.json"),
        person_place(".trash"),
        day("chronicle"),
    ] {
        is_absent(root, &place).await;
    }
}

/// A link opens nothing before the journal is set up: the route is behind
/// the dashboard's session gate like every page it leads to.
#[tokio::test]
async fn a_journal_that_isnt_set_up_sends_a_link_to_setup() {
    let journal = journal();
    let root = journal.path();
    let link = seal(root, &recording("desk", "100000_300")).unwrap();
    let (status, _, location) = get(router(root.to_path_buf()), &format!("/app/open/{link}")).await;
    assert!(status.is_redirection(), "{status}");
    assert!(
        location
            .as_deref()
            .is_some_and(|to| to.starts_with("/init")),
        "{location:?}"
    );
}
