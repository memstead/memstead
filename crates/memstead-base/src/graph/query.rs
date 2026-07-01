//! BFS reachability search and orphan/stub detection.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::entity::EntityId;
use crate::store::{EdgeSource, InEdge, Store};

/// Find all entities reachable from a starting entity within max_depth hops.
/// Undirected traversal (follows both outgoing and incoming edges).
/// Returns the set of reachable entity IDs including the start node.
pub fn reachable(store: &Store, from: &EntityId, max_depth: usize) -> Vec<EntityId> {
    let mut visited = HashSet::new();
    visited.insert(from.clone());

    let mut queue: VecDeque<(EntityId, usize)> = VecDeque::new();
    queue.push_back((from.clone(), 0));

    while let Some((id, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }

        for edge in store.outgoing(&id) {
            if !visited.contains(&edge.target) {
                visited.insert(edge.target.clone());
                queue.push_back((edge.target.clone(), depth + 1));
            }
        }
        for edge in store.incoming(&id) {
            if !visited.contains(&edge.from) {
                visited.insert(edge.from.clone());
                queue.push_back((edge.from.clone(), depth + 1));
            }
        }
    }

    visited.into_iter().collect()
}

/// Like [`reachable`] but returns each reached entity's hop-distance from
/// `from` — the depth at which BFS first reaches it (`from` itself is 0).
/// Undirected (outgoing + incoming), same membership as [`reachable`]. The
/// distances drive proximity ranking of a `related_to` neighbourhood:
/// nearer first.
pub fn reachable_distances(
    store: &Store,
    from: &EntityId,
    max_depth: usize,
) -> HashMap<EntityId, usize> {
    let mut dist: HashMap<EntityId, usize> = HashMap::new();
    dist.insert(from.clone(), 0);

    let mut queue: VecDeque<(EntityId, usize)> = VecDeque::new();
    queue.push_back((from.clone(), 0));

    while let Some((id, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        for edge in store.outgoing(&id) {
            if !dist.contains_key(&edge.target) {
                dist.insert(edge.target.clone(), depth + 1);
                queue.push_back((edge.target.clone(), depth + 1));
            }
        }
        for edge in store.incoming(&id) {
            if !dist.contains_key(&edge.from) {
                dist.insert(edge.from.clone(), depth + 1);
                queue.push_back((edge.from.clone(), depth + 1));
            }
        }
    }
    dist
}

/// Find all entities reachable from `from` by walking only edges whose
/// `rel_type` is in `edge_types`, up to `max_depth` hops. Undirected
/// traversal (outgoing + incoming). Returns one tuple per reached entity
/// — never `from` itself — carrying the edge label used to first reach it
/// and the depth of that first reach (1 = direct neighbour).
///
/// `max_depth == 0` or an empty `edge_types` returns an empty vec.
///
/// This is the graph-expansion primitive behind `SearchScope.expand_via`.
/// Shares the BFS shape with `reachable` but filters by rel_type and
/// reports how each entity was reached.
pub fn reachable_via(
    store: &Store,
    from: &EntityId,
    edge_types: &[String],
    max_depth: usize,
) -> Vec<(EntityId, String, usize)> {
    if max_depth == 0 || edge_types.is_empty() {
        return Vec::new();
    }

    let mut visited: HashSet<EntityId> = HashSet::new();
    visited.insert(from.clone());

    let mut results: Vec<(EntityId, String, usize)> = Vec::new();
    let mut queue: VecDeque<(EntityId, usize)> = VecDeque::new();
    queue.push_back((from.clone(), 0));

    while let Some((id, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        for edge in store.outgoing(&id) {
            if !edge_types.iter().any(|t| t == &edge.rel_type) {
                continue;
            }
            if visited.insert(edge.target.clone()) {
                results.push((edge.target.clone(), edge.rel_type.clone(), depth + 1));
                queue.push_back((edge.target.clone(), depth + 1));
            }
        }
        for edge in store.incoming(&id) {
            if !edge_types.iter().any(|t| t == &edge.rel_type) {
                continue;
            }
            if visited.insert(edge.from.clone()) {
                results.push((edge.from.clone(), edge.rel_type.clone(), depth + 1));
                queue.push_back((edge.from.clone(), depth + 1));
            }
        }
    }

    results
}

/// Would adding an edge `from --rel_type--> to` close a cycle in the
/// subgraph restricted to edges of `rel_type`? Returns the back-path as
/// `[to, …, from]` when a cycle exists, `None` otherwise.
///
/// A self-loop (`from == to`) is a length-1 cycle and is reported without
/// a BFS. Otherwise this walks forward from `to` along outgoing edges
/// whose `rel_type` matches, looking for `from`. Cost is O(edges of that
/// rel_type) in the worst case.
pub fn would_cycle(
    store: &Store,
    from: &EntityId,
    to: &EntityId,
    rel_type: &str,
) -> Option<Vec<EntityId>> {
    if from == to {
        return Some(vec![from.clone()]);
    }

    let mut parent: std::collections::HashMap<EntityId, EntityId> = std::collections::HashMap::new();
    let mut visited: HashSet<EntityId> = HashSet::new();
    visited.insert(to.clone());

    let mut queue: VecDeque<EntityId> = VecDeque::new();
    queue.push_back(to.clone());

    while let Some(current) = queue.pop_front() {
        for edge in store.outgoing(&current) {
            if edge.rel_type != rel_type {
                continue;
            }
            let next = &edge.target;
            if *next == *from {
                let mut path = vec![from.clone(), current.clone()];
                let mut cursor = current;
                while let Some(p) = parent.get(&cursor) {
                    path.push(p.clone());
                    cursor = p.clone();
                }
                path.reverse();
                return Some(path);
            }
            if visited.insert(next.clone()) {
                parent.insert(next.clone(), current.clone());
                queue.push_back(next.clone());
            }
        }
    }
    None
}

/// Find orphan entities — non-stub entities with no edges at all (completely isolated).
pub fn find_orphans(store: &Store) -> Vec<EntityId> {
    let mut results = Vec::new();
    for entity in store.all_entities() {
        if entity.stub {
            continue;
        }
        let out = store.outgoing(&entity.id);
        let inc = store.incoming(&entity.id);
        if out.is_empty() && inc.is_empty() {
            results.push(entity.id.clone());
        }
    }
    results
}

/// Find stub entities — entities created from unresolved references.
/// Returns each stub with the list of entities that reference it.
pub fn find_stubs(store: &Store) -> Vec<(EntityId, Vec<EntityId>)> {
    let mut results = Vec::new();
    for entity in store.all_entities() {
        if !entity.stub {
            continue;
        }
        let referenced_by: Vec<EntityId> = store
            .incoming(&entity.id)
            .iter()
            .map(|e| e.from.clone())
            .collect();
        results.push((entity.id.clone(), referenced_by));
    }
    results
}

/// One entity's degree counts. `total == incoming + outgoing`; kept explicit
/// so the JSON wire shape is self-describing and callers don't re-derive it.
///
/// `typed_*` excludes auto-emitted mention edges (`EdgeSource::BodyLink` —
/// the `[[wiki-link]]` → REFERENCES alias-synthesis pass) so centrality can
/// rank by declared dependency rather than co-mention. Auto-emitted mentions
/// are the bulk of all edges, so `total` (which keeps them) is dominated by
/// co-mention; `typed_total` is the dependency degree. The raw `total` is
/// retained — the mention edges are not dropped from the graph, only set
/// aside for ranking. Mention degree is `total - typed_total`.
/// Stubs are never included in `most_connected` results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connectivity {
    pub id: EntityId,
    pub total: usize,
    pub incoming: usize,
    pub outgoing: usize,
    pub typed_total: usize,
    pub typed_incoming: usize,
    pub typed_outgoing: usize,
}

/// Compute one entity's raw and typed degree. `incoming_counts` decides
/// which incoming edges contribute (e.g. source-in-vault scoping for a
/// vault-filtered health view); every outgoing edge always counts. Typed
/// degree excludes `EdgeSource::BodyLink` (auto-emitted mention) edges.
pub fn connectivity_for(
    store: &Store,
    id: &EntityId,
    incoming_counts: impl Fn(&InEdge) -> bool,
) -> Connectivity {
    let out = store.outgoing(id);
    let outgoing = out.len();
    let typed_outgoing = out
        .iter()
        .filter(|e| e.source != EdgeSource::BodyLink)
        .count();

    let mut incoming = 0;
    let mut typed_incoming = 0;
    for e in store.incoming(id) {
        if !incoming_counts(e) {
            continue;
        }
        incoming += 1;
        if e.source != EdgeSource::BodyLink {
            typed_incoming += 1;
        }
    }

    Connectivity {
        id: id.clone(),
        total: outgoing + incoming,
        incoming,
        outgoing,
        typed_total: typed_outgoing + typed_incoming,
        typed_incoming,
        typed_outgoing,
    }
}

/// Centrality ordering: dependency degree (`typed_total`) descending first,
/// then raw `total` descending, then `id` lexicographic ascending as a
/// stable deterministic tie-break. Ranking by `typed_total` keeps a
/// co-mention-inflated hub from outranking a real dependency hub.
pub fn cmp_by_dependency(a: &Connectivity, b: &Connectivity) -> Ordering {
    b.typed_total
        .cmp(&a.typed_total)
        .then_with(|| b.total.cmp(&a.total))
        .then_with(|| a.id.0.cmp(&b.id.0))
}

/// Find the most connected non-stub entities, ranked by dependency degree
/// (typed edges) — see [`cmp_by_dependency`]. Returns up to `limit` entries.
pub fn most_connected(store: &Store, limit: usize) -> Vec<Connectivity> {
    let mut entries: Vec<Connectivity> = store
        .all_entities()
        .filter(|e| !e.stub)
        .map(|e| connectivity_for(store, &e.id, |_| true))
        .collect();

    entries.sort_by(cmp_by_dependency);
    entries.truncate(limit);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Entity;
    use crate::store::{Edge, EdgeSource};
    use indexmap::IndexMap;

    fn entity(id: &str, vault: &str, stub: bool) -> Entity {
        Entity {
            id: EntityId(id.to_string()),
            title: id.to_string(),
            entity_type: "spec".to_string(),
            vault: vault.to_string(),
            file_path: String::new(),
            metadata: IndexMap::new(),
            sections: IndexMap::new(),
            relationships: Vec::new(),
            content_hash: String::new(),
            stub,
            stub_kind: if stub {
                Some(crate::entity::StubKind::LoadTime)
            } else {
                None
            },
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

    /// An auto-emitted mention edge (the `[[wiki-link]]` → REFERENCES
    /// alias-synthesis pass), marked `EdgeSource::BodyLink` — excluded
    /// from the typed (dependency) degree.
    fn add_body_edge(store: &mut Store, from: &str, to: &str) {
        store.add_edge(
            EntityId(from.to_string()),
            Edge {
                rel_type: "REFERENCES".to_string(),
                target: EntityId(to.to_string()),
                source: EdgeSource::BodyLink,
            },
        );
    }

    fn build_linear_store() -> Store {
        // A -> B -> C
        let mut store = Store::new();
        store.upsert(EntityId("a".into()), entity("a", "s", false));
        store.upsert(EntityId("b".into()), entity("b", "s", false));
        store.upsert(EntityId("c".into()), entity("c", "s", false));
        add_edge(&mut store, "a", "b", "USES");
        add_edge(&mut store, "b", "c", "USES");
        store
    }

    #[test]
    fn reachable_within_depth() {
        let store = build_linear_store();
        let a = EntityId("a".into());

        let r0 = reachable(&store, &a, 0);
        assert_eq!(r0.len(), 1); // just self

        let r1 = reachable(&store, &a, 1);
        assert_eq!(r1.len(), 2); // a + b

        let r2 = reachable(&store, &a, 2);
        assert_eq!(r2.len(), 3); // a + b + c
    }

    #[test]
    fn reachable_undirected() {
        let store = build_linear_store();
        let c = EntityId("c".into());
        // From C, going backwards via incoming edges
        let r = reachable(&store, &c, 10);
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn find_orphans_isolated_node() {
        let mut store = Store::new();
        store.upsert(EntityId("a".into()), entity("a", "s", false));
        store.upsert(EntityId("b".into()), entity("b", "s", false));
        add_edge(&mut store, "a", "b", "USES");
        store.upsert(EntityId("c".into()), entity("c", "s", false));
        // c has no edges

        let orphans = find_orphans(&store);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0], EntityId("c".into()));
    }

    #[test]
    fn find_orphans_skips_stubs() {
        let mut store = Store::new();
        store.upsert(EntityId("a".into()), entity("a", "s", true)); // stub, isolated
        store.upsert(EntityId("b".into()), entity("b", "s", false)); // non-stub, isolated

        let orphans = find_orphans(&store);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0], EntityId("b".into()));
    }

    #[test]
    fn find_stubs_returns_stub_entities() {
        let mut store = Store::new();
        store.upsert(EntityId("real".into()), entity("real", "s", false));
        store.upsert(EntityId("stub1".into()), entity("stub1", "s", true));
        add_edge(&mut store, "real", "stub1", "REFERENCES");

        let stubs = find_stubs(&store);
        assert_eq!(stubs.len(), 1);
        assert_eq!(stubs[0].0, EntityId("stub1".into()));
        assert_eq!(stubs[0].1, vec![EntityId("real".into())]);
    }

    #[test]
    fn most_connected_sorted_descending() {
        let mut store = Store::new();
        store.upsert(EntityId("a".into()), entity("a", "s", false));
        store.upsert(EntityId("b".into()), entity("b", "s", false));
        store.upsert(EntityId("c".into()), entity("c", "s", false));
        // a has 2 edges (1 out + 1 in from c->a)
        // b has 1 edge (1 in from a->b)
        // c has 1 edge (1 out to a)
        add_edge(&mut store, "a", "b", "USES");
        add_edge(&mut store, "c", "a", "PART_OF");

        let top = most_connected(&store, 10);
        assert_eq!(top[0].id, EntityId("a".into()));
        assert_eq!(top[0].total, 2);
        assert_eq!(top[0].incoming, 1);
        assert_eq!(top[0].outgoing, 1);
    }

    #[test]
    fn most_connected_respects_limit() {
        let mut store = Store::new();
        for i in 0..5 {
            store.upsert(
                EntityId(format!("e{i}")),
                entity(&format!("e{i}"), "s", false),
            );
        }
        let top = most_connected(&store, 2);
        assert_eq!(top.len(), 2);
    }

    // ---- reachable_via ----

    #[test]
    fn reachable_via_filters_by_edge_type() {
        // a --USES--> b ; a --REFERENCES--> c
        let mut store = Store::new();
        store.upsert(EntityId("a".into()), entity("a", "s", false));
        store.upsert(EntityId("b".into()), entity("b", "s", false));
        store.upsert(EntityId("c".into()), entity("c", "s", false));
        add_edge(&mut store, "a", "b", "USES");
        add_edge(&mut store, "a", "c", "REFERENCES");

        let r = reachable_via(&store, &EntityId("a".into()), &["USES".to_string()], 1);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, EntityId("b".into()));
        assert_eq!(r[0].1, "USES");
        assert_eq!(r[0].2, 1);
    }

    #[test]
    fn reachable_via_bidirectional() {
        // From b, walk back to a via incoming edge.
        let mut store = Store::new();
        store.upsert(EntityId("a".into()), entity("a", "s", false));
        store.upsert(EntityId("b".into()), entity("b", "s", false));
        add_edge(&mut store, "a", "b", "USES");
        let r = reachable_via(&store, &EntityId("b".into()), &["USES".to_string()], 1);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, EntityId("a".into()));
        assert_eq!(r[0].2, 1);
    }

    #[test]
    fn reachable_via_zero_depth_empty() {
        let store = build_linear_store();
        let r = reachable_via(&store, &EntityId("a".into()), &["USES".to_string()], 0);
        assert!(r.is_empty());
    }

    #[test]
    fn reachable_via_empty_edge_types_empty() {
        let store = build_linear_store();
        let r = reachable_via(&store, &EntityId("a".into()), &[], 10);
        assert!(r.is_empty());
    }

    #[test]
    fn reachable_via_respects_depth_limit() {
        let store = build_linear_store(); // a -> b -> c with USES
        let r1 = reachable_via(&store, &EntityId("a".into()), &["USES".to_string()], 1);
        assert_eq!(r1.len(), 1, "depth 1 reaches b only");
        assert_eq!(r1[0].0, EntityId("b".into()));
        assert_eq!(r1[0].2, 1);

        let r2 = reachable_via(&store, &EntityId("a".into()), &["USES".to_string()], 2);
        assert_eq!(r2.len(), 2);
        let depths: std::collections::HashMap<EntityId, usize> =
            r2.iter().map(|(id, _, d)| (id.clone(), *d)).collect();
        assert_eq!(depths[&EntityId("b".into())], 1);
        assert_eq!(depths[&EntityId("c".into())], 2);
    }

    #[test]
    fn reachable_via_bfs_records_shortest_depth() {
        // Diamond: a -> b -> d ; a -> c -> d. d is reachable via 2 hops from a
        // through two paths. BFS should record depth=2 exactly once.
        let mut store = Store::new();
        for id in ["a", "b", "c", "d"] {
            store.upsert(EntityId(id.into()), entity(id, "s", false));
        }
        add_edge(&mut store, "a", "b", "R");
        add_edge(&mut store, "a", "c", "R");
        add_edge(&mut store, "b", "d", "R");
        add_edge(&mut store, "c", "d", "R");

        let r = reachable_via(&store, &EntityId("a".into()), &["R".to_string()], 3);
        let entries: std::collections::HashMap<EntityId, usize> =
            r.iter().map(|(id, _, d)| (id.clone(), *d)).collect();
        assert_eq!(entries.len(), 3, "b, c, d each appear once");
        assert_eq!(entries[&EntityId("d".into())], 2);
    }

    #[test]
    fn most_connected_skips_stubs() {
        let mut store = Store::new();
        store.upsert(EntityId("real".into()), entity("real", "s", false));
        store.upsert(EntityId("stub".into()), entity("stub", "s", true));
        add_edge(&mut store, "real", "stub", "REFERENCES");

        let top = most_connected(&store, 10);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].id, EntityId("real".into()));
    }

    // ---- would_cycle ----

    #[test]
    fn would_cycle_self_loop_always_reported() {
        let mut store = Store::new();
        store.upsert(EntityId("a".into()), entity("a", "s", false));
        let path = would_cycle(&store, &EntityId("a".into()), &EntityId("a".into()), "PART_OF");
        assert_eq!(path, Some(vec![EntityId("a".into())]));
    }

    #[test]
    fn would_cycle_single_back_edge() {
        // a -PART_OF-> b already. Adding b -PART_OF-> a closes a cycle.
        let mut store = Store::new();
        store.upsert(EntityId("a".into()), entity("a", "s", false));
        store.upsert(EntityId("b".into()), entity("b", "s", false));
        add_edge(&mut store, "a", "b", "PART_OF");
        let path = would_cycle(&store, &EntityId("b".into()), &EntityId("a".into()), "PART_OF")
            .expect("cycle");
        assert_eq!(path, vec![EntityId("a".into()), EntityId("b".into())]);
    }

    #[test]
    fn would_cycle_deep_chain() {
        // foo's future edge: foo -PART_OF-> bar. Existing: bar->baz->foo.
        let mut store = Store::new();
        for id in ["foo", "bar", "baz"] {
            store.upsert(EntityId(id.into()), entity(id, "s", false));
        }
        add_edge(&mut store, "bar", "baz", "PART_OF");
        add_edge(&mut store, "baz", "foo", "PART_OF");
        let path = would_cycle(
            &store,
            &EntityId("foo".into()),
            &EntityId("bar".into()),
            "PART_OF",
        )
        .expect("cycle");
        assert_eq!(
            path,
            vec![
                EntityId("bar".into()),
                EntityId("baz".into()),
                EntityId("foo".into())
            ]
        );
    }

    #[test]
    fn would_cycle_ignores_other_rel_types() {
        // a -DEPENDS_ON-> b exists. Proposed b -PART_OF-> a should not
        // trip the PART_OF subgraph even though a non-PART_OF back-edge
        // exists.
        let mut store = Store::new();
        store.upsert(EntityId("a".into()), entity("a", "s", false));
        store.upsert(EntityId("b".into()), entity("b", "s", false));
        add_edge(&mut store, "a", "b", "DEPENDS_ON");
        assert!(
            would_cycle(&store, &EntityId("b".into()), &EntityId("a".into()), "PART_OF")
                .is_none()
        );
    }

    #[test]
    fn would_cycle_none_for_disjoint_graph() {
        let mut store = Store::new();
        for id in ["a", "b", "c", "d"] {
            store.upsert(EntityId(id.into()), entity(id, "s", false));
        }
        add_edge(&mut store, "c", "d", "PART_OF");
        assert!(
            would_cycle(&store, &EntityId("a".into()), &EntityId("b".into()), "PART_OF")
                .is_none()
        );
    }

    #[test]
    fn would_cycle_parallel_paths_do_not_trip() {
        // a -PART_OF-> b and a -PART_OF-> c (no path from b to a).
        // Proposed c -PART_OF-> a should be flagged (c has no back-path
        // today, but adding it alongside existing a->c would form a
        // cycle a->c->a — confirm the BFS catches that).
        let mut store = Store::new();
        for id in ["a", "b", "c"] {
            store.upsert(EntityId(id.into()), entity(id, "s", false));
        }
        add_edge(&mut store, "a", "b", "PART_OF");
        add_edge(&mut store, "a", "c", "PART_OF");
        // Proposed a -PART_OF-> b is fine — a already -PART_OF-> b.
        assert!(
            would_cycle(&store, &EntityId("a".into()), &EntityId("b".into()), "PART_OF")
                .is_none(),
            "sibling paths must not trip"
        );
        // Proposed b -PART_OF-> a would close a cycle a->b->a.
        assert!(
            would_cycle(&store, &EntityId("b".into()), &EntityId("a".into()), "PART_OF")
                .is_some()
        );
    }

    #[test]
    fn most_connected_distinguishes_hub_vs_fanout() {
        let mut store = Store::new();
        for id in ["hub", "fanout", "r1", "r2", "r3", "r4", "t1", "t2", "t3", "t4"] {
            store.upsert(EntityId(id.into()), entity(id, "s", false));
        }
        // hub: 4 incoming, 0 outgoing
        add_edge(&mut store, "r1", "hub", "REFERENCES");
        add_edge(&mut store, "r2", "hub", "REFERENCES");
        add_edge(&mut store, "r3", "hub", "REFERENCES");
        add_edge(&mut store, "r4", "hub", "REFERENCES");
        // fanout: 0 incoming, 4 outgoing
        add_edge(&mut store, "fanout", "t1", "USES");
        add_edge(&mut store, "fanout", "t2", "USES");
        add_edge(&mut store, "fanout", "t3", "USES");
        add_edge(&mut store, "fanout", "t4", "USES");

        let top = most_connected(&store, 10);
        let hub = top.iter().find(|c| c.id == EntityId("hub".into())).unwrap();
        assert_eq!(hub.total, 4);
        assert_eq!(hub.incoming, 4);
        assert_eq!(hub.outgoing, 0);
        let fanout = top
            .iter()
            .find(|c| c.id == EntityId("fanout".into()))
            .unwrap();
        assert_eq!(fanout.total, 4);
        assert_eq!(fanout.incoming, 0);
        assert_eq!(fanout.outgoing, 4);

        // Tie-break: "fanout" < "hub" lex, so fanout appears first.
        let fanout_pos = top.iter().position(|c| c.id.0 == "fanout").unwrap();
        let hub_pos = top.iter().position(|c| c.id.0 == "hub").unwrap();
        assert!(
            fanout_pos < hub_pos,
            "ties must resolve by id lex ascending"
        );
    }

    /// #46: a node inflated purely by auto-emitted mentions (BodyLink)
    /// must not outrank a node with real typed dependencies. `typed_total`
    /// drives the ranking; `total` (which keeps the mentions) is retained
    /// but only a secondary tie-break.
    #[test]
    fn most_connected_ranks_by_dependency_not_mention() {
        let mut store = Store::new();
        for id in [
            "mentionhub", "dephub", "m1", "m2", "m3", "m4", "m5", "d1", "d2",
        ] {
            store.upsert(EntityId(id.into()), entity(id, "s", false));
        }
        // mentionhub: 5 incoming mention edges — high total, zero typed.
        for m in ["m1", "m2", "m3", "m4", "m5"] {
            add_body_edge(&mut store, m, "mentionhub");
        }
        // dephub: 2 incoming typed (USES) edges — lower total, real deps.
        add_edge(&mut store, "d1", "dephub", "USES");
        add_edge(&mut store, "d2", "dephub", "USES");

        let top = most_connected(&store, 10);
        let mh = top.iter().find(|c| c.id.0 == "mentionhub").unwrap();
        let dh = top.iter().find(|c| c.id.0 == "dephub").unwrap();

        // Raw total still counts the mentions (not dropped from the graph).
        assert_eq!(mh.total, 5);
        assert_eq!(mh.typed_total, 0, "all of mentionhub's edges are mentions");
        assert_eq!(dh.total, 2);
        assert_eq!(dh.typed_total, 2, "dephub's edges are typed dependencies");

        // Ranking: dephub (2 typed) outranks mentionhub (0 typed) despite
        // mentionhub's higher raw total — the co-mention inflation is gone.
        let mh_pos = top.iter().position(|c| c.id.0 == "mentionhub").unwrap();
        let dh_pos = top.iter().position(|c| c.id.0 == "dephub").unwrap();
        assert!(dh_pos < mh_pos, "dependency hub must outrank co-mention hub");
    }
}
