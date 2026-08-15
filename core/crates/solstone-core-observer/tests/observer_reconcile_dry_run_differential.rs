use super::common;

use serde_json::json;
use solstone_core_observer::store::reconcile::reconcile_plan;
use solstone_core_observer::store::reload::load_observers;

#[test]
fn reconcile_dry_run_plan_matches_python_across_duplicate_edge_cases_without_writes() {
    let root = common::root("reconcile");
    let now = common::now_ms();
    common::write_record(
        &root,
        json!({"key":"aaaaaaaa111","name":"triple","created_at":now-3000,"stats":{"segments_received":1,"bytes_received":10,"duplicates_rejected":true,"note":"ignored"}}),
    );
    common::write_record(
        &root,
        json!({"key":"bbbbbbbb222","name":"triple","created_at":now-2000,"stats":{"segments_received":2,"bytes_received":20,"duplicates_rejected":2,"ignored":false}}),
    );
    common::write_record(
        &root,
        json!({"key":"cccccccc333","name":"triple","created_at":now-1000,"stats":{"segments_received":4,"bytes_received":40,"fraction":1.5}}),
    );
    common::write_record(
        &root,
        json!({"key":"dddddddd444","name":"single","created_at":now-500,"stats":{"segments_received":99}}),
    );
    common::write_record(
        &root,
        json!({"key":"eeeeeeee555","name":"revoked-same","created_at":now-400,"revoked":true,"stats":{"segments_received":1}}),
    );
    common::write_record(
        &root,
        json!({"key":"ffffffff666","name":"revoked-same","created_at":now-300,"revoked":true,"stats":{"segments_received":2}}),
    );
    common::write_record(
        &root,
        json!({"key":"gggggggg777","name":"","created_at":now-250,"stats":{"segments_received":3,"bytes_received":30}}),
    );
    common::write_record(
        &root,
        json!({"key":"hhhhhhhh888","name":"","created_at":now-150,"stats":{"segments_received":5,"bytes_received":50}}),
    );
    let before = snapshot(&root);
    let records = load_observers(&root).expect("records");
    let rust = serde_json::to_value(reconcile_plan(&records).iter().map(|entry| serde_json::json!({"name":entry.name,"survivor_prefix":entry.survivor_prefix,"revoked_prefixes":entry.revoked_prefixes,"stats":entry.stats})).collect::<Vec<_>>()).expect("value");
    let oracle = common::oracle(&root, "reconcile", false, now, None);
    assert_eq!(rust, oracle["code"]);
    let names: Vec<_> = rust
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(names.contains(&"triple"));
    assert!(names.contains(&""));
    assert!(!names.contains(&"single"));
    assert!(!names.contains(&"revoked-same"));
    let triple = rust
        .as_array()
        .expect("array")
        .iter()
        .find(|entry| entry["name"] == "triple")
        .expect("triple");
    assert_eq!(triple["stats"]["segments_received"], 7);
    assert_eq!(triple["stats"]["bytes_received"], 70);
    assert_eq!(triple["stats"]["duplicates_rejected"], 2);
    assert!(triple["stats"].get("note").is_none());
    assert!(triple["stats"].get("ignored").is_none());
    assert_eq!(snapshot(&root), before);
    common::cleanup(root);
}

fn snapshot(root: &std::path::Path) -> Vec<(String, u64, u128)> {
    fn walk(
        root: &std::path::Path,
        current: &std::path::Path,
        rows: &mut Vec<(String, u64, u128)>,
    ) {
        for entry in std::fs::read_dir(current).expect("directory") {
            let path = entry.expect("entry").path();
            let metadata = std::fs::metadata(&path).expect("metadata");
            if metadata.is_dir() {
                walk(root, &path, rows);
            } else {
                rows.push((
                    path.strip_prefix(root)
                        .expect("relative")
                        .display()
                        .to_string(),
                    metadata.len(),
                    metadata
                        .modified()
                        .expect("mtime")
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("epoch")
                        .as_nanos(),
                ));
            }
        }
    }
    let mut rows = Vec::new();
    walk(root, &root.join("apps/observer"), &mut rows);
    rows.sort();
    rows
}
