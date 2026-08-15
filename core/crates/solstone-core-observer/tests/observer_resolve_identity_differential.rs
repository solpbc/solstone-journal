use super::common;

use serde_json::json;
use solstone_core_observer::{ObserverCommand, execute};

#[test]
fn python_identity_resolution_requires_bound_renamed_record_and_allows_unbound_twin() {
    let root = common::root("resolve-identity");
    let fingerprint = format!("sha256:{}", "c".repeat(64));
    let bound = common::write_record(
        &root,
        json!({"key":"abcdefgh123", "name":"before-rename", "enabled":true, "stats":{}, "device_binding":{"device":fingerprint,"kind":"cert"}}),
    );
    common::write_record(
        &root,
        json!({"key":"ijklmnop123", "name":"unbound", "enabled":true, "stats":{}}),
    );
    execute(
        &root,
        ObserverCommand::Rename {
            old: "before-rename".into(),
            new: "after-rename".into(),
            json: false,
        },
        common::now_ms(),
    )
    .expect("native rename");
    let result = common::python_json(
        &root,
        r#"import json, os, sys
from flask import Flask, g
from solstone.convey import state
from solstone.convey.secure_listener import ConveyIdentity
from solstone.apps.observer.utils import resolve_observer_identity
from solstone.observe.protocol import OBSERVER_HANDLE_HEADER
from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.paths import authorized_clients_path
state.journal_root = os.environ['SOLSTONE_JOURNAL']
payload = json.load(sys.stdin)
app = Flask(__name__)
AuthorizedClients(authorized_clients_path()).add(payload['fingerprint'], 'observer', 'instance-1')
identity = ConveyIdentity(mode='pl-direct', fingerprint=payload['fingerprint'], device_label='observer', paired_at='2026-04-20T00:00:00Z', session_id=None)
with app.test_request_context(headers={OBSERVER_HANDLE_HEADER: payload['bound_key']}):
    g.identity = identity
    observer, prefix, error = resolve_observer_identity()
    positive = {'name': observer.get('name') if observer else None, 'prefix': prefix, 'error': error is None}
with app.test_request_context(headers={OBSERVER_HANDLE_HEADER: payload['unbound_key']}):
    g.identity = ConveyIdentity(mode='pl-direct', fingerprint='sha256:' + ('d' * 64), device_label='other', paired_at='2026-04-20T00:00:00Z', session_id=None)
    observer, prefix, error = resolve_observer_identity()
    negative = {'name': observer.get('name') if observer else None, 'prefix': prefix, 'error': error is None}
json.dump({'positive': positive, 'negative': negative}, sys.stdout)
"#,
        json!({"fingerprint":fingerprint,"bound_key":bound.key(),"unbound_key":"ijklmnop123"}),
    );
    assert_eq!(
        result["positive"],
        json!({"name":"after-rename","prefix":"abcdefgh","error":true})
    );
    assert_eq!(
        result["negative"],
        json!({"name":"unbound","prefix":"ijklmnop","error":true})
    );
    common::cleanup(root);
}
