use serde_json::{Map, Value, json};
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockOptions, mutate_journal_config,
};
use std::path::Path;

pub fn defaults() -> Value {
    json!({"enabled":false,"mode":"byo","destination":{"repository":null,"backend":null,"credentials":{}},"daily_key":null,"recovery_key":null,"confirmed_recovery_key":false,"retention":{"hourly":24,"daily":7,"weekly":4,"monthly":12},"offload":{"enabled":false,"budget_bytes":null,"floor_bytes":null},"schedule":{"every":"daily","enabled":false},"last_backup":{"time":null,"snapshot_id":null,"status":null,"error_reason":null},"last_prune":{"time":null,"status":null,"error_reason":null},"last_offload":{"time":null,"status":null,"reason":null,"last_ok_time":null,"files_marked":0,"bytes_marked":0,"ran_out_of_markable_media":false},"last_verification":{"time":null,"status":null,"reason":null,"last_ok_time":null,"checked_subset":null},"last_restore":{"time":null,"status":null,"reason":null,"scope":null,"day":null,"segments_selected":0,"segments_restored":0,"files_expected":0,"files_restored":0,"bytes_expected":0,"bytes_restored":0}})
}
fn merge(default: &Value, source: &Value) -> Value {
    match (default, source) {
        (_, Value::Null) => default.clone(),
        (Value::Object(left), Value::Object(right)) => {
            let mut output = left.clone();
            for (key, value) in right {
                let old = output.get(key).unwrap_or(&Value::Null);
                output.insert(key.clone(), merge(old, value));
            }
            Value::Object(output)
        }
        (_, value) => value.clone(),
    }
}
pub fn backup(root: &Path) -> Result<Map<String, Value>, ()> {
    let config = read_journal_config(root)
        .map_err(|_| ())?
        .config
        .unwrap_or_default();
    Ok(
        merge(&defaults(), config.get("backup").unwrap_or(&Value::Null))
            .as_object()
            .cloned()
            .unwrap(),
    )
}
pub fn mutate<T>(
    root: &Path,
    mutator: impl FnOnce(&mut Map<String, Value>) -> (bool, T),
) -> Result<T, ()> {
    mutate_journal_config(root, LockOptions::default(), |config| {
        let backup = config
            .entry("backup".to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        if !backup.is_object() {
            *backup = Value::Object(Map::new());
        }
        let backup = backup
            .as_object_mut()
            .expect("backup was normalized to an object");
        let (changed, value) = mutator(backup);
        JournalConfigMutation { changed, value }
    })
    .map(|result| result.value)
    .map_err(|_| ())
}
