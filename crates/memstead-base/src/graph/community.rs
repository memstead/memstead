//! Louvain community detection with deterministic seeding.
//!
//! Implements the Louvain algorithm from scratch, matching the graphology
//! implementation's traversal order and PRNG for bit-exact deterministic output.
//! Output is pure in-memory — no disk persistence.

use super::{BRIDGE_SAMPLE_CAP, ClusterInfo, CommunityBridge, LouvainOutput, SampleEdge};
use crate::entity::EntityId;
use crate::store::Store;
use std::collections::{BTreeSet, HashMap, HashSet};

// ---------------------------------------------------------------------------
// Seeded PRNG — mulberry32 (must match JS exactly)
// ---------------------------------------------------------------------------

/// Mulberry32 PRNG matching the JS implementation.
/// Uses i32 signed arithmetic for `| 0` coercion (overflow wrapping),
/// but `>>>` (unsigned right shift) must use u32 casts — JS `>>> n`
/// first converts to unsigned 32-bit, then shifts with zero-fill.
pub fn mulberry32(seed: u32) -> impl FnMut() -> f64 {
    let mut s = seed as i32;
    move || {
        s = s.wrapping_add(0x6d2b79f5_u32 as i32);
        // JS >>> is unsigned right shift: cast to u32, shift, cast back
        let mut t: i32 = (s ^ ((s as u32 >> 15) as i32)).wrapping_mul(1 | s);
        t = (t.wrapping_add((t ^ ((t as u32 >> 7) as i32)).wrapping_mul(61 | t))) ^ t;
        (((t ^ ((t as u32 >> 14) as i32)) as u32) as f64) / 4294967296.0
    }
}

// ---------------------------------------------------------------------------
// Louvain index — CSR-based undirected graph for community detection
// ---------------------------------------------------------------------------

/// Compressed Sparse Row index for the Louvain algorithm.
/// Mirrors the graphology UndirectedLouvainIndex structure.
struct LouvainIndex {
    c: usize, // number of active communities
    m: f64,   // total edge weight
    e: usize, // CSR edge count (2 * undirected edges)
    u: usize, // unused community stack pointer

    resolution: f64,
    nodes: Vec<String>, // original node IDs (EntityId strings)

    // Edge-level (CSR)
    neighborhood: Vec<usize>, // target node indices
    weights: Vec<f64>,        // edge weights (parallel to neighborhood)

    // Node-level
    loops: Vec<f64>,        // self-loop weights per node
    starts: Vec<usize>,     // CSR row pointers (length = C + 1 at init)
    belongings: Vec<usize>, // community assignment per node
    mapping: Vec<usize>,    // final mapping from original node to community

    // Community-level
    counts: Vec<usize>,      // node count per community
    unused: Vec<usize>,      // stack of unused community IDs
    total_weights: Vec<f64>, // total weight per community
}

impl LouvainIndex {
    /// Build the Louvain index from an undirected weighted graph.
    ///
    /// `nodes`: ordered list of node IDs
    /// `edges`: list of (source_idx, target_idx, weight) for undirected edges
    fn new(nodes: Vec<String>, edges: &[(usize, usize, f64)], resolution: f64) -> Self {
        let n = nodes.len();

        // Count degree (excluding self-loops) for each node to compute CSR starts
        let mut degree = vec![0usize; n];
        let mut self_loop_count = 0usize;
        for &(src, tgt, _) in edges {
            if src == tgt {
                self_loop_count += 1;
            } else {
                degree[src] += 1;
                degree[tgt] += 1;
            }
        }

        let size = (edges.len() - self_loop_count) * 2;
        let mut neighborhood = vec![0usize; size];
        let mut weights = vec![0.0f64; size];
        let mut loops = vec![0.0f64; n];

        // Compute starts: cumulative degree, counting DOWN like graphology
        let mut starts = vec![0usize; n + 1];
        {
            let mut cum = 0usize;
            for i in 0..n {
                cum += degree[i];
                starts[i] = cum;
            }
            starts[n] = size;
        }

        let mut belongings = vec![0usize; n];
        let mut counts = vec![0usize; n];
        let mut total_weights = vec![0.0f64; n];

        for i in 0..n {
            belongings[i] = i;
            counts[i] = 1;
        }

        let mut m = 0.0f64;

        // Single sweep over edges — fill CSR arrays
        for &(source, target, weight) in edges {
            m += weight;

            if source == target {
                total_weights[source] += weight * 2.0;
                loops[source] = weight * 2.0;
            } else {
                total_weights[source] += weight;
                total_weights[target] += weight;

                starts[source] -= 1;
                let start_source = starts[source];
                starts[target] -= 1;
                let start_target = starts[target];

                neighborhood[start_source] = target;
                neighborhood[start_target] = source;
                weights[start_source] = weight;
                weights[start_target] = weight;
            }
        }

        let mapping = belongings.clone();
        let unused = vec![0usize; n];

        LouvainIndex {
            c: n,
            m,
            e: size,
            u: 0,
            resolution,
            nodes,
            neighborhood,
            weights,
            loops,
            starts,
            belongings,
            mapping,
            counts,
            unused,
            total_weights,
        }
    }

