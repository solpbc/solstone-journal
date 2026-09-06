// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Browser copy mirrored from the Python curation application.

use serde_json::{Map, Value};

pub fn payload() -> Value {
    Value::Object(
        COPY.iter()
            .map(|(name, value)| ((*name).to_owned(), Value::String((*value).to_owned())))
            .collect::<Map<_, _>>(),
    )
}

const COPY: [(&str, &str); 52] = [
    ("CUR_HEADING", "suggestions"),
    (
        "CUR_FACET_BODY",
        "journal activity doesn't fit your facets well. create the \"{name}\" facet?",
    ),
    ("CUR_FACET_CREATE_ACTION", "create facet"),
    ("CUR_FACET_DISMISS_ACTION", "not now"),
    (
        "CUR_ENTITY_BODY",
        "\"{a}\" and \"{b}\" look like the same entity. merge them?",
    ),
    ("CUR_ENTITY_MERGE_ACTION", "merge"),
    ("CUR_ENTITY_DISMISS_ACTION", "keep separate"),
    ("CUR_ENTITY_FACETS_LABEL", "in {facets}"),
    (
        // G2-40: the lede a large duplicate-entity group opens on, framed as
        // what clearing the visible batch gets the owner rather than the
        // full count owed.
        "CUR_ENTITY_GROUP_LEDE",
        "{count} names look like duplicates. clearing them tidies your whole journal.",
    ),
    ("CUR_ENTITY_SHOW_REST_ACTION", "show the rest ({count})"),
    (
        "CUR_ENTITY_PROGRESS_LABEL",
        "{reviewed} reviewed · {left} left",
    ),
    ("CUR_ENTITY_DONE_FOR_NOW_ACTION", "done for now"),
    (
        "CUR_SPEAKER_BODY",
        "solstone noticed \"{source}\" and \"{target}\" may be the same speaker. merge them?",
    ),
    ("CUR_SPEAKER_MERGE_ACTION", "review merge"),
    ("CUR_SPEAKER_DISMISS_ACTION", "keep separate"),
    (
        "CUR_SPEAKER_CANDIDATE_PAIR_BODY",
        "two voices in your journal sound alike. merge them?",
    ),
    (
        "CUR_SPEAKER_CANDIDATE_PAIR_MERGE_ACTION",
        "merge candidates",
    ),
    ("CUR_SPEAKER_CANDIDATE_PAIR_DISMISS_ACTION", "keep separate"),
    ("CUR_SPEAKER_CANDIDATE_PAIR_SIMILARITY_LABEL", "cosine"),
    ("CUR_SPEAKER_CANDIDATE_PAIR_INTERVALS_LABEL", "intervals"),
    ("CUR_SPEAKER_CANDIDATE_PAIR_SOURCE_LABEL", "candidate A"),
    ("CUR_SPEAKER_CANDIDATE_PAIR_TARGET_LABEL", "candidate B"),
    (
        "CUR_EMPTY_STATE",
        "nothing to review — solstone hasn't spotted new structure to suggest.",
    ),
    (
        "CUR_ENTITY_PREVIEW_LEAD",
        "before merging, here's what will change.",
    ),
    ("CUR_ENTITY_CONFIRM_ACTION", "confirm merge"),
    ("CUR_ENTITY_CANCEL_ACTION", "cancel"),
    ("CUR_ENTITY_SELECT_ALL_ACTION", "select all"),
    ("CUR_ENTITY_BATCH_MERGE_ACTION", "merge selected"),
    ("CUR_ENTITY_BATCH_DISMISS_ACTION", "keep selected separate"),
    ("CUR_ENTITY_BATCH_MERGE_LEAD", "these pairs will be merged:"),
    (
        "CUR_ENTITY_BATCH_DISMISS_LEAD",
        "these pairs will be kept separate:",
    ),
    ("CUR_ENTITY_BATCH_CONFIRM_MERGE_ACTION", "merge all"),
    (
        "CUR_ENTITY_BATCH_CONFIRM_DISMISS_ACTION",
        "keep all separate",
    ),
    ("CUR_ENTITY_BATCH_MERGE_SUMMARY", "merged {ok} of {total}."),
    (
        "CUR_ENTITY_BATCH_DISMISS_SUMMARY",
        "kept {ok} of {total} separate.",
    ),
    (
        "CUR_ENTITY_BATCH_FAILED_NOTE",
        "{failed} still need attention below.",
    ),
    ("CUR_AMBIGUITY_BODY", "which entry matches \"{query}\"?"),
    ("CUR_AMBIGUITY_ORIGIN_LABEL", "noticed in"),
    ("CUR_AMBIGUITY_CHOOSE_ACTION", "choose {name}"),
    ("CUR_UNDO_ACTION", "undo merge"),
    ("CUR_UNDO_DONE", "merge undone."),
    (
        "CUR_UNDO_UNAVAILABLE",
        "undo isn't available for this earlier merge.",
    ),
    ("CUR_UNDO_FAILED", "the merge couldn't be undone."),
    ("CUR_REPAIR_REQUIRED", "{detail} {remediation}"),
    (
        "CUR_ENTITY_PREVIEW_EMPTY",
        "no journal changes are needed for this merge.",
    ),
    (
        "CUR_ENTITY_PREVIEW_ERRORS",
        "some segment updates may need attention.",
    ),
    ("CUR_PREVIEW_AKAS_LABEL", "aliases added"),
    ("CUR_PREVIEW_EMAILS_LABEL", "emails added"),
    ("CUR_PREVIEW_FACETS_LABEL", "facet links"),
    ("CUR_PREVIEW_OBSERVATIONS_LABEL", "notes moved"),
    ("CUR_PREVIEW_SEGMENTS_LABEL", "speaker labels updated"),
    ("CUR_PREVIEW_VOICEPRINTS_LABEL", "voice samples moved"),
];
