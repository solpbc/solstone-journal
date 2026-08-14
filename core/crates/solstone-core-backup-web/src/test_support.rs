use serde_json::{Value, json};
use std::fs;
use tempfile::TempDir;

pub const RECOVERY_KEY: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ";
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
    if phase == "corrupt" {
        fs::write(
            config.join("journal.json"),
            b"{\"setup\": {\"completed_at\": 17672256",
        )
        .expect("corrupt");
        return root;
    }
    let mut document = json!({"setup":{"completed_at":1767225600}});
    if phase != "fresh" {
        document["backup"] = backup(phase);
    }
    fs::write(
        config.join("journal.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&document).expect("json")
        ),
    )
    .expect("config");
    root
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