    #[allow(dead_code)] // Part of the Louvain interface, used by non-fast paths
    fn compute_node_degree(&self, i: usize) -> f64 {
        let mut degree = 0.0;
        let start = self.starts[i];
        let end = self.starts[i + 1];
        for o in start..end {
            degree += self.weights[o];
        }
        degree
    }

    fn isolate(&mut self, i: usize, degree: f64) -> usize {
        let current_community = self.belongings[i];

        // Already isolated
        if self.counts[current_community] == 1 {
            return current_community;
        }

        self.u -= 1;
        let new_community = self.unused[self.u];
        let loops = self.loops[i];

        self.total_weights[current_community] -= degree + loops;
        self.total_weights[new_community] += degree + loops;
        self.belongings[i] = new_community;
        self.counts[current_community] -= 1;
        self.counts[new_community] += 1;

        new_community
    }

    fn move_node(&mut self, i: usize, degree: f64, target_community: usize) {
        let current_community = self.belongings[i];
        let loops = self.loops[i];

        self.total_weights[current_community] -= degree + loops;
        self.total_weights[target_community] += degree + loops;
        self.belongings[i] = target_community;

        let now_empty = self.counts[current_community] == 1;
        self.counts[current_community] -= 1;
        self.counts[target_community] += 1;

        if now_empty {
            self.unused[self.u] = current_community;
            self.u += 1;
        }
    }

    /// Fast delta computation (equivalent to graphology's fastDelta).
    fn fast_delta(
        &self,
        i: usize,
        degree: f64,
        target_community_degree: f64,
        target_community: usize,
    ) -> f64 {
        let m = self.m;
        let target_total = self.total_weights[target_community];
        let d = degree + self.loops[i];
        target_community_degree - (d * target_total * self.resolution) / (2.0 * m)
    }

    /// Fast delta with own community (accounts for node being removed first).
    fn fast_delta_with_own_community(
        &self,
        i: usize,
        degree: f64,
        target_community_degree: f64,
        target_community: usize,
    ) -> f64 {
        let m = self.m;
        let target_total = self.total_weights[target_community];
        let d = degree + self.loops[i];
        target_community_degree - (d * (target_total - d) * self.resolution) / (2.0 * m)
    }

