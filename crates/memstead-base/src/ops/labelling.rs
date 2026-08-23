//! Grounded labelling over a schema-declared attack set — the one
//! argumentation-semantics computation that is parameter-free,
//! unique, polynomial, and explainable by construction: unattacked
//! entities are `accepted`, whatever an accepted entity attacks is
//! `defeated`, entities whose attackers are all defeated are
//! `accepted`, the rest stay `undecided`.
//!
//! A label is a reported observation with its evidence — never a
//! stored value, never a write gate, never a status. The labelling is
//! deliberately support-blind: it walks attack edges only, and a
//! defeated supporter never flips what it supports; the chain-shape
//! statistics give the reader that fact as a count instead.

use std::collections::{BTreeMap, HashMap};

use crate::entity::EntityId;
use crate::store::Store;
use memstead_schema::{LabellingDef, ReachDirection, SupportWalk};

/// The grounded label of one entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    Accepted,
    Defeated,
    Undecided,
}

impl Label {
    pub fn wire(&self) -> &'static str {
        match self {
            Label::Accepted => "accepted",
            Label::Defeated => "defeated",
            Label::Undecided => "undecided",
        }
    }
}

/// One mem's grounded labelling — the least fixpoint over the pinned
/// graph (non-stub entities of the mem; attack-set edges whose
/// endpoints are both non-stub nodes of the mem). Deterministic:
/// BTreeMaps keyed by id string.
#[derive(Debug, Clone)]
pub struct MemLabelling {
    /// Label per non-stub entity id of the mem.
    pub labels: BTreeMap<String, Label>,
    /// Direct in-mem attackers per entity id, sorted.
    pub attackers: BTreeMap<String, Vec<String>>,
    /// Attack-set edges incident to this mem's nodes whose other
    /// endpoint lives in another mem — excluded from the computation
    /// and counted, never guessed.
    pub cross_mem_edges_excluded: usize,
}

impl MemLabelling {
    /// The accepted direct attackers of `id` — the evidence a
    /// `defeated` label always carries.
    pub fn accepted_attackers_of(&self, id: &str) -> Vec<String> {
        self.direct_attackers_with(id, Label::Accepted)
    }

    /// The undecided direct attackers of `id` — the open attacker set
    /// that keeps an `undecided` label open.
    pub fn undecided_attackers_of(&self, id: &str) -> Vec<String> {
        self.direct_attackers_with(id, Label::Undecided)
    }

