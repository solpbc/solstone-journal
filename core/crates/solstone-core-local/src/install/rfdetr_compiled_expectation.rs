// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[derive(Debug, Clone, Copy)]
pub struct CompiledDeliveryContract {
    #[allow(dead_code)]
    // Retained in the fixed generated-value contract for later evidence binding.
    pub delivery_contract_sha256: &'static str,
    pub slot_id: &'static str,
    pub archive_sha256: &'static str,
    pub archive_size: u64,
    pub executable_member_path: &'static str,
    pub executable_sha256: &'static str,
}

include!(concat!(
    env!("OUT_DIR"),
    "/rfdetr_compiled_expectation_value.rs"
));