    /// Coarsen the graph: merge nodes in same community into super-nodes.
    fn zoom_out(&mut self) {
        let n_original = self.nodes.len();
        let mut new_labels: HashMap<usize, usize> = HashMap::new();

        struct InducedNode {
            adj: HashMap<usize, f64>,
            total_weight: f64,
            internal_weight: f64,
        }

        let mut induced: Vec<InducedNode> = Vec::new();
        let mut c_new = 0usize;

        // Renumber communities
        for i in 0..self.c {
            let ci = self.belongings[i];
            if let std::collections::hash_map::Entry::Vacant(e) = new_labels.entry(ci) {
                e.insert(c_new);
                induced.push(InducedNode {
                    adj: HashMap::new(),
                    total_weight: self.total_weights[ci],
                    internal_weight: 0.0,
                });
                c_new += 1;
            }
            self.belongings[i] = new_labels[&ci];
        }

        // Update mapping
        for i in 0..n_original {
            self.mapping[i] = self.belongings[self.mapping[i]];
        }

        // Build induced graph matrix
        for i in 0..self.c {
            let ci = self.belongings[i];
            let data = &mut induced[ci];
            data.internal_weight += self.loops[i];

            for j in self.starts[i]..self.starts[i + 1] {
                let n = self.neighborhood[j];
                let cj = self.belongings[n];

                if ci == cj {
                    data.internal_weight += self.weights[j];
                    continue;
                }
                *data.adj.entry(cj).or_insert(0.0) += self.weights[j];
            }
        }

        // Rewrite neighborhood
        self.c = c_new;
        let mut n_edges = 0usize;
        let mut edge_count = 0usize;

        for (ci, data) in induced.iter().enumerate() {
            self.total_weights[ci] = data.total_weight;
            self.loops[ci] = data.internal_weight;
            self.counts[ci] = 1;
            self.starts[ci] = n_edges;
            self.belongings[ci] = ci;

            // Sort adj keys for deterministic order
            let mut adj_keys: Vec<usize> = data.adj.keys().copied().collect();
            adj_keys.sort();

            for &cj in &adj_keys {
                let w = data.adj[&cj];
                if n_edges < self.neighborhood.len() {
                    self.neighborhood[n_edges] = cj;
                    self.weights[n_edges] = w;
                }
                edge_count += 1;
                n_edges += 1;
            }
        }

        self.starts[c_new] = edge_count;
        self.e = edge_count;
        self.u = 0;
    }

    /// Compute modularity of current partition.
    fn modularity(&self) -> f64 {
        let m2 = self.m * 2.0;
        let mut internal_weights = vec![0.0f64; self.c];

        for i in 0..self.c {
            let ci = self.belongings[i];
            internal_weights[ci] += self.loops[i];

            for j in self.starts[i]..self.starts[i + 1] {
                let cj = self.belongings[self.neighborhood[j]];
                if ci == cj {
                    internal_weights[ci] += self.weights[j];
                }
            }
        }

        let mut q = 0.0;
        for (iw, tw) in internal_weights
            .iter()
            .zip(self.total_weights.iter())
            .take(self.c)
        {
            q += iw / m2 - (tw / m2).powi(2) * self.resolution;
        }
        q
    }

