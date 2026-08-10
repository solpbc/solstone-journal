mod common;
use solstone_core_observer::store::format::render_list;
use solstone_core_observer::store::reload::load_observers;

#[test]
fn list_human_table_is_byte_exact_python_oracle() {
    common::with_utc_tz(|| {
        let root = common::root("list-human");
        let now = common::now_ms();
        common::seed_full_fixture(&root, now);
        let output = render_list(&load_observers(&root).expect("records"), false, now);
        let oracle = common::oracle(&root, "list", false, now, None);
        assert_eq!(
            format!("{output}\n").as_bytes(),
            oracle["stdout"].as_str().expect("stdout").as_bytes()
        );
        common::cleanup(root);
    });
}
