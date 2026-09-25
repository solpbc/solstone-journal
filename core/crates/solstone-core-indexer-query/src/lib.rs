// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Compilation and read-only execution of journal full-text queries.
//!
//! `load_entity_network`, `load_network_overview`, `load_edge_evidence`, and
//! `open_edges_reader` are a separate relationship-edge surface and are not
//! bounded by this chunk-query claim.

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
    EdgeQueryError, EntityTypeLookup, EvidenceRow, KindSummary, NETWORK_EVIDENCE_LIMIT_MAX,
    NETWORK_NEIGHBOR_LIMIT_MAX, NetworkNeighbor, NetworkOverviewRequest, NetworkOverviewResponse,
    NetworkRequest, NetworkResponse, OverviewEntity, OverviewTotals, is_safe_entity_id_component,
    load_edge_evidence, load_entity_network, load_network_overview, network_bound_detail,
    open_edges_reader, read_edge_repair_state,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use execute::QueryCounters;
pub use execute::{
    IndexedEntry, OwnerIndex, ResolvedCounts, SearchPlan, agents, coverage, hit_at,
    indexed_entity_ids, open_owner_index, read_indexed_entry, search, search_connection,
    search_counts, search_counts_connection,
};
pub use predicate::{EffectiveDateConstraint, PredicateInput, QueryPredicate};
pub use temporal::{TemporalExtraction, extract_temporal_references};
pub use types::{
    AdmittedCategory, ConnectionBoundary, ConnectionBoundaryError, ConnectionCorpusRefusal,
    ConnectionIndexDegraded, ConnectionScope, ConnectionSearchHit, ConnectionSearchRequest,
    ConnectionSearchResponse, ConnectionStartAfter, CountsResponse, CoverageResponse,
    CoverageState, IndexAccessError, IndexBuildCounts, IndexDegraded, NeedsOwner, Order,
    OwnerBoundary, QueryBoundary, SearchHit, SearchMetadata, SearchRequest, SearchResponse,
};

#[cfg(test)]
mod edges_tests;
#[cfg(test)]
mod execute_tests;
#[cfg(test)]
mod tests;
