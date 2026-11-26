// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Generated from contracts/journal-web-host/v1.json
// Regenerate with:
//   cargo test --manifest-path core/Cargo.toml -p solstone-core-repository-contracts --lib journal_web_host_contract::regenerate_journal_web_host_contract -- --ignored
// Do not edit directly; the source definition is the only hand-authored occurrence of the product token and the capability name.

window.solstoneJournalWebHost = {
version: 1,
userAgentProduct: "SolstoneJournalExternal/1",
javascriptCapability: "solstoneJournalExternalNavigation",
};

window.solstoneImportContract = Object.freeze({"capability":"solstoneJournalHost","contextMaxBytes":16384,"drop":"solstone-journal-host-drop","event":"solstone-journal-host-status","pick":"import.pick","reply":"import.reply","status":"import.status","unsupported":"unsupported","version":2});
window.solstoneAuthContract = Object.freeze({"authorize":"auth.authorize","cancel":"auth.cancel","capability":"solstoneJournalHost","prepare":"auth.prepare","prepared":"auth.prepared","result":"auth.result","unsupported":"unsupported","version":2});
window.solstoneExportContract = Object.freeze({"cancel":"export.cancel","capability":"solstoneJournalHost","deviceSpace":"export.device_space","event":"solstone-journal-export-status","pause":"export.pause","resume":"export.resume","reveal":"export.reveal","start":"export.start","status":"export.status","unsupported":"unsupported","version":2});
