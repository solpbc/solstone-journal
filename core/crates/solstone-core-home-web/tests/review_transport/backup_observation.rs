// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use solstone_core_backup::{
    Destination, HostedBinding, generate_and_store_keys, get_backup_config, save_hosted_binding,
    set_destination, set_enabled, set_mode,
};
use solstone_core_backup_runtime::{
    BackupServices, Clock, HttpTransport, JournalMaintenance, JournalMaintenanceError, ToolRunner,
    prepare,
};
use solstone_core_backup_runtime::{
    hosted_runtime::{HttpError, HttpRequest, HttpResponse},
    runner::{ToolOutput, ToolRequest},
};
use solstone_core_backup_web::BackupWebDeps;
use std::{
    cell::RefCell,
    collections::VecDeque,
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
};
use tower::ServiceExt;

struct Fixed;
impl Clock for Fixed {
    fn now_unix(&self) -> i64 {
        1791910000
    }
    fn iso_week(&self) -> u8 {
        42
    }
}
struct Maintenance;
impl JournalMaintenance for Maintenance {
    fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
        Ok(())
    }
}
struct Broker;
impl HttpTransport for Broker {
    fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse{status:200,headers:vec![],body:serde_json::to_vec(&json!({"access_key_id":"synthetic-access", "secret_access_key":"synthetic-secret", "session_token":"synthetic-session", "endpoint":"https://synthetic.invalid", "expires_at":"later"})).unwrap()})
    }
}
struct Script {
    outputs: RefCell<VecDeque<ToolOutput>>,
    after_unlock: RefCell<Option<Box<dyn FnOnce()>>>,
}
impl ToolRunner for Script {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        if request.argv.iter().any(|arg| arg == "unlock")
            && let Some(change) = self.after_unlock.borrow_mut().take()
        {
            change();
        }
        Ok(self
            .outputs
            .borrow_mut()
            .pop_front()
            .expect("expected synthetic tool call"))
    }
}
fn destination(repository: &str) -> Destination {
    Destination {
        repository: repository.into(),
        backend: "s3".into(),
        credentials:
            json!({"access_key_id":"synthetic-access", "secret_access_key":"synthetic-secret"})
                .as_object()
                .unwrap()
                .clone(),
    }
}
fn setup() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    set_destination(root.path(), &destination("s3:synthetic/current")).unwrap();
    generate_and_store_keys(root.path()).unwrap();
    set_enabled(root.path(), true).unwrap();
    root
}
fn execute(
    capability: solstone_core_backup_runtime::AdmittedCapability,
    code: i32,
    change: Option<Box<dyn FnOnce()>>,
) {
    let runner = Script {
        outputs: RefCell::new(VecDeque::from([
            ToolOutput {
                returncode: 0,
                stdout: vec![],
                stderr: vec![],
            },
            ToolOutput {
                returncode: code,
                stdout: br#"{"message_type":"summary","snapshot_id":"synthetic-copy"}"#.to_vec(),
                stderr: vec![],
            },
        ])),
        after_unlock: RefCell::new(change),
    };
    let result = capability.execute(&BackupServices {
        runner: &runner,
        http: &Broker,
        clock: &Fixed,
        restic: Some((Path::new("/synthetic/restic")).into()),
        rclone_path: Some(Path::new("/synthetic/rclone")),
        version: "test",
        journal_maintenance: &Maintenance,
    });
    assert_eq!(
        result.status,
        if code == 0 || code == 3 {
            "ok"
        } else {
            "error"
        }
    );
    assert!(runner.outputs.borrow().is_empty());
}
async fn read(deps: &BackupWebDeps) -> Value {
    let before = std::fs::read(deps.journal_root.join("config/journal.json")).unwrap();
    let router = solstone_core_home_web::routes_with_backup_deps(
        deps.clone(),
        solstone_core_home_web::Clock::fixed(
            chrono::DateTime::from_timestamp(1791910000, 0).unwrap(),
        ),
        |_| false,
    );
    let response = router
        .oneshot(
            Request::builder()
                .uri("/app/home/api/pulse")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(
        std::fs::read(deps.journal_root.join("config/journal.json")).unwrap(),
        before,
        "Home read must not write config"
    );
    let serialized = payload.to_string();
    for secret in [
        "synthetic-secret",
        "synthetic-access",
        "synthetic-session",
        "synthetic-broker-token",
        "setup_fingerprint",
    ] {
        assert!(!serialized.contains(secret), "Home exposed {secret}");
    }
    payload
}

#[tokio::test]
async fn admitted_copy_to_home_http_to_production_javascript() {
    let root = setup();
    let deps = BackupWebDeps::new(root.path().into());
    let mut cases = serde_json::Map::new();
    execute(prepare(root.path(), &Fixed).unwrap(), 12, None);
    let bound = get_backup_config(root.path()).unwrap()["last_backup"]["setup_fingerprint"].clone();
    assert!(bound.as_str().is_some_and(|s| s.len() == 64));
    let current = read(&deps).await;
    assert_eq!(current["backup"], json!({"available":true,"issue":true}));
    cases.insert("initial_current".into(), current);
    let a = prepare(root.path(), &Fixed).unwrap();
    set_destination(root.path(), &destination("s3:synthetic/replacement")).unwrap();
    execute(a, 12, None);
    assert_eq!(
        get_backup_config(root.path()).unwrap()["last_backup"]["setup_fingerprint"],
        bound
    );
    let superseded = read(&deps).await;
    assert_eq!(superseded["backup"]["issue"], false);
    cases.insert("superseded".into(), superseded);
    execute(prepare(root.path(), &Fixed).unwrap(), 12, None);
    let current = read(&deps).await;
    assert_eq!(current["backup"]["issue"], true);
    cases.insert("current".into(), current);
    let mut repaired = destination("s3:synthetic/replacement");
    repaired
        .credentials
        .insert("secret_access_key".into(), json!("replacement-secret"));
    set_destination(root.path(), &repaired).unwrap();
    let credentials_changed = read(&deps).await;
    assert_eq!(credentials_changed["backup"]["issue"], false);
    cases.insert("credentials_changed".into(), credentials_changed);
    let admitted = prepare(root.path(), &Fixed).unwrap();
    let change_root = root.path().to_path_buf();
    execute(
        admitted,
        12,
        Some(Box::new(move || {
            set_destination(
                &change_root,
                &destination("s3:synthetic/in-flight-replacement"),
            )
            .unwrap()
        })),
    );
    assert_eq!(
        read(&deps).await["backup"]["issue"],
        false,
        "in-flight replacement cannot stamp old attempt as current"
    );
    execute(prepare(root.path(), &Fixed).unwrap(), 12, None);
    assert_eq!(read(&deps).await["backup"]["issue"], true);
    execute(prepare(root.path(), &Fixed).unwrap(), 0, None);
    let success = read(&deps).await;
    assert_eq!(success["backup"]["issue"], false);
    cases.insert("success".into(), success);
    for code in [1, 3, 11, 124] {
        execute(prepare(root.path(), &Fixed).unwrap(), code, None);
        assert_eq!(
            read(&deps).await["backup"]["issue"],
            false,
            "routine/partial reason {code}"
        );
    }
    execute(prepare(root.path(), &Fixed).unwrap(), 12, None);
    set_enabled(root.path(), false).unwrap();
    let off = read(&deps).await;
    assert_eq!(off["backup"]["issue"], false);
    cases.insert("off".into(), off);
    set_enabled(root.path(), true).unwrap();
    // Legacy results have no association. Malformed association is unknown.
    let path = root.path().join("config/journal.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["backup"]["last_backup"]
        .as_object_mut()
        .unwrap()
        .remove("setup_fingerprint");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let legacy = read(&deps).await;
    assert_eq!(legacy["backup"]["issue"], false);
    cases.insert("legacy".into(), legacy);
    config["backup"]["last_backup"]["setup_fingerprint"] = json!("broken");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let unknown = read(&deps).await;
    assert_eq!(unknown["backup"]["available"], false);
    cases.insert("unknown".into(), unknown);
    execute(prepare(root.path(), &Fixed).unwrap(), 12, None);
    let saved_config = std::fs::read(&path).unwrap();
    for malformed in [
        json!({"enabled":null}),
        json!(null),
        json!({"enabled":"false"}),
        json!({"enabled":true,"last_backup":null}),
    ] {
        config["backup"] = malformed;
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        assert_eq!(
            read(&deps).await["backup"]["available"],
            false,
            "malformed config cannot clear the warning"
        );
    }
    std::fs::write(&path, &saved_config).unwrap();
    let mut invalid_credentials: Value = serde_json::from_slice(&saved_config).unwrap();
    invalid_credentials["backup"]["destination"]["credentials"] = Value::Null;
    std::fs::write(&path, serde_json::to_vec(&invalid_credentials).unwrap()).unwrap();
    let malformed_config = read(&deps).await;
    assert_eq!(malformed_config["backup"]["available"], false);
    cases.insert("malformed_config".into(), malformed_config);
    std::fs::write(&path, saved_config).unwrap();
    // Operated backup uses the captured binding, not freshly minted credentials.
    let binding = HostedBinding {
        broker_endpoint: "https://broker.synthetic.invalid".into(),
        account_id: "account".into(),
        instance_id: "instance".into(),
        bucket: "bucket".into(),
        prefix: "prefix".into(),
        broker_token: "synthetic-broker-token".into(),
    };
    set_mode(root.path(), "operated").unwrap();
    save_hosted_binding(root.path(), &binding).unwrap();
    execute(prepare(root.path(), &Fixed).unwrap(), 12, None);
    let operated = read(&deps).await;
    assert_eq!(operated["backup"]["issue"], true);
    cases.insert("operated".into(), operated);
    let mut replacement = binding.clone();
    replacement.broker_token = "replacement-token".into();
    save_hosted_binding(root.path(), &replacement).unwrap();
    assert_eq!(read(&deps).await["backup"]["issue"], false);
    replacement = binding.clone();
    replacement.broker_endpoint = "https://replacement.synthetic.invalid".into();
    save_hosted_binding(root.path(), &replacement).unwrap();
    assert_eq!(read(&deps).await["backup"]["issue"], false);
    save_hosted_binding(root.path(), &binding).unwrap();
    let binding_file = root.path().join("backup/hosted/binding.json");
    let saved = std::fs::read(&binding_file).unwrap();
    std::fs::write(&binding_file, b"{broken").unwrap();
    let binding_unknown = read(&deps).await;
    assert_eq!(binding_unknown["backup"]["available"], false);
    cases.insert("binding_unknown".into(), binding_unknown);
    std::fs::write(&binding_file, saved).unwrap();
    let restored = read(&deps).await;
    assert_eq!(restored["backup"]["issue"], true);
    cases.insert("binding_restored".into(), restored);
    let encoded = serde_json::to_vec(&cases).unwrap();
    if let Some(dir) = std::env::var_os("BACKUP_HOME_EVIDENCE_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(Path::new(&dir).join("home-http-fixtures.json"), &encoded).unwrap();
    }
    let manifest = env!("CARGO_MANIFEST_DIR");
    let mut node = Command::new("node")
        .arg(Path::new(manifest).join("tests/backup_observation.js"))
        .arg(manifest)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    node.stdin.take().unwrap().write_all(&encoded).unwrap();
    let result = node.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&result.stdout)
            .contains("PASS: admitted current/superseded HTTP lineage"),
        "consumer must finish its async assertions"
    );
    println!("{}", String::from_utf8_lossy(&result.stdout));
}
