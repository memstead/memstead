//! Graph algorithms — BFS traversal, community detection, neighborhood queries.

pub mod community;
pub mod query;
pub mod relations;

use serde::Serialize;
use std::collections::HashMap;

/// Output of Louvain community detection: clusters, reverse lookup, and
/// quality metrics. Computed on demand by `Engine::communities()` and
/// cached in an in-memory memo that is invalidated whenever the graph
/// mutates (see `Engine::invalidate_communities`).
#[derive(Debug, Clone, Serialize)]
pub struct LouvainOutput {
    pub modularity: f64,
    pub count: usize,
    pub clusters: HashMap<String, ClusterInfo>,
    pub entity_cluster_map: HashMap<String, String>,
}

/// Info about a single community cluster.
#[derive(Debug, Clone, Serialize)]
pub struct ClusterInfo {
    pub entities: Vec<String>,
}

/// Inter-cluster edge aggregation. Pair keys are lexicographically normalised
/// (`from_cluster <= to_cluster`), so each unordered cluster pair appears at
/// most once. `sample_edges` carries the directed tuples as they exist in
/// the store, capped at [`BRIDGE_SAMPLE_CAP`].
#[derive(Debug, Clone, Serialize)]
pub struct CommunityBridge {
    pub from_cluster: String,
    pub to_cluster: String,
    pub edge_count: usize,
    pub edge_types: Vec<String>,
    pub sample_edges: Vec<SampleEdge>,
}

/// A directed sample edge for a [`CommunityBridge`].
#[derive(Debug, Clone, Serialize)]
pub struct SampleEdge {
    pub from: String,
    pub to: String,
    pub rel_type: String,
}

/// Maximum number of [`SampleEdge`] tuples attached to a [`CommunityBridge`].
pub const BRIDGE_SAMPLE_CAP: usize = 3;
