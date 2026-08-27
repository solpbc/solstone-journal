// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[allow(dead_code)]
#[path = "support/mod.rs"]
mod support;

#[path = "calendar_routes.rs"]
mod calendar_routes;
#[path = "corpus.rs"]
mod corpus;
#[path = "devices_ingest_mount.rs"]
mod devices_ingest_mount;
#[path = "discovery_routes.rs"]
mod discovery_routes;
#[path = "import_journal_door_mount.rs"]
mod import_journal_door_mount;
#[path = "media_routes.rs"]
mod media_routes;
#[path = "network_corpus.rs"]
mod network_corpus;
#[path = "network_write_routes.rs"]
mod network_write_routes;
#[path = "populated_corpus.rs"]
mod populated_corpus;
#[path = "push_mount.rs"]
mod push_mount;
#[path = "quality_known_routes.rs"]
mod quality_known_routes;
#[path = "review_routes.rs"]
mod review_routes;
#[path = "thinking_corpus.rs"]
mod thinking_corpus;
