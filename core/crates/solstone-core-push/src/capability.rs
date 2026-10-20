// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{JsonWriteOptions, write_json};

use crate::envelope::PushKey;
use crate::model::PushPlatform;
use crate::store::PushStoreError;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CapabilityDocument {
    pub(crate) version: u32,
    pub(crate) grants: Vec<CapabilityGrant>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CapabilityGrant {
    pub(crate) platform: PushPlatform,
    pub(crate) cid: String,
    pub(crate) identity: String,
    pub(crate) push_key: PushKey,
    pub(crate) registered_at: String,
}

pub(crate) fn grant_matches(
    grant: &CapabilityGrant,
    platform: PushPlatform,
    cid: &str,
    identity: &str,
    push_key: &PushKey,
    registered_at: &str,
) -> bool {
    grant.platform == platform
        && grant.cid == cid
        && grant.identity == identity
        && &grant.push_key == push_key
        && grant.registered_at == registered_at
}

pub(crate) fn sort_grants(grants: &mut [CapabilityGrant]) {
    grants.sort_by(|a, b| {
        let rank_a = match a.platform {
            PushPlatform::Ios => 0,
            PushPlatform::Android => 1,
        };
        let rank_b = match b.platform {
            PushPlatform::Ios => 0,
            PushPlatform::Android => 1,
        };
        rank_a
            .cmp(&rank_b)
            .then_with(|| a.cid.cmp(&b.cid))
            .then_with(|| a.identity.cmp(&b.identity))
            .then_with(|| a.registered_at.cmp(&b.registered_at))
    });
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum CapabilityReadStatus {
    Ready(Vec<CapabilityGrant>),
    Missing,
    Malformed,
    Unreadable,
}

#[cfg(test)]
type TestWriteHook = Box<dyn Fn(&CapabilityDocument) -> Result<(), std::io::Error> + Send>;

#[cfg(test)]
thread_local! {
    static TEST_READ_FAULT: std::cell::RefCell<bool> = const { std::cell::RefCell::new(false) };
    static TEST_WRITE_HOOK: std::cell::RefCell<Option<TestWriteHook>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_test_read_fault(fault: bool) {
    TEST_READ_FAULT.with(|f| *f.borrow_mut() = fault);
}

#[cfg(test)]
pub(crate) fn get_test_read_fault() -> bool {
    TEST_READ_FAULT.with(|f| *f.borrow())
}

#[cfg(test)]
pub(crate) fn set_test_write_hook(hook: Option<TestWriteHook>) {
    TEST_WRITE_HOOK.with(|h| *h.borrow_mut() = hook);
}

#[cfg(test)]
pub(crate) fn clear_test_capability_hooks() {
    TEST_READ_FAULT.with(|f| *f.borrow_mut() = false);
    TEST_WRITE_HOOK.with(|h| *h.borrow_mut() = None);
}

pub(crate) fn read_capability_document(path: &Path) -> CapabilityReadStatus {
    #[cfg(test)]
    if TEST_READ_FAULT.with(|f| *f.borrow()) {
        log::warn!("push presentation capability unreadable");
        return CapabilityReadStatus::Unreadable;
    }

    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return CapabilityReadStatus::Missing;
        }
        Err(_) => {
            log::warn!("push presentation capability unreadable");
            return CapabilityReadStatus::Unreadable;
        }
    };

    let doc: CapabilityDocument = match serde_json::from_str(&contents) {
        Ok(d) => d,
        Err(_) => {
            log::warn!("push presentation capability malformed");
            return CapabilityReadStatus::Malformed;
        }
    };

    if doc.version != 1 {
        log::warn!("push presentation capability malformed");
        return CapabilityReadStatus::Malformed;
    }

    CapabilityReadStatus::Ready(doc.grants)
}

pub(crate) fn write_capability_document(
    path: &Path,
    mut doc: CapabilityDocument,
) -> Result<(), PushStoreError> {
    sort_grants(&mut doc.grants);

    #[cfg(test)]
    {
        let hook_result = TEST_WRITE_HOOK.with(|h| h.borrow().as_ref().map(|hook| hook(&doc)));
        if let Some(Err(io_err)) = hook_result {
            log::warn!("push presentation capability write failed");
            return Err(PushStoreError::Write(
                solstone_core_journal_io::AtomicWriteError::Io {
                    path: path.to_path_buf(),
                    source: io_err,
                },
            ));
        }
    }

    let write_res = write_json(
        path,
        &doc,
        JsonWriteOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    );

    match write_res {
        Ok(()) => Ok(()),
        Err(err) => {
            log::warn!("push presentation capability write failed");
            Err(PushStoreError::Write(err))
        }
    }
}
