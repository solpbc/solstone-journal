// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeObjectCrtDisposition {
    FollowsCcDefaultFlags,
    FollowsCcWithExactTokenStaticCrt,
    FollowsOwnedMsvcSelector,
    UnresolvedPendingNativeLinkQualification,
    NotACrtObject,
}

#[allow(dead_code)]
pub(crate) struct NativeObjectCrtRow {
    pub(crate) name: &'static str,
    pub(crate) disposition: NativeObjectCrtDisposition,
}

#[allow(dead_code)]
pub(crate) fn windows_native_crt_dispositions() -> &'static [NativeObjectCrtRow] {
    &[
        NativeObjectCrtRow {
            name: "ring",
            disposition: NativeObjectCrtDisposition::FollowsCcDefaultFlags,
        },
        NativeObjectCrtRow {
            name: "libsqlite3-sys",
            disposition: NativeObjectCrtDisposition::FollowsCcWithExactTokenStaticCrt,
        },
        NativeObjectCrtRow {
            name: "ffmpeg",
            disposition: NativeObjectCrtDisposition::FollowsOwnedMsvcSelector,
        },
        NativeObjectCrtRow {
            name: "WebView2LoaderStatic.lib",
            disposition: NativeObjectCrtDisposition::UnresolvedPendingNativeLinkQualification,
        },
        NativeObjectCrtRow {
            name: "windows_version_resource",
            disposition: NativeObjectCrtDisposition::NotACrtObject,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_native_crt_dispositions_order_and_contents() {
        let table = windows_native_crt_dispositions();
        assert_eq!(table.len(), 5);
        assert_eq!(table[0].name, "ring");
        assert_eq!(
            table[0].disposition,
            NativeObjectCrtDisposition::FollowsCcDefaultFlags
        );
        assert_eq!(table[1].name, "libsqlite3-sys");
        assert_eq!(
            table[1].disposition,
            NativeObjectCrtDisposition::FollowsCcWithExactTokenStaticCrt
        );
        assert_eq!(table[2].name, "ffmpeg");
        assert_eq!(
            table[2].disposition,
            NativeObjectCrtDisposition::FollowsOwnedMsvcSelector
        );
        assert_eq!(table[3].name, "WebView2LoaderStatic.lib");
        assert_eq!(
            table[3].disposition,
            NativeObjectCrtDisposition::UnresolvedPendingNativeLinkQualification
        );
        assert_eq!(table[4].name, "windows_version_resource");
        assert_eq!(
            table[4].disposition,
            NativeObjectCrtDisposition::NotACrtObject
        );
    }
}