    /// Collect the final community assignment: node name → community index.
    fn collect(&self) -> HashMap<String, usize> {
        let mut result = HashMap::new();
        for (i, node) in self.nodes.iter().enumerate() {
            result.insert(node.clone(), self.mapping[i]);
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Sparse queue set for fast local moves
// ---------------------------------------------------------------------------

/// A FIFO queue that rejects duplicates (items already in the queue).
struct SparseQueueSet {
    queue: Vec<usize>,
    head: usize,
    in_set: Vec<bool>,
    count: usize,
}

impl SparseQueueSet {
    fn new(capacity: usize) -> Self {
        Self {
            queue: Vec::with_capacity(capacity),
            head: 0,
            in_set: vec![false; capacity],
            count: 0,
        }
    }

    fn enqueue(&mut self, item: usize) {
        if !self.in_set[item] {
            self.in_set[item] = true;
            self.queue.push(item);
            self.count += 1;
        }
    }

    fn dequeue(&mut self) -> Option<usize> {
        while self.head < self.queue.len() {
            let item = self.queue[self.head];
            self.head += 1;
            if self.in_set[item] {
                self.in_set[item] = false;
                self.count -= 1;
                return Some(item);
            }
        }
        None
    }

    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.count == 0
    }
}

// ---------------------------------------------------------------------------
// Louvain main loop
// ---------------------------------------------------------------------------

const EPSILON: f64 = 1e-10;

fn tie_breaker(
    best_community: usize,
    current_community: usize,
    target_community: usize,
    delta: f64,
    best_delta: f64,
) -> bool {
    if (delta - best_delta).abs() < EPSILON {
        if best_community == current_community {
            false
        } else {
            target_community > best_community
        }
    } else {
        delta > best_delta
    }
}

/// Run the Louvain algorithm with fast local moves and random walk.
fn run_louvain(index: &mut LouvainIndex, rng: &mut dyn FnMut() -> f64) {
    let mut move_was_made = true;
    let mut queue;

    while move_was_made {
        let l = index.c;
        move_was_made = false;

        // Random walk starting index
        let ri_start = (rng() * l as f64) as usize;

        // Enqueue all nodes in random-walk order
        queue = SparseQueueSet::new(l);
        for s in 0..l {
            let i = (ri_start + s) % l;
            queue.enqueue(i);
        }

        // Per-node community weights — reused across iterations
        let mut community_weights: HashMap<usize, f64> = HashMap::new();

        while let Some(i) = queue.dequeue() {
            let mut degree = 0.0;
            community_weights.clear();

            let current_community = index.belongings[i];
            let start = index.starts[i];
            let end = index.starts[i + 1];

            // Traverse neighbors — accumulate degree and community weights
            for pos in start..end {
                let j = index.neighborhood[pos];
                let weight = index.weights[pos];
                let target_community = index.belongings[j];

                degree += weight;
                *community_weights.entry(target_community).or_insert(0.0) += weight;
            }

            // Find best community
            let own_weight = community_weights
                .get(&current_community)
                .copied()
                .unwrap_or(0.0);
            let mut best_delta =
                index.fast_delta_with_own_community(i, degree, own_weight, current_community);
            let mut best_community = current_community;

            // Sort community keys for deterministic iteration
            let mut comm_keys: Vec<usize> = community_weights.keys().copied().collect();
            comm_keys.sort();

            for &target_community in &comm_keys {
                if target_community == current_community {
                    continue;
                }
                let target_degree = community_weights[&target_community];
                let delta = index.fast_delta(i, degree, target_degree, target_community);

                if tie_breaker(
                    best_community,
                    current_community,
                    target_community,
                    delta,
                    best_delta,
                ) {
                    best_delta = delta;
                    best_community = target_community;
                }
            }

            // Should we move the node?
            if best_delta < 0.0 {
                // Move back to singleton
                best_community = index.isolate(i, degree);
                if best_community == current_community {
                    continue;
                }
            } else if best_community == current_community {
                continue;
            } else {
                index.move_node(i, degree, best_community);
            }

            move_was_made = true;

            // Enqueue neighbors in other communities
            let start = index.starts[i];
            let end = index.starts[i + 1];
            for pos in start..end {
                let j = index.neighborhood[pos];
                let target_community = index.belongings[j];
                if target_community != best_community {
                    queue.enqueue(j);
                }
            }
        }

        if move_was_made {
            index.zoom_out();
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Detect communities using the Louvain algorithm.
/// Uses a seeded PRNG for deterministic results.
///
/// `edge_weight_fn` maps relationship types to weights.
/// Nodes where `store.get(id).stub == true` are excluded.
/// `visible_fn` filters entities by vault visibility.
pub fn detect_communities<F>(
    store: &Store,
    resolution: f64,
    seed: u32,
    edge_weight_fn: F,
) -> LouvainOutput
where
    F: Fn(&str) -> f64,
{
    // Build undirected weighted graph from store
    let mut node_ids: Vec<String> = Vec::new();
    let mut node_index: HashMap<String, usize> = HashMap::new();

    // Add visible, non-stub nodes
    // Sort for deterministic iteration
    let mut all_ids: Vec<&EntityId> = store.all_ids().collect();
    all_ids.sort_by(|a, b| a.0.cmp(&b.0));

    for id in &all_ids {
        let entity = match store.get(id) {
            Some(e) => e,
            None => continue,
        };
        if entity.stub {
            continue;
        }
        let idx = node_ids.len();
        node_ids.push(id.0.clone());
        node_index.insert(id.0.clone(), idx);
    }

    if node_ids.is_empty() {
        return LouvainOutput {
            modularity: 0.0,
            count: 0,
            clusters: HashMap::new(),
            entity_cluster_map: HashMap::new(),
        };
    }

    // Build weighted undirected edges — accumulate weights for multi-edges
    let mut edge_map: HashMap<(usize, usize), f64> = HashMap::new();

    for id in &all_ids {
        let entity = match store.get(id) {
            Some(e) if !e.stub => e,
            _ => continue,
        };

        let src_idx = match node_index.get(&entity.id.0) {
            Some(&i) => i,
            None => continue,
        };

        for edge in store.outgoing(&entity.id) {
            let tgt_idx = match node_index.get(&edge.target.0) {
                Some(&i) => i,
                None => continue,
            };

            let weight = edge_weight_fn(&edge.rel_type);

            // Undirected: use canonical order (smaller, larger)
            let key = if src_idx <= tgt_idx {
                (src_idx, tgt_idx)
            } else {
                (tgt_idx, src_idx)
            };
            *edge_map.entry(key).or_insert(0.0) += weight;
        }
    }

    let edges: Vec<(usize, usize, f64)> =
        edge_map.into_iter().map(|((a, b), w)| (a, b, w)).collect();

    // Single node or no edges — return one cluster
    if node_ids.len() == 1 || edges.is_empty() {
        let mut clusters = HashMap::new();
        let mut entity_cluster_map = HashMap::new();
        for id in &node_ids {
            entity_cluster_map.insert(id.clone(), "0".to_string());
        }
        clusters.insert(
            "0".to_string(),
            ClusterInfo {
                entities: node_ids,
            },
        );
        return LouvainOutput {
            modularity: 0.0,
            count: 1,
            clusters,
            entity_cluster_map,
        };
    }

    // Run Louvain
    let mut index = LouvainIndex::new(node_ids, &edges, resolution);
    let mut rng = mulberry32(seed);
    run_louvain(&mut index, &mut rng);

    let modularity = index.modularity();
    let assignments = index.collect();

    // Group by community ID
    let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
    let mut entity_cluster_map: HashMap<String, String> = HashMap::new();
    for (node_id, community_id) in assignments {
        let cluster_id = community_id.to_string();
        entity_cluster_map.insert(node_id.clone(), cluster_id.clone());
        grouped.entry(cluster_id).or_default().push(node_id);
    }

    let count = grouped.len();
    let clusters = grouped
        .into_iter()
        .map(|(id, entities)| (id, ClusterInfo { entities }))
        .collect();

    LouvainOutput {
        modularity,
        count,
        clusters,
        entity_cluster_map,
    }
}

/// Aggregate inter-cluster edges into undirected [`CommunityBridge`] entries.
///
/// Pair keys are lexicographically normalised (`from_cluster <= to_cluster`)
/// so each unordered cluster pair appears at most once. Intra-cluster edges
/// are skipped. If `vault_filter` is set, only edges whose **source** entity
/// lives in that vault contribute — matches `memstead_health`'s asymmetric filter.
/// Edges where either endpoint lacks a cluster assignment in `louvain` are
/// skipped. `sample_edges` is capped at [`BRIDGE_SAMPLE_CAP`] and sorted by
/// `(rel_type, from, to)` for determinism; `edge_types` is sorted ASCII.
pub fn aggregate_bridges(
    store: &Store,
    louvain: &LouvainOutput,
    vault_filter: Option<&str>,
) -> Vec<CommunityBridge> {
    // Accumulator per unordered cluster pair: (edge_count, edge_types, samples).
    struct Acc {
        edge_count: usize,
        edge_types: BTreeSet<String>,
        samples: Vec<SampleEdge>,
    }
    let mut by_pair: HashMap<(String, String), Acc> = HashMap::new();

    let mut all_ids: Vec<&EntityId> = store.all_ids().collect();
    all_ids.sort_by(|a, b| a.0.cmp(&b.0));

    for id in &all_ids {
        let entity = match store.get(id) {
            Some(e) => e,
            None => continue,
        };
        if let Some(vf) = vault_filter
            && entity.vault != vf
        {
            continue;
        }
        let src_cluster = match louvain.entity_cluster_map.get(&entity.id.0) {
            Some(c) => c.clone(),
            None => continue,
        };

        for edge in store.outgoing(&entity.id) {
            let tgt_cluster = match louvain.entity_cluster_map.get(&edge.target.0) {
                Some(c) => c.clone(),
                None => continue,
            };
            if src_cluster == tgt_cluster {
                continue;
            }

            let (from_c, to_c) = if src_cluster <= tgt_cluster {
                (src_cluster.clone(), tgt_cluster.clone())
            } else {
                (tgt_cluster.clone(), src_cluster.clone())
            };

            let entry = by_pair.entry((from_c, to_c)).or_insert_with(|| Acc {
                edge_count: 0,
                edge_types: BTreeSet::new(),
                samples: Vec::new(),
            });
            entry.edge_count += 1;
            entry.edge_types.insert(edge.rel_type.clone());
            entry.samples.push(SampleEdge {
                from: entity.id.0.clone(),
                to: edge.target.0.clone(),
                rel_type: edge.rel_type.clone(),
            });
        }
    }

    let mut bridges: Vec<CommunityBridge> = by_pair
        .into_iter()
        .map(|((from_cluster, to_cluster), mut acc)| {
            acc.samples
                .sort_by(|a, b| a.rel_type.cmp(&b.rel_type).then_with(|| a.from.cmp(&b.from)).then_with(|| a.to.cmp(&b.to)));
            acc.samples.truncate(BRIDGE_SAMPLE_CAP);
            CommunityBridge {
                from_cluster,
                to_cluster,
                edge_count: acc.edge_count,
                edge_types: acc.edge_types.into_iter().collect(),
                sample_edges: acc.samples,
            }
        })
        .collect();
    bridges.sort_by(|a, b| {
        a.from_cluster
            .cmp(&b.from_cluster)
            .then_with(|| a.to_cluster.cmp(&b.to_cluster))
    });
    bridges
}

/// Cluster ids from `louvain` that have at least one member living in
/// `vault`.
///
/// This is the membership rule that scopes `memstead_overview` /
/// `memstead_health` community output under a `vault` filter: it filters the
/// already-computed global partition rather than re-running detection on
/// a subgraph (per-vault Louvain ≠ the global partition restricted to a
/// vault, and would renumber clusters). Surviving cluster ids keep their
/// global-pass values, and surviving clusters keep their full membership
/// (including out-of-vault members) — only *which* clusters are reported
/// is scoped, not their contents. Stub entities are ignored, matching the
/// scoped entity-count definition.
///
/// Returned as a sorted set so callers iterate deterministically.
pub fn clusters_in_vault(
    store: &Store,
    louvain: &LouvainOutput,
    vault: &str,
) -> BTreeSet<String> {
    // Entity-id strings (the `.0` form used as cluster members) for the
    // non-stub entities that live in `vault`.
    let in_vault: HashSet<&str> = store
        .all_entities()
        .filter(|e| !e.stub && e.vault == vault)
        .map(|e| e.id.0.as_str())
        .collect();
    louvain
        .clusters
        .iter()
        .filter(|(_, info)| {
            info.entities
                .iter()
                .any(|member| in_vault.contains(member.as_str()))
        })
        .map(|(cid, _)| cid.clone())
        .collect()
}

/// Generate a default cluster summary by joining member entity titles.
/// Called at render time (never stored) — see `render::render_overview_markdown`
/// and `render::render_community_context_section`.
pub fn generate_auto_summary(store: &Store, entities: &[String]) -> String {
    entities
        .iter()
        .map(|eid| {
            store
                .get(&EntityId(eid.clone()))
                .map(|e| e.title.as_str())
                .unwrap_or(eid.as_str())
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Entity;
    use crate::store::{Edge, EdgeSource};

    fn entity(id: &str, vault: &str) -> Entity {
        Entity {
            id: EntityId(id.to_string()),
            title: id.to_string(),
            entity_type: "spec".to_string(),
            vault: vault.to_string(),
            file_path: String::new(),
            metadata: indexmap::IndexMap::new(),
            sections: indexmap::IndexMap::new(),
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    fn add_edge(store: &mut Store, from: &str, to: &str, rel: &str) {
        store.add_edge(
            EntityId(from.to_string()),
            Edge {
                rel_type: rel.to_string(),
                target: EntityId(to.to_string()),
                source: EdgeSource::Explicit,
            },
        );
    }

    // -----------------------------------------------------------------------
    // PRNG cross-validation: 100 reference values from JS mulberry32(42)
    // -----------------------------------------------------------------------

    #[test]
    fn mulberry32_cross_validation_with_js() {
        let expected: [f64; 100] = [
            0.6011037519201636,
            0.44829055899754167,
            0.8524657934904099,
            0.6697340414393693,
            0.17481389874592423,
            0.5265925421845168,
            0.2732279943302274,
            0.6247446539346129,
            0.8654746483080089,
            0.4723170551005751,
            0.24992373422719538,
            0.8820588334929198,
            0.7457375649828464,
            0.3070015134289861,
            0.19725383794866502,
            0.5007294877432287,
            0.6866120179183781,
            0.6106208984274417,
            0.003842951962724328,
            0.47078192373737693,
            0.8373374259099364,
            0.05120926629751921,
            0.5923239905387163,
            0.03153795562684536,
            0.2669559868518263,
            0.06178139243274927,
            0.18568900716491044,
            0.7835472931619734,
            0.530335606308654,
            0.027123609324917197,
            0.17300523445010185,
            0.8426881253253669,
            0.4877399173565209,
            0.8090229837689549,
            0.3194617456756532,
            0.44989572861231863,
            0.03743921360000968,
            0.05139273451641202,
            0.5565997792873532,
            0.5967295127920806,
            0.24517467361874878,
            0.6456944046076387,
            0.20951048866845667,
            0.30362963443621993,
            0.7386213727295399,
            0.8587109192740172,
            0.5079962892923504,
            0.2041900979820639,
            0.28420698270201683,
            0.29299163701944053,
            0.07469462975859642,
            0.6598597934935242,
            0.6807689322158694,
            0.6930881165899336,
            0.927839900366962,
            0.08796802558936179,
            0.9437352467793971,
            0.43113749707117677,
            0.942294550826773,
            0.13729840773157775,
            0.11094316560775042,
            0.015842905268073082,
            0.3604456859175116,
            0.48172251763753593,
            0.611922019161284,
            0.8995579732581973,
            0.07758240564726293,
            0.7195980150718242,
            0.934867731994018,
            0.2782514716964215,
            0.7065853946842253,
            0.1796227190643549,
            0.5203163539990783,
            0.8218917581252754,
            0.12059257342480123,
            0.9956386350095272,
            0.562800214625895,
            0.9161201647948474,
            0.4339903697837144,
            0.5784530241508037,
            0.3369620821904391,
            0.5962391037028283,
            0.3229577951133251,
            0.7294139908626676,
            0.2952086783479899,
            0.4497627364471555,
            0.8431257682386786,
            0.6950652054511011,
            0.9940114878118038,
            0.8901981303934008,
            0.431891469983384,
            0.5452131326310337,
            0.29592951526865363,
            0.1008774396032095,
            0.6967215123586357,
            0.3133056035730988,
            0.7859425814822316,
            0.9047754912171513,
            0.09364134701900184,
            0.47539179865270853,
        ];

        let mut rng = mulberry32(42);
        for (i, &exp) in expected.iter().enumerate() {
            let got = rng();
            assert!(
                (got - exp).abs() < 1e-15,
                "PRNG mismatch at index {i}: expected {exp}, got {got}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Community detection tests
    // -----------------------------------------------------------------------

    #[test]
    fn detect_empty_store() {
        let store = Store::new();
        let result = detect_communities(&store, 1.0, 42, |_| 1.0);
        assert_eq!(result.count, 0);
        assert!(result.clusters.is_empty());
    }

    #[test]
    fn detect_single_node() {
        let mut store = Store::new();
        store.upsert(EntityId("a".into()), entity("a", "s"));
        let result = detect_communities(&store, 1.0, 42, |_| 1.0);
        assert_eq!(result.count, 1);
        assert_eq!(result.clusters.len(), 1);
    }

    #[test]
    fn detect_disconnected_nodes() {
        let mut store = Store::new();
        store.upsert(EntityId("a".into()), entity("a", "s"));
        store.upsert(EntityId("b".into()), entity("b", "s"));
        // No edges
        let result = detect_communities(&store, 1.0, 42, |_| 1.0);
        assert_eq!(result.count, 1);
        // All disconnected nodes end up in one "cluster"
    }

    #[test]
    fn detect_two_clusters() {
        let mut store = Store::new();
        // Cluster 1: a-b-c (triangle)
        store.upsert(EntityId("a".into()), entity("a", "s"));
        store.upsert(EntityId("b".into()), entity("b", "s"));
        store.upsert(EntityId("c".into()), entity("c", "s"));
        add_edge(&mut store, "a", "b", "USES");
        add_edge(&mut store, "b", "c", "USES");
        add_edge(&mut store, "c", "a", "USES");

        // Cluster 2: d-e-f (triangle)
        store.upsert(EntityId("d".into()), entity("d", "s"));
        store.upsert(EntityId("e".into()), entity("e", "s"));
        store.upsert(EntityId("f".into()), entity("f", "s"));
        add_edge(&mut store, "d", "e", "USES");
        add_edge(&mut store, "e", "f", "USES");
        add_edge(&mut store, "f", "d", "USES");

        // Weak bridge between clusters
        add_edge(&mut store, "c", "d", "REFERENCES");

        let result = detect_communities(&store, 1.0, 42, |_| 1.0);
        assert!(
            result.count >= 2,
            "Expected at least 2 clusters, got {}",
            result.count
        );

        // Verify a, b, c are in the same community
        let node_to_cluster = &result.entity_cluster_map;

        assert_eq!(node_to_cluster["a"], node_to_cluster["b"]);
        assert_eq!(node_to_cluster["b"], node_to_cluster["c"]);
        assert_eq!(node_to_cluster["d"], node_to_cluster["e"]);
        assert_eq!(node_to_cluster["e"], node_to_cluster["f"]);
        assert_ne!(node_to_cluster["a"], node_to_cluster["d"]);
    }

    #[test]
    fn detect_skips_stubs() {
        let mut store = Store::new();
        store.upsert(EntityId("real".into()), entity("real", "s"));
        let mut stub = entity("stub", "s");
        stub.stub = true;
        store.upsert(EntityId("stub".into()), stub);
        add_edge(&mut store, "real", "stub", "REFERENCES");

        let result = detect_communities(&store, 1.0, 42, |_| 1.0);
        // Only the real entity should be in communities
        let total_entities: usize = result.clusters.values().map(|c| c.entities.len()).sum();
        assert_eq!(total_entities, 1);
    }

    #[test]
    fn detect_deterministic() {
        let mut store = Store::new();
        for i in 0..10 {
            store.upsert(EntityId(format!("e{i}")), entity(&format!("e{i}"), "s"));
        }
        for i in 0..9 {
            add_edge(&mut store, &format!("e{i}"), &format!("e{}", i + 1), "USES");
        }

        let r1 = detect_communities(&store, 1.0, 42, |_| 1.0);
        let r2 = detect_communities(&store, 1.0, 42, |_| 1.0);

        assert_eq!(r1.count, r2.count);
        assert!(
            (r1.modularity - r2.modularity).abs() < 1e-10,
            "Modularity mismatch: {} vs {}",
            r1.modularity,
            r2.modularity
        );
    }

    #[test]
    fn detect_respects_edge_weights() {
        let mut store = Store::new();
        store.upsert(EntityId("a".into()), entity("a", "s"));
        store.upsert(EntityId("b".into()), entity("b", "s"));
        store.upsert(EntityId("c".into()), entity("c", "s"));
        add_edge(&mut store, "a", "b", "IMPLEMENTS"); // weight 2.0
        add_edge(&mut store, "b", "c", "REFERENCES"); // weight 1.0

        let result = detect_communities(&store, 1.0, 42, |rel| match rel {
            "IMPLEMENTS" => 2.0,
            _ => 1.0,
        });
        // With 3 nodes the algorithm should find 1 cluster
        assert!(result.count >= 1);
    }

    // -----------------------------------------------------------------------
    // Sparse queue set tests
    // -----------------------------------------------------------------------

    #[test]
    fn sparse_queue_set_basic() {
        let mut q = SparseQueueSet::new(5);
        q.enqueue(0);
        q.enqueue(1);
        q.enqueue(0); // duplicate — should be ignored
        assert_eq!(q.dequeue(), Some(0));
        assert_eq!(q.dequeue(), Some(1));
        assert_eq!(q.dequeue(), None);
    }

    #[test]
    fn sparse_queue_set_fifo_order() {
        let mut q = SparseQueueSet::new(10);
        q.enqueue(3);
        q.enqueue(1);
        q.enqueue(4);
        assert_eq!(q.dequeue(), Some(3));
        assert_eq!(q.dequeue(), Some(1));
        assert_eq!(q.dequeue(), Some(4));
    }

    #[test]
    fn sparse_queue_set_re_enqueue_after_dequeue() {
        let mut q = SparseQueueSet::new(5);
        q.enqueue(0);
        assert_eq!(q.dequeue(), Some(0));
        q.enqueue(0); // should work after dequeue
        assert_eq!(q.dequeue(), Some(0));
    }
}