    fn direct_attackers_with(&self, id: &str, label: Label) -> Vec<String> {
        self.attackers
            .get(id)
            .map(|atts| {
                atts.iter()
                    .filter(|a| self.labels.get(a.as_str()) == Some(&label))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Compute one mem's grounded labelling over the declared attack set.
pub fn compute_mem_labelling(store: &Store, mem: &str, attack: &[String]) -> MemLabelling {
    // The pinned node set: every non-stub entity of the mem.
    let mut node_ids: Vec<String> = store
        .all_entities()
        .filter(|e| e.mem == mem && !e.stub)
        .map(|e| e.id.0.clone())
        .collect();
    node_ids.sort();
    let node_set: std::collections::HashSet<&str> = node_ids.iter().map(String::as_str).collect();

    // Direct attackers per node (attack edges INTO the node), and the
    // cross-mem exclusion count over incident attack edges in both
    // directions. Stub endpoints drop the edge silently (a stub has
    // no mem-internal standing); cross-mem endpoints are counted.
    let mut attackers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut cross_mem_edges_excluded = 0usize;
    for id_str in &node_ids {
        let id = EntityId(id_str.clone());
        let mut atts: Vec<String> = Vec::new();
        for edge in store.incoming(&id) {
            if !attack.iter().any(|n| n == &edge.rel_type) {
                continue;
            }
            if edge.from.mem() != mem {
                cross_mem_edges_excluded += 1;
                continue;
            }
            if node_set.contains(edge.from.0.as_str()) {
                atts.push(edge.from.0.clone());
            }
        }
        for edge in store.outgoing(&id) {
            if !attack.iter().any(|n| n == &edge.rel_type) {
                continue;
            }
            if edge.target.mem() != mem {
                cross_mem_edges_excluded += 1;
            }
        }
        atts.sort();
        atts.dedup();
        attackers.insert(id_str.clone(), atts);
    }

    // Least fixpoint: a node whose attackers are all Defeated becomes
    // Accepted (vacuously true for unattacked nodes); a node with an
    // Accepted attacker becomes Defeated; iterate to fixpoint; the
    // rest stay Undecided.
    let mut labels: HashMap<&str, Label> = HashMap::new();
    loop {
        let mut changed = false;
        for id in &node_ids {
            if labels.contains_key(id.as_str()) {
                continue;
            }
            let atts = &attackers[id.as_str()];
            if atts
                .iter()
                .all(|a| labels.get(a.as_str()) == Some(&Label::Defeated))
            {
                labels.insert(id.as_str(), Label::Accepted);
                changed = true;
            } else if atts
                .iter()
                .any(|a| labels.get(a.as_str()) == Some(&Label::Accepted))
            {
                labels.insert(id.as_str(), Label::Defeated);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let labels: BTreeMap<String, Label> = node_ids
        .iter()
        .map(|id| {
            (
                id.clone(),
                labels.get(id.as_str()).copied().unwrap_or(Label::Undecided),
            )
        })
        .collect();

    MemLabelling {
        labels,
        attackers,
        cross_mem_edges_excluded,
    }
}

/// The chain-shape statistics over one entity's support subtree —
/// the adversarial-shape indicators (an unusually deep chain with no
/// failing leaves warrants scrutiny). The engine serves numbers, the
/// reader judges.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeStats {
    /// Longest observed level of the visited-set-bounded breadth-first
    /// walk (exact longest path on tree-shaped support).
    pub depth: u64,
    /// Mean number of support successors over the walked nodes that
    /// have any (0.0 when none do).
    pub branching: f64,
    /// Leaves of a terminal type over all leaves — `None` when the
    /// subtree has no leaves (an isolated entity, or a pure cycle).
    pub terminal_share: Option<f64>,
    /// Subtree nodes (excluding the entity) labelled `defeated` by
    /// their own mem's labelling.
    pub defeated_in_support: u64,
    /// Subtree nodes (excluding the entity) labelled `undecided`.
    pub undecided_in_support: u64,
}

/// Walk the support subtree from `start` and compute the shape
/// statistics. `label_of` resolves a subtree node's label (nodes of
/// mems without a labelling declaration resolve to `None` and count
/// toward neither label count).
pub fn compute_shape(
    store: &Store,
    start: &EntityId,
    walk: &SupportWalk,
    label_of: &dyn Fn(&EntityId) -> Option<Label>,
) -> ShapeStats {
    let successors = |id: &EntityId| -> Vec<EntityId> {
        let mut next: Vec<EntityId> = match walk.direction {
            ReachDirection::Out => store
                .outgoing(id)
                .iter()
                .filter(|e| walk.relationships.iter().any(|n| n == &e.rel_type))
                .map(|e| e.target.clone())
                .collect(),
            ReachDirection::In => store
                .incoming(id)
                .iter()
                .filter(|e| walk.relationships.iter().any(|n| n == &e.rel_type))
                .map(|e| e.from.clone())
                .collect(),
        };
        next.sort_by(|a, b| a.0.cmp(&b.0));
        next.dedup();
        next
    };

    // Visited-set-bounded BFS from the start; the subtree is every
    // node reached (the start excluded from all counts).
    let mut visited: std::collections::HashSet<EntityId> = std::iter::once(start.clone()).collect();
    let mut frontier = vec![start.clone()];
    let mut depth: u64 = 0;
    let mut subtree: Vec<EntityId> = Vec::new();
    let mut successor_counts: Vec<usize> = Vec::new();
    // The start's own successor count participates in branching.
    let start_succ = successors(start).len();
    if start_succ > 0 {
        successor_counts.push(start_succ);
    }
    while !frontier.is_empty() {
        let mut next_frontier = Vec::new();
        for current in frontier {
            for next in successors(&current) {
                if visited.insert(next.clone()) {
                    subtree.push(next.clone());
                    next_frontier.push(next);
                }
            }
        }
        if !next_frontier.is_empty() {
            depth += 1;
        }
        frontier = next_frontier;
    }

    let mut leaves_total = 0u64;
    let mut leaves_terminal = 0u64;
    let mut defeated_in_support = 0u64;
    let mut undecided_in_support = 0u64;
    for node in &subtree {
        let succ = successors(node);
        if succ.is_empty() {
            leaves_total += 1;
            if store
                .get(node)
                .is_some_and(|e| !e.stub && walk.terminal_types.iter().any(|t| t == &e.entity_type))
            {
                leaves_terminal += 1;
            }
        } else {
            successor_counts.push(succ.len());
        }
        match label_of(node) {
            Some(Label::Defeated) => defeated_in_support += 1,
            Some(Label::Undecided) => undecided_in_support += 1,
            _ => {}
        }
    }

    let branching = if successor_counts.is_empty() {
        0.0
    } else {
        successor_counts.iter().sum::<usize>() as f64 / successor_counts.len() as f64
    };
    let terminal_share = if leaves_total == 0 {
        None
    } else {
        Some(leaves_terminal as f64 / leaves_total as f64)
    };

    ShapeStats {
        depth,
        branching,
        terminal_share,
        defeated_in_support,
        undecided_in_support,
    }
}

/// One entity's served labelling view — label, evidence, and the
/// optional shape block.
#[derive(Debug, Clone)]
pub struct LabellingView {
    pub label: Label,
    pub defeated_by: Vec<String>,
    pub undecided_by: Vec<String>,
    pub shape: Option<ShapeStats>,
}

impl LabellingView {
    /// The structured-envelope form:
    /// `{label, defeated_by, undecided_by, shape?}`.
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "label": self.label.wire(),
            "defeated_by": self.defeated_by,
            "undecided_by": self.undecided_by,
        });
        if let Some(shape) = &self.shape {
            v["shape"] = serde_json::json!({
                "depth": shape.depth,
                "branching": shape.branching,
                "terminal_share": shape.terminal_share,
                "defeated_in_support": shape.defeated_in_support,
                "undecided_in_support": shape.undecided_in_support,
            });
        }
        v
    }
}

/// Convenience: whether a schema's manifest declares labelling.
pub fn labelling_of(schema: &memstead_schema::Schema) -> Option<&LabellingDef> {
    schema.manifest.relationships.labelling.as_ref()
}
