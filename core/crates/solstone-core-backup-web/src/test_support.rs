use serde_json::{Map, Value, json};
use solstone_core_backup::{HostedBinding, save_hosted_binding, set_mode};
use solstone_core_backup_runtime::readiness::{
    RESTIC_SCHEMA_VERSION, RESTIC_TOOL, RESTIC_VERSION, binary_path, file_sha256, platform_info,
    sentinel_path,
};
use solstone_core_offload::{OffloadFile, append_offload_event, ledger_path_for_day};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub const RECOVERY_KEY: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const PORTAL_BASE: &str = "https://services.solstone.app";
pub const DEVICE_TOTAL_BYTES: u64 = 1_000_000_000_000;
pub const DEVICE_FREE_BYTES: u64 = 250_000_000_000;
pub fn corpus() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/convey_backup_corpus.json"
    )))
    .expect("backup corpus")
}
pub fn root(phase: &str) -> TempDir {
    let root = TempDir::new().expect("journal");
    if phase == "unestablished" {
        return root;
    }
    let config = root.path().join("config");
    fs::create_dir_all(&config).expect("config");
    fs::write(
        config.join("journal.json"),
        python_build_journal_bytes(phase),
    )
    .expect("config");
    root
}
pub fn python_build_journal_bytes(phase: &str) -> Vec<u8> {
    if phase == "corrupt" {
        return b"{\"setup\": {\"completed_at\": 17672256".to_vec();
    }
    let mut document = json!({"setup":{"completed_at":1767225600}});
    if phase != "fresh" {
        document["backup"] = backup(phase);
    }
    format!(
        "{}\n",
        serde_json::to_string_pretty(&sort_json_keys(document)).expect("json")
    )
    .into_bytes()
}

fn sort_json_keys(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(sort_json_keys).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sort_json_keys(value)))
                    .collect::<Map<_, _>>(),
            )
        }
        value => value,
    }
}
fn backup(phase: &str) -> Value {
    let mut value = json!({"enabled":true,"mode":"byo","destination":{"repository":"s3:s3.example.invalid/journal-corpus","backend":"s3","credentials":{"access_key_id":"CORPUSKEYID","secret_access_key":"corpus-secret"}},"daily_key":"corpus-daily-key","recovery_key":RECOVERY_KEY,"confirmed_recovery_key":true,"retention":{"hourly":24,"daily":7,"weekly":4,"monthly":12},"offload":{"enabled":false,"budget_bytes":null,"floor_bytes":null},"schedule":{"every":"daily","enabled":true},"last_prune":{"time":1769996400,"status":"ok","error_reason":null}});
    match phase {
        "enabled_never_run" => {
            value["last_backup"] =
                json!({"time":null,"snapshot_id":null,"status":null,"error_reason":null});
            value["last_verification"] = json!({"time":null,"status":null,"reason":null,"checked_subset":null,"last_ok_time":null});
            value["last_prune"] = json!({"time":null,"status":null,"error_reason":null});
        }
        "broken" => {
            value["last_backup"] = json!({"time":1770003600,"snapshot_id":null,"status":"error","error_reason":"locked"});
            value["last_verification"] = json!({"time":1770003000,"status":"error","reason":"read_data_mismatch","checked_subset":"5%","last_ok_time":1769990000});
        }
        "healthy" => {
            value["last_backup"] = json!({"time":1770000000,"snapshot_id":"9f2c1ab4","status":"ok","error_reason":null});
            value["last_verification"] = json!({"time":1769990000,"status":"ok","reason":null,"checked_subset":"5%","last_ok_time":1769990000});
        }
        _ => panic!("known phase"),
    };
    value
}

pub fn hosted_binding() -> HostedBinding {
    HostedBinding {
        broker_endpoint: PORTAL_BASE.into(),
        account_id: "account".into(),
        instance_id: "instance".into(),
        bucket: "bucket".into(),
        prefix: "owner/prefix".into(),
        broker_token: "broker-token-secret".into(),
    }
}

pub fn hosted_binding_wrong_origin() -> HostedBinding {
    HostedBinding {
        broker_endpoint: "https://broker.example".into(),
        ..hosted_binding()
    }
}

pub fn hosted_bound_root() -> TempDir {
    let root = self::root("healthy");
    set_mode(root.path(), "operated").expect("mode");
    save_hosted_binding(root.path(), &hosted_binding()).expect("binding");
    root
}

pub fn offload_inventory_root() -> TempDir {
    let root = self::root("healthy");
    let first = root.path().join("chronicle/20260101/010000_001");
    let second = root.path().join("chronicle/20260102/020000_001");
    fs::create_dir_all(&first).expect("first segment");
    fs::create_dir_all(&second).expect("second segment");
    fs::write(first.join("pending.webm"), b"abc").expect("pending");
    fs::write(first.join("other.webm"), b"0123456789").expect("other");
    fs::write(second.join("raw.webm"), b"01234567890123456789").expect("raw");
    append_offload_event(
        root.path(),
        "20260101",
        "_default",
        "010000_001",
        "snapshot-a",
        &[OffloadFile {
            name: "pending.webm".into(),
            bytes: 3,
            sha256: "a".repeat(64),
        }],
        1,
    )
    .expect("first ledger");
    append_offload_event(
        root.path(),
        "20260102",
        "_default",
        "020000_001",
        "snapshot-b",
        &[OffloadFile {
            name: "backup.webm".into(),
            bytes: 7,
            sha256: "b".repeat(64),
        }],
        2,
    )
    .expect("second ledger");
    root
}

pub fn degraded_offload_root() -> TempDir {
    let root = offload_inventory_root();
    let path = ledger_path_for_day(root.path(), "20260103").expect("ledger path");
    fs::create_dir_all(path.parent().expect("parent")).expect("offload dir");
    fs::write(&path, "{not json\n").expect("corrupt ledger");
    root
}

pub fn unreadable_offload_root() -> TempDir {
    let root = offload_inventory_root();
    let path = ledger_path_for_day(root.path(), "20260104").expect("ledger path");
    fs::create_dir_all(&path).expect("ledger path as directory");
    root
}

pub fn write_ready_restic(dir: &Path) -> PathBuf {
    let binary = binary_path(dir);
    fs::write(&binary, b"restic-fixture").expect("restic fixture");
    let digest = file_sha256(&binary).expect("digest");
    let (os, arch) = platform_info().expect("platform");
    fs::write(
        sentinel_path(dir),
        serde_json::to_vec(&json!({
            "schema_version": RESTIC_SCHEMA_VERSION,
            "tool": RESTIC_TOOL,
            "version": RESTIC_VERSION,
            "sha256": digest,
            "platform": {"os": os, "arch": arch},
            "binary_path": binary,
        }))
        .expect("sentinel"),
    )
    .expect("write sentinel");
    binary
}
