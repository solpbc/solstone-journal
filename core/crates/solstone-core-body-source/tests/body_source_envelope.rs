// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "support/mod.rs"]
mod support;

#[path = "body_envelope_decode_bounded.rs"]
mod body_envelope_decode_bounded;
#[path = "body_envelope_decode_days.rs"]
mod body_envelope_decode_days;
#[path = "body_envelope_decode_fixture.rs"]
mod body_envelope_decode_fixture;
#[path = "body_envelope_decode_ledger_summary.rs"]
mod body_envelope_decode_ledger_summary;
#[path = "body_envelope_decode_matrix.rs"]
mod body_envelope_decode_matrix;
#[path = "body_envelope_decode_precedence.rs"]
mod body_envelope_decode_precedence;
#[path = "body_envelope_decode_shards.rs"]
mod body_envelope_decode_shards;
#[path = "body_envelope_encode_bounded.rs"]
mod body_envelope_encode_bounded;
#[path = "body_envelope_encode_fixture.rs"]
mod body_envelope_encode_fixture;
#[path = "body_envelope_encode_numeric.rs"]
mod body_envelope_encode_numeric;
#[path = "body_envelope_encode_twins.rs"]
mod body_envelope_encode_twins;
#[path = "body_envelope_error.rs"]
mod body_envelope_error;
#[path = "body_envelope_fixture.rs"]
mod body_envelope_fixture;
#[path = "body_envelope_manifest_binding.rs"]
mod body_envelope_manifest_binding;
#[path = "body_wire_identity_error.rs"]
mod body_wire_identity_error;
#[path = "coordinate_error.rs"]
mod coordinate_error;
#[path = "envelope_error.rs"]
mod envelope_error;
#[path = "envelope_ledger_error.rs"]
mod envelope_ledger_error;
#[path = "envelope_ledger_fixture.rs"]
mod envelope_ledger_fixture;
#[path = "envelope_ledger_fuzz.rs"]
mod envelope_ledger_fuzz;
#[path = "envelope_shard_error.rs"]
mod envelope_shard_error;
#[path = "envelope_shard_fixture.rs"]
mod envelope_shard_fixture;
#[path = "envelope_shard_fuzz.rs"]
mod envelope_shard_fuzz;
