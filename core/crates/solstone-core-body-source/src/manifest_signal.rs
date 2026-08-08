// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// The observable body-manifest key signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestKeySignal {
    BodyKeyPresent { unknown_body_key: bool },
    NoBodyKey,
    Unreadable,
}

/// Inspects an optional manifest input for top-level body-prefixed keys.
pub fn inspect_body_manifest_signal(input: Option<&[u8]>) -> ManifestKeySignal {
    let Some(bytes) = input else {
        return ManifestKeySignal::Unreadable;
    };

    match crate::manifest_scan::scan_body_manifest(bytes) {
        Err(_) => ManifestKeySignal::Unreadable,
        Ok(scanned) if scanned.has_body_prefixed_key() => ManifestKeySignal::BodyKeyPresent {
            unknown_body_key: scanned.has_unknown_body_prefixed_key(),
        },
        Ok(_) => ManifestKeySignal::NoBodyKey,
    }
}
