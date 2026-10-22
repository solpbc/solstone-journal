// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The enqueue seam for each family discovery can index. A new family must
//! name its producer here; only material the app never writes is scan-only.

use solstone_core_format::content::Family;

pub const PRODUCERS: &[(Family, &str)] = &[
    (
        Family::Markdown,
        "talent output, import publication, location projection and weekly reflection",
    ),
    (Family::Activity, "facets store publication"),
    (Family::ActionLog, "facet and journal action logs"),
    (Family::StructuredImport, "import publication"),
    (Family::AiChat, "import publication"),
    (Family::Browser, "ingest retained text publication"),
    (Family::DayAccumulator, "talent day accumulator"),
    (Family::FacetEntity, "facets store publication"),
    (Family::Observation, "entity facet observations"),
    (Family::Documents, "think segment publication"),
    (Family::Screen, "think segment publication"),
    (
        Family::ScreenText,
        "describe screen-text publication and removal",
    ),
    (Family::Sense, "think segment publication"),
    (Family::MorningBriefing, "daily talent publication"),
    (Family::AgentMemory, "memory note publication"),
    (
        Family::Workout,
        "import publication and full import scan token",
    ),
    (Family::ProtectedVideo, "ingest retained text publication"),
    (Family::DesktopActivity, "ingest retained text publication"),
    (
        Family::Calendar,
        "ingest calendar note publication and calendar import publication",
    ),
];

pub const CATCH_ALL_ONLY: &[(Family, &str)] = &[(
    Family::Event,
    "legacy facet events: no application writer; retained files and owner hand edits",
)];
