// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod batch;
mod matcher;
mod normalize;
mod slug;

pub use batch::{build_name_resolution_map, find_entity_by_email, is_name_variant_match};
pub use matcher::{
    EntityNameCandidate, EntityNameMatch, EntityNameMatchOutcome, MatchTier, char_len,
    find_matching_entity, find_matching_entity_detailed, first_word_match, prefix_token_match,
    single_token_first_word_match, token_sort, token_sort_ratio, token_subset_match,
};
pub use normalize::{matchable_resolution_query, normalize_resolution_query};
pub use slug::{MAX_ENTITY_SLUG_LENGTH, entity_slug};
