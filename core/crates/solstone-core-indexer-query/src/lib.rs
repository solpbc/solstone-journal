// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Compilation and read-only execution of journal full-text queries.

mod atomize;
mod compile;
mod edges;
mod execute;
mod ladder;
mod predicate;
mod temporal;
mod types;

#[cfg(test)]
mod test_support;

pub use compile::{CompileOutcome, QueryCompilation, compile_query};
pub use edges::{
    DirectedCounts, EdgeEvidenceRequest, EdgeEvidenceResponse, EdgeFilters, EdgeFiltersPayload,
    EdgeQueryError, EntityTypeLookup, EvidenceRow, KindSummary, NetworkNeighbor,
    NetworkOverviewRequest, NetworkOverviewResponse, NetworkRequest, NetworkResponse,
    OverviewEntity, OverviewTotals, is_safe_entity_id_component, load_edge_evidence,
    load_entity_network, load_network_overview, open_edges_reader,
};
pub use execute::{
    IndexedEntry, agents, coverage, hit_at, indexed_entity_ids, read_indexed_entry, search,
    search_counts,
};
pub use predicate::{EffectiveDateConstraint, PredicateInput, QueryPredicate};
pub use temporal::{TemporalExtraction, extract_temporal_references};
pub use types::{
    CountsResponse, CoverageResponse, CoverageState, IndexAccessError, IndexBuildCounts,
    IndexDegraded, Order, SearchHit, SearchMetadata, SearchRequest, SearchResponse,
};

#[cfg(test)]
mod edges_tests;
#[cfg(test)]
mod execute_tests;
#[cfg(test)]
mod tests;
