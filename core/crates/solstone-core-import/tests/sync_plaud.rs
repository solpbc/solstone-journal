// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::Cell;

use solstone_core_import::{
    PlaudHttp, PlaudRemoteFile, PlaudSyncOptions, SyncActionSeams, sync_plaud_with_http,
};

struct Double {
    calls: Cell<u8>,
}

impl PlaudHttp for Double {
    fn list_files(
        &mut self,
        access_token: &str,
    ) -> Result<Vec<PlaudRemoteFile>, solstone_core_import::ImportError> {
        assert_eq!(access_token, "explicit-token");
        self.calls.set(self.calls.get() + 1);
        Ok(vec![PlaudRemoteFile {
            id: "trashed".to_owned(),
            filename: "private.mp3".to_owned(),
            fullname: "private".to_owned(),
            filesize: 9,
            start_time: 8,
            duration: 120_000,
            is_trash: true,
        }])
    }
}

#[test]
fn injected_http_catalogues_without_a_live_network_call() {
    let temporary = tempfile::tempdir().unwrap();
    let mut http = Double {
        calls: Cell::new(0),
    };
    let mut seams = SyncActionSeams {
        per_item_action: |_: solstone_core_import::SyncActionRequest<'_>| {
            panic!("a skipped Plaud item must not invoke the action seam")
        },
    };

    let report = sync_plaud_with_http(
        &PlaudSyncOptions {
            journal: temporary.path(),
            save: true,
            access_token: "explicit-token",
        },
        &mut http,
        &mut seams,
    )
    .unwrap();

    assert_eq!(http.calls.get(), 1);
    assert_eq!(report.skipped, 1);
}
