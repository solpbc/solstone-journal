use super::common;
use serde_json::Value;
use solstone_core_observer::store::format::render_list;
use solstone_core_observer::store::reload::load_observers;

#[test]
fn list_json_matches_python() {
    let root = common::root("list-json");
    let now = common::now_ms();
    let fixture = common::seed_full_fixture(&root, now);
    let records = load_observers(&root).expect("records");
    let rust: Value = serde_json::from_str(&render_list(&records, true, now)).expect("rust JSON");
    let oracle = common::oracle(&root, "list", true, now, None);
    assert_eq!(
        rust,
        serde_json::from_str::<Value>(oracle["stdout"].as_str().expect("stdout").trim())
            .expect("python JSON")
    );
    let rust_names: Vec<_> = rust
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|record| record["name"].as_str())
        .collect();
    assert!(rust_names.contains(&fixture.bound_name));
    assert!(rust_names.contains(&fixture.unbound_name));
    for excluded in fixture.excluded_names {
        assert!(
            !rust_names.contains(&excluded),
            "{excluded} must be skipped"
        );
        assert!(
            !oracle["stdout"]
                .as_str()
                .expect("stdout")
                .contains(excluded),
            "Python must skip {excluded}"
        );
    }
    common::cleanup(root);
}
