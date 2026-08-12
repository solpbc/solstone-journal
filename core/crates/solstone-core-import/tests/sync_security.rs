// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use solstone_core_import::{
    PlaudHttp, PlaudRemoteFile, PlaudSyncOptions, SyncActionSeams, sync_plaud_with_http,
};

struct TokenCheckingHttp {
    expected: &'static str,
}

impl PlaudHttp for TokenCheckingHttp {
    fn list_files(
        &mut self,
        access_token: &str,
    ) -> Result<Vec<PlaudRemoteFile>, solstone_core_import::ImportError> {
        assert_eq!(access_token, self.expected);
        Ok(vec![PlaudRemoteFile {
            id: "one".to_owned(),
            filename: "one.mp3".to_owned(),
            fullname: "one".to_owned(),
            filesize: 1,
            start_time: 1,
            duration: 31_000,
            is_trash: false,
        }])
    }
}

#[test]
fn supplied_credential_never_enters_state_report_error_or_journal_tree() {
    const TOKEN: &str = "PLAUD_ACCESS_TOKEN_SENTINEL";
    let temporary = tempfile::tempdir().unwrap();
    let mut http = TokenCheckingHttp { expected: TOKEN };
    let mut seams = SyncActionSeams {
        per_item_action: |_: solstone_core_import::SyncActionRequest<'_>| Ok(()),
    };

    let report = sync_plaud_with_http(
        &PlaudSyncOptions {
            journal: temporary.path(),
            save: false,
            access_token: TOKEN,
        },
        &mut http,
        &mut seams,
    )
    .unwrap();

    assert!(!format!("{report:?}").contains(TOKEN));
    let state = fs::read_to_string(temporary.path().join("imports/plaud.json")).unwrap();
    assert!(!state.contains(TOKEN));
    assert!(!format!("{:?}", temporary.path()).contains(TOKEN));
}
