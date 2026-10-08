// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Installed package root, POSIX manifest, and Windows signed-payload verifier.
//!
//! The package root is the directory that contains `bin/`, `lib/`, and `share/`.
//! It is not the payload root (`share/` or a checkout `core/payload`) and not
//! the versioned-install prefix that owns `current`.

pub mod pin;
pub mod posix;
pub mod windows_payload;

#[cfg(any(windows, test))]
pub mod windows_adapter;

#[cfg(feature = "test-fixture-pin")]
pub use pin::install_test_fixture_pin;
pub use pin::{PRODUCT_PIN, resolve_pin, signed_package_pin_environment, verify_pinned_signature};

pub use posix::{
    COMPILED_VERSION, ExecutablePlatform, INSTALLED_PAYLOAD_MANIFEST, INSTALLED_PAYLOAD_SCHEMA,
    INSTALLED_PAYLOAD_SIGNATURE, InstalledPackage, InstalledPayloadRefusal, PRODUCT,
    TARGET_LINUX_AARCH64, TARGET_LINUX_X86_64, TARGET_MACOS_ARM64, bin_parent_root,
    canonical_target, host_executable_platform, locate_installed_package,
    package_root_from_executable_path, render_installed_payload, verify_installed_package,
};

pub use windows_payload::WINDOWS_PAYLOAD_TARGET as TARGET_WINDOWS_X86_64;

pub mod code {
    pub const MANIFEST_MISSING: &str = "manifest-missing";
    pub const MANIFEST_INVALID: &str = "manifest-invalid";
    pub const UNSUPPORTED_LOCATION: &str = "unsupported-location";
    pub const WRONG_PRODUCT: &str = "wrong-product";
    pub const WRONG_TARGET: &str = "wrong-target";
    pub const RESTART_TO_FINISH_UPDATE: &str = "restart-to-finish-update";
    pub const MEMBER_CHANGED: &str = "member-changed";
    pub const MEMBER_MISSING: &str = "member-missing";
    pub const UNEXPECTED_FILE: &str = "unexpected-file";
    pub const MEMBER_UNREADABLE: &str = "member-unreadable";
    pub const UNSAFE_PATH: &str = "unsafe-path";
}

pub mod guidance {
    pub const PACKAGE_MISMATCH: &str = "your journal's install doesn't match; if you just updated, restart your journal, otherwise reinstall it.";
    pub const RESTART_UPDATE: &str = "restart your journal to finish the update.";
    pub const MANIFEST_MISSING: &str =
        "part of your journal's install is missing; reinstall your journal.";
    pub const MANIFEST_INVALID: &str =
        "part of your journal's install is damaged; reinstall your journal.";
    pub const UNSUPPORTED_LOCATION: &str = "this copy of the journal isn't running from an installed package, so its files can't be checked.";
    pub const WRONG_PRODUCT: &str =
        "what's installed here isn't the journal; reinstall your journal.";
    pub const WRONG_TARGET: &str = "your journal was built for a different kind of computer; reinstall the version for this one.";
    pub const MEMBER_UNREADABLE: &str = "part of your journal's install couldn't be read; if it keeps happening, reinstall your journal.";
    pub const UNSAFE_PATH: &str =
        "part of your journal's install isn't what it should be; reinstall your journal.";
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[must_use]
pub fn compiled_target() -> &'static str {
    TARGET_LINUX_X86_64
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
#[must_use]
pub fn compiled_target() -> &'static str {
    TARGET_LINUX_AARCH64
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[must_use]
pub fn compiled_target() -> &'static str {
    TARGET_MACOS_ARM64
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[must_use]
pub fn compiled_target() -> &'static str {
    TARGET_WINDOWS_X86_64
}

#[cfg(not(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "windows", target_arch = "x86_64"),
)))]
compile_error!("installed payload has no target id for this platform");
