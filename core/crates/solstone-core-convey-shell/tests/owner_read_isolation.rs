// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Verification of owner read isolation (spawn_blocking) across all qualifying GET handlers.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use solstone_core_convey_http::owner_read::{OwnerReadRole, test_hooks};
use solstone_core_convey_shell::router;
use tokio::sync::Mutex;
use tower::ServiceExt;

static TEST_LOCK: Mutex<()> = Mutex::const_new(());

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::TempDir::new_in("/var/tmp").expect("fixture tempdir");
        for path in [
            "facets",
            "entities",
            "chronicle",
            "talents",
            "config",
            "awareness",
            "speakers",
        ] {
            fs::create_dir_all(root.path().join(path)).expect("journal directory");
        }
        fs::write(
            root.path().join("config/journal.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "setup": {"completed_at": 1},
                "identity": {"name": "Test Owner", "timezone": "UTC"},
                "providers": {"active": {"provider": "test"}}
            }))
            .expect("config"),
        )
        .expect("write journal.json");
        Self { root }
    }

    fn path(&self) -> PathBuf {
        self.root.path().to_path_buf()
    }
}

async fn get_response(
    path: &str,
    journal_root: &Path,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let app = router(journal_root.to_path_buf());
    let response = app
        .oneshot(Request::get(path).body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    (status, headers, body)
}

async fn get(path: &str, journal_root: &Path) -> (StatusCode, Vec<u8>) {
    let (status, _, body) = get_response(path, journal_root).await;
    (status, body)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteClassification {
    Moved(OwnerReadRole),
    AlreadyIsolated,
    NoQualifyingWork,
}

#[test]
fn owner_read_role_classification_table_exhaustiveness() {
    let table: &[(&str, RouteClassification)] = &[
        // Speakers (27 live GET paths in convey-shell lib.rs:755-854)
        ("/app/speakers/", RouteClassification::NoQualifyingWork),
        ("/app/speakers/{day}", RouteClassification::NoQualifyingWork),
        (
            "/app/speakers/workspace",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/speakers/static/who_is_this.js",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/speakers/api/state",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/speakers/api/index",
            RouteClassification::Moved(OwnerReadRole::SpeakersIndex),
        ),
        (
            "/app/speakers/api/grid",
            RouteClassification::Moved(OwnerReadRole::SpeakersGrid),
        ),
        (
            "/app/speakers/api/quality",
            RouteClassification::Moved(OwnerReadRole::SpeakersQuality),
        ),
        (
            "/app/speakers/api/owner/status",
            RouteClassification::Moved(OwnerReadRole::SpeakersOwnerStatus),
        ),
        (
            "/app/speakers/api/discovery/cache",
            RouteClassification::Moved(OwnerReadRole::SpeakersDiscoveryCache),
        ),
        (
            "/app/speakers/api/discovery/cluster/{cluster_id}/presence",
            RouteClassification::Moved(OwnerReadRole::SpeakersDiscoveryPresence),
        ),
        (
            "/app/speakers/api/discovery/resolve-statement",
            RouteClassification::Moved(OwnerReadRole::SpeakersDiscoveryResolveStatement),
        ),
        (
            "/app/speakers/api/people/search",
            RouteClassification::Moved(OwnerReadRole::SpeakersPeopleSearch),
        ),
        (
            "/app/speakers/api/serve_audio/{day}/{*rel_path}",
            RouteClassification::Moved(OwnerReadRole::SpeakersServeAudio),
        ),
        (
            "/app/speakers/api/speakers/known",
            RouteClassification::Moved(OwnerReadRole::SpeakersKnown),
        ),
        (
            "/app/speakers/api/speakers/{day}/{stream}/{segment_key}",
            RouteClassification::Moved(OwnerReadRole::SpeakersSegmentSpeakers),
        ),
        (
            "/app/speakers/api/review/{day}/{stream}/{segment_key}/{source}",
            RouteClassification::Moved(OwnerReadRole::SpeakersReview),
        ),
        (
            "/app/speakers/api/stats/{month}",
            RouteClassification::Moved(OwnerReadRole::SpeakersMonthStats),
        ),
        (
            "/app/speakers/api/segments/{day}",
            RouteClassification::Moved(OwnerReadRole::SpeakersSegments),
        ),
        (
            "/app/speakers/api/segments-cli/{day}",
            RouteClassification::Moved(OwnerReadRole::SpeakersSegmentsCli),
        ),
        (
            "/app/speakers/api/review-cli/{day}/{stream}/{segment_key}/{source}",
            RouteClassification::Moved(OwnerReadRole::SpeakersReviewCli),
        ),
        (
            "/app/speakers/api/status",
            RouteClassification::Moved(OwnerReadRole::SpeakersStatus),
        ),
        (
            "/app/speakers/api/suggest",
            RouteClassification::Moved(OwnerReadRole::SpeakersSuggest),
        ),
        (
            "/app/speakers/api/name-variants/keep-separate",
            RouteClassification::Moved(OwnerReadRole::SpeakersKeepSeparate),
        ),
        (
            "/app/speakers/api/discovery/dismissals",
            RouteClassification::Moved(OwnerReadRole::SpeakersDismissals),
        ),
        (
            "/app/speakers/api/discovery/identify/operations",
            RouteClassification::Moved(OwnerReadRole::SpeakersIdentifyOperations),
        ),
        (
            "/app/speakers/api/discovery/identify/operations/{operation_id}",
            RouteClassification::Moved(OwnerReadRole::SpeakersIdentifyOperation),
        ),
        // Transcripts (11 live GET paths in transcripts-web lib.rs:85-110)
        (
            "/app/transcripts/",
            RouteClassification::Moved(OwnerReadRole::TranscriptsRoot),
        ),
        (
            "/app/transcripts/workspace",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/transcripts/{day}",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/transcripts/api/index",
            RouteClassification::Moved(OwnerReadRole::TranscriptsIndex),
        ),
        (
            "/app/transcripts/api/stats/{month}",
            RouteClassification::Moved(OwnerReadRole::TranscriptsMonthStats),
        ),
        (
            "/app/transcripts/api/ranges/{day}",
            RouteClassification::Moved(OwnerReadRole::TranscriptsRanges),
        ),
        (
            "/app/transcripts/api/segments/{day}",
            RouteClassification::Moved(OwnerReadRole::TranscriptsSegments),
        ),
        (
            "/app/transcripts/api/day/{day}",
            RouteClassification::Moved(OwnerReadRole::TranscriptsDay),
        ),
        (
            "/app/transcripts/api/read/{day}",
            RouteClassification::Moved(OwnerReadRole::TranscriptsRead),
        ),
        (
            "/app/transcripts/api/segment/{day}/{stream}/{segment_key}",
            RouteClassification::Moved(OwnerReadRole::TranscriptsSegment),
        ),
        (
            "/app/transcripts/api/serve_file/{day}/{*rel_path}",
            RouteClassification::Moved(OwnerReadRole::TranscriptsServeFile),
        ),
        // Stats (8 live GET paths in stats-web lib.rs:24-43)
        ("/app/stats/", RouteClassification::NoQualifyingWork),
        (
            "/app/stats/workspace",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/stats/static/{*name}",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/stats/background",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/stats/api/stats",
            RouteClassification::Moved(OwnerReadRole::StatsData),
        ),
        (
            "/app/stats/api/usage",
            RouteClassification::Moved(OwnerReadRole::StatsUsage),
        ),
        (
            "/app/stats/api/index",
            RouteClassification::Moved(OwnerReadRole::StatsIndex),
        ),
        (
            "/app/stats/api/stats/{month}",
            RouteClassification::Moved(OwnerReadRole::StatsMonthStats),
        ),
        // Health (10 live GET paths in health-web lib.rs:31-42 + journal_data api_router)
        ("/app/health/", RouteClassification::NoQualifyingWork),
        (
            "/app/health/workspace",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/health/static/{*name}",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/health/api/state",
            RouteClassification::Moved(OwnerReadRole::HealthState),
        ),
        (
            "/app/health/api/log",
            RouteClassification::Moved(OwnerReadRole::HealthLog),
        ),
        (
            "/app/health/api/info",
            RouteClassification::Moved(OwnerReadRole::HealthInfo),
        ),
        (
            "/api/health/summary",
            RouteClassification::Moved(OwnerReadRole::HealthSummary),
        ),
        ("/api/health/full", RouteClassification::AlreadyIsolated),
        (
            "/api/health/range",
            RouteClassification::Moved(OwnerReadRole::HealthRange),
        ),
        (
            "/api/health/pipeline",
            RouteClassification::Moved(OwnerReadRole::HealthPipeline),
        ),
        // Home (8 live GET paths in home-web lib.rs:26-42)
        ("/app/home/", RouteClassification::NoQualifyingWork),
        ("/app/home", RouteClassification::NoQualifyingWork),
        ("/app/home/workspace", RouteClassification::NoQualifyingWork),
        (
            "/app/home/static/home.js",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/home/static/removals.js",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/home/api/removals",
            RouteClassification::AlreadyIsolated,
        ),
        (
            "/app/home/api/pulse",
            RouteClassification::Moved(OwnerReadRole::HomePulse),
        ),
        (
            "/app/home/api/briefing",
            RouteClassification::Moved(OwnerReadRole::HomeBriefing),
        ),
        // Backup (6 live GET paths in backup-web lib.rs:160-170)
        ("/app/backup/", RouteClassification::NoQualifyingWork),
        (
            "/app/backup/workspace",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/backup/background",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/backup/static/{name}",
            RouteClassification::NoQualifyingWork,
        ),
        (
            "/app/backup/status",
            RouteClassification::Moved(OwnerReadRole::BackupStatus),
        ),
        (
            "/app/backup/offload/status",
            RouteClassification::Moved(OwnerReadRole::BackupOffloadStatus),
        ),
    ];

    assert_eq!(table.len(), 70, "Must classify all 70 live GET routes");

    let mut distinct_paths = BTreeSet::new();
    for (path, _) in table {
        assert!(
            distinct_paths.insert(*path),
            "Duplicate path in live GET table: {path}"
        );
    }

    let moved_roles: BTreeSet<OwnerReadRole> = table
        .iter()
        .filter_map(|(_, classification)| match classification {
            RouteClassification::Moved(role) => Some(*role),
            _ => None,
        })
        .collect();

    let all_roles: BTreeSet<OwnerReadRole> = OwnerReadRole::ALL.iter().copied().collect();
    assert_eq!(
        moved_roles, all_roles,
        "Moved roles must equal OwnerReadRole::ALL exactly"
    );
    assert_eq!(moved_roles.len(), 45);

    let ten_measured = [
        OwnerReadRole::SpeakersKnown,
        OwnerReadRole::SpeakersGrid,
        OwnerReadRole::SpeakersOwnerStatus,
        OwnerReadRole::TranscriptsIndex,
        OwnerReadRole::TranscriptsMonthStats,
        OwnerReadRole::StatsIndex,
        OwnerReadRole::HealthSummary,
        OwnerReadRole::HomePulse,
        OwnerReadRole::HomeBriefing,
        OwnerReadRole::BackupOffloadStatus,
    ];
    for measured in ten_measured {
        assert!(
            all_roles.contains(&measured),
            "OwnerReadRole::ALL must contain measured role {measured:?}"
        );
    }

    let already_isolated: BTreeSet<&str> = table
        .iter()
        .filter_map(|(path, classification)| match classification {
            RouteClassification::AlreadyIsolated => Some(*path),
            _ => None,
        })
        .collect();
    assert_eq!(
        already_isolated,
        BTreeSet::from(["/app/home/api/removals", "/api/health/full"]),
        "AlreadyIsolated must be exactly home removals and health full"
    );

    let no_qualifying: Vec<&str> = table
        .iter()
        .filter_map(|(path, classification)| match classification {
            RouteClassification::NoQualifyingWork => Some(*path),
            _ => None,
        })
        .collect();
    assert_eq!(
        no_qualifying.len(),
        23,
        "NoQualifyingWork must have exactly 23 routes"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_read_homogeneous_isolation() {
    let _lock = TEST_LOCK.lock().await;
    let fixture = Fixture::new();
    let journal_root = fixture.path();

    for &role in OwnerReadRole::ALL {
        test_hooks::reset();
        test_hooks::hold(role);
        let uri = role.probe_uri();

        let root1 = journal_root.clone();
        let handle1 = tokio::spawn(async move { get(uri, &root1).await });
        let root2 = journal_root.clone();
        let handle2 = tokio::spawn(async move { get(uri, &root2).await });

        tokio::time::timeout(
            Duration::from_secs(5),
            tokio::task::spawn_blocking(move || {
                test_hooks::wait_entered(role, 2);
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("wait_entered timed out for role {role:?} at URI {uri}"))
        .expect("wait_entered join handle");

        let shell_root = journal_root.clone();
        let shell_res = tokio::time::timeout(Duration::from_secs(5), async move {
            get("/api/shell", &shell_root).await
        })
        .await
        .expect("GET /api/shell timed out while blocking tasks were held");

        assert_eq!(
            shell_res.0,
            StatusCode::OK,
            "GET /api/shell must succeed with 200 while {role:?} is held"
        );

        test_hooks::release(role);

        let _ = tokio::time::timeout(Duration::from_secs(5), handle1)
            .await
            .expect("slow request 1 timed out")
            .expect("join handle 1");
        let _ = tokio::time::timeout(Duration::from_secs(5), handle2)
            .await
            .expect("slow request 2 timed out")
            .expect("join handle 2");

        test_hooks::reset();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_read_mixed_isolation() {
    let _lock = TEST_LOCK.lock().await;
    test_hooks::reset();
    let fixture = Fixture::new();
    let journal_root = fixture.path();

    for &role in OwnerReadRole::ALL {
        test_hooks::hold(role);
    }

    let mut handles = Vec::new();
    for &role in OwnerReadRole::ALL {
        let uri = role.probe_uri();
        let root = journal_root.clone();
        handles.push(tokio::spawn(async move { get(uri, &root).await }));
    }

    for &role in OwnerReadRole::ALL {
        let uri = role.probe_uri();
        tokio::time::timeout(
            Duration::from_secs(5),
            tokio::task::spawn_blocking(move || {
                test_hooks::wait_entered(role, 1);
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("wait_entered timed out for role {role:?} at URI {uri}"))
        .expect("wait_entered join handle");
    }

    let shell_root = journal_root.clone();
    let shell_res = tokio::time::timeout(Duration::from_secs(5), async move {
        get("/api/shell", &shell_root).await
    })
    .await
    .expect("GET /api/shell timed out while all 45 roles were held");

    assert_eq!(
        shell_res.0,
        StatusCode::OK,
        "GET /api/shell must succeed with 200 while all 45 roles are held"
    );

    test_hooks::release_all();

    for handle in handles {
        let _ = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("task timed out after release_all")
            .expect("join handle");
    }

    test_hooks::reset();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_read_panic_recovery_returns_500_zero_byte_body() {
    let _lock = TEST_LOCK.lock().await;
    let fixture = Fixture::new();
    let journal_root = fixture.path();

    for &role in OwnerReadRole::ALL {
        test_hooks::reset();
        test_hooks::panic(role);

        let (status, body) = get(role.probe_uri(), &journal_root).await;
        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Role {role:?} must return 500 on worker panic"
        );
        assert!(
            body.is_empty(),
            "Role {role:?} on panic must return 0-byte body, got {} bytes: {:?}",
            body.len(),
            String::from_utf8_lossy(&body)
        );
        test_hooks::reset();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_read_admission_unestablished_and_corrupt_config() {
    let _lock = TEST_LOCK.lock().await;

    // 1. Unestablished journal (no setup completed)
    let unestablished_dir = tempfile::TempDir::new_in("/var/tmp").expect("unestablished tempdir");
    let unestablished_root = unestablished_dir.path();
    for &role in OwnerReadRole::ALL {
        test_hooks::reset();
        let uri = role.probe_uri();
        let (status, headers, _body) = get_response(uri, unestablished_root).await;
        assert_eq!(
            status,
            StatusCode::FOUND,
            "Unestablished journal for role {role:?} at {uri} must return 302 Found"
        );
        assert_eq!(
            headers.get(header::LOCATION).and_then(|h| h.to_str().ok()),
            Some("/init"),
            "Unestablished journal for role {role:?} must redirect to /init"
        );
        assert_eq!(
            test_hooks::entered(role),
            0,
            "Role {role:?} must not enter blocking hook on unestablished journal"
        );
    }

    // 2. Corrupt journal.json
    let corrupt_dir = tempfile::TempDir::new_in("/var/tmp").expect("corrupt tempdir");
    let corrupt_root = corrupt_dir.path();
    fs::create_dir_all(corrupt_root.join("config")).expect("config dir");
    fs::write(corrupt_root.join("config/journal.json"), b"{not json")
        .expect("corrupt config write");

    for &role in OwnerReadRole::ALL {
        test_hooks::reset();
        let uri = role.probe_uri();
        let (status, _headers, body) = get_response(uri, corrupt_root).await;
        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Corrupt config for role {role:?} at {uri} must return 500"
        );
        if uri.trim_matches('/').split('/').any(|s| s == "api") {
            let val: Value = serde_json::from_slice(&body).unwrap_or_else(|_| {
                panic!("Role {role:?} at {uri} should return JSON error envelope")
            });
            assert_eq!(
                val["reason_code"], "corrupt_config",
                "Role {role:?} at {uri} should return reason_code corrupt_config"
            );
        }
        assert_eq!(
            test_hooks::entered(role),
            0,
            "Role {role:?} must not enter blocking hook on corrupt journal"
        );
    }

    test_hooks::reset();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_read_cheap_admission_checks_stay_async() {
    let _lock = TEST_LOCK.lock().await;
    test_hooks::reset();
    let fixture = Fixture::new();
    let journal_root = fixture.path();

    // 1. SpeakersPeopleSearch with empty query -> returns 200 without entering hook
    let (status, body) = get("/app/speakers/api/people/search?q=", &journal_root).await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(v["query"], "");
    assert_eq!(v["people"], serde_json::json!([]));
    assert_eq!(
        test_hooks::entered(OwnerReadRole::SpeakersPeopleSearch),
        0,
        "Empty people search should stay async"
    );

    // 2. SpeakersServeAudio with invalid day format -> returns 404 without entering hook
    let (status, _) = get(
        "/app/speakers/api/serve_audio/bad-day/audio.wav",
        &journal_root,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        test_hooks::entered(OwnerReadRole::SpeakersServeAudio),
        0,
        "Invalid day audio serve should stay async"
    );

    // 3. TranscriptsServeFile with invalid day format -> returns 404 without entering hook
    let (status, _) = get(
        "/app/transcripts/api/serve_file/bad-day/raw.jsonl",
        &journal_root,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        test_hooks::entered(OwnerReadRole::TranscriptsServeFile),
        0,
        "Invalid day transcripts serve should stay async"
    );

    // 4. Speakers Month Stats with invalid month format -> returns 400 without entering hook
    let (status, _) = get("/app/speakers/api/stats/invalid_month", &journal_root).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        test_hooks::entered(OwnerReadRole::SpeakersMonthStats),
        0,
        "Invalid month speakers stats should stay async"
    );

    // 5. Transcripts Month Stats with invalid month format -> returns 400 without entering hook
    let (status, _) = get("/app/transcripts/api/stats/invalid_month", &journal_root).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        test_hooks::entered(OwnerReadRole::TranscriptsMonthStats),
        0,
        "Invalid month transcripts stats should stay async"
    );

    // 6. Stats Calendar with invalid month format -> returns 400 without entering hook
    let (status, _) = get("/app/stats/api/stats/invalid_month", &journal_root).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        test_hooks::entered(OwnerReadRole::StatsMonthStats),
        0,
        "Invalid month stats calendar should stay async"
    );

    // 7. Health Logs with missing/invalid path -> returns 400 without entering hook
    let (status, _) = get("/app/health/api/log?path=invalid", &journal_root).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        test_hooks::entered(OwnerReadRole::HealthLog),
        0,
        "Invalid health log path should stay async"
    );

    // 8. Health Pipeline without day -> returns 400 without entering hook
    let (status, _) = get("/api/health/pipeline", &journal_root).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        test_hooks::entered(OwnerReadRole::HealthPipeline),
        0,
        "Missing health pipeline day should stay async"
    );

    test_hooks::reset();
}
