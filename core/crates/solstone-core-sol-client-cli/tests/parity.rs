// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use solstone_core_sol_client::seam::ScriptedHttpTransport;
use solstone_core_sol_client_cli::{DispatchSeams, dispatch_sol_call_with_seams};

#[test]
fn device_owned_terminal_settings_are_unsupported_without_http() {
    for verb in ["show", "set"] {
        let transport = ScriptedHttpTransport::new(vec![]);
        let output = dispatch_sol_call_with_seams(
            &["settings".into(), "observer".into(), verb.into()],
            &BTreeMap::new(),
            "",
            "20260723",
            DispatchSeams {
                transport: &transport,
                clock: None,
                files: None,
                build_identity: None,
                client_item_ids: None,
                notification_sink: None,
            },
        );
        assert_eq!(output.stdout, "");
        assert_eq!(output.stderr, "unsupported command.\n");
        assert_eq!(output.exit, 64);
        transport.assert_done();
    }
}

#[test]
fn retired_commitment_ledger_commands_are_unsupported_without_http() {
    let transport = ScriptedHttpTransport::new(vec![]);
    let output = dispatch_sol_call_with_seams(
        &["ledger".to_string(), "list".to_string()],
        &BTreeMap::new(),
        "",
        "20260723",
        DispatchSeams {
            transport: &transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
        },
    );

    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "unsupported command.\n");
    assert_eq!(output.exit, 64);
    transport.assert_done();
}
