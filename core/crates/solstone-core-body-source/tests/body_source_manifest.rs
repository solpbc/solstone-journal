// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "support/mod.rs"]
mod support;

#[path = "body_shard_validator.rs"]
mod body_shard_validator;
#[path = "bundle_id_fixture.rs"]
mod bundle_id_fixture;
#[path = "bundle_id_fuzz.rs"]
mod bundle_id_fuzz;
#[path = "health_hash.rs"]
mod health_hash;
#[path = "manifest_binding_apply.rs"]
mod manifest_binding_apply;
#[path = "manifest_binding_error.rs"]
mod manifest_binding_error;
#[path = "manifest_binding_fixture.rs"]
mod manifest_binding_fixture;
#[path = "manifest_decode_bounded.rs"]
mod manifest_decode_bounded;
#[path = "manifest_decode_fixture.rs"]
mod manifest_decode_fixture;
#[path = "manifest_decode_oracle.rs"]
mod manifest_decode_oracle;
#[path = "manifest_decode_precedence.rs"]
mod manifest_decode_precedence;
#[path = "manifest_scan_error.rs"]
mod manifest_scan_error;
#[path = "manifest_scan_fixture.rs"]
mod manifest_scan_fixture;
#[path = "manifest_signal.rs"]
mod manifest_signal;
