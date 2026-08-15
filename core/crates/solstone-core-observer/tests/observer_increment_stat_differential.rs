use super::common;

use serde_json::json;

#[test]
fn python_increment_stat_advances_native_written_empty_stats_without_attrition() {
    let root = common::root("increment-stat");
    let record = common::write_record(
        &root,
        json!({"key":"abcdefgh123", "name":"counter", "stats":{"note":"keep"}}),
    );
    let result = common::python_json(
        &root,
        r#"import importlib.util, json, os, sys
from solstone.convey import state
state.journal_root = os.environ['SOLSTONE_JOURNAL']
path = os.path.join(os.environ['SOLSTONE_REPO_ROOT'], 'solstone/apps/observer/utils.py')
spec = importlib.util.spec_from_file_location('observer_utils_oracle', path)
utils = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = utils
spec.loader.exec_module(utils)
payload = json.load(sys.stdin)
utils.increment_stat(payload['prefix'], 'segments_received')
utils.increment_stat(payload['prefix'], 'segments_received')
with open(os.path.join(os.environ['SOLSTONE_JOURNAL'], 'apps/observer/observers', payload['prefix'] + '.json')) as handle:
    json.dump(json.load(handle), sys.stdout)
"#,
        json!({"prefix": record.prefix()}),
    );
    assert_eq!(result["stats"]["segments_received"], 2);
    assert_eq!(result["stats"]["note"], "keep");
    common::cleanup(root);
}
