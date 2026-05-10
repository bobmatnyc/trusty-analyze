//! Analysis primitives for trusty-analyzer.
//!
//! Operates on `trusty_common::CodeChunk` corpora fetched from the trusty-search
//! daemon over HTTP. No direct database access — the search daemon is the
//! authoritative source of chunk data.
//!
//! Modules:
//! - [`complexity`]: cyclomatic / cognitive complexity, code smell detection
//! - [`blame`]: temporal decay scoring (the search daemon does the actual
//!   `git log -L`; this crate just consumes the wire-format `ChunkBlame`)
//! - [`concept_cluster`]: k-means clustering helpers (label-only; no embedder
//!   dependency in this crate — callers supply embeddings)
//! - [`facts`]: redb-backed canonical fact store, owned by the analyzer
//! - [`client`]: HTTP client for fetching chunks/index summaries from
//!   trusty-search

pub mod blame;
pub mod client;
pub mod complexity;
pub mod complexity_ts;
pub mod concept_cluster;
pub mod facts;
pub mod linker;
pub mod quality;
pub mod registry;
pub mod scip;

pub use client::{IndexSummary, TrustySearchClient};
pub use complexity::compute_complexity_for;
pub use concept_cluster::{bow_embedding, cluster, ClusterResult, ConceptCluster};
pub use facts::FactStore;
pub use linker::link;
pub use registry::AnalyzerRegistry;
pub use scip::{extract_kg_from_scip, index_to_graph as scip_index_to_graph, ScipIngestSummary};

#[cfg(test)]
mod integration_tests;
#[cfg(test)]
pub(crate) mod test_utils;
