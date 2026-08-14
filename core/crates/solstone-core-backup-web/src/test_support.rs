use serde_json::{Map, Value, json};
use std::fs;
use tempfile::TempDir;

pub const RECOVERY_KEY: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ";
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
