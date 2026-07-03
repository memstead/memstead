//! In-memory graph store. Dumb data structure — no validation, no side effects.
//! All mutations go through Engine methods.

use crate::entity::{Entity, EntityId};
use std::collections::HashMap;

/// Edge in the graph.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub rel_type: String,
    pub target: EntityId,
    pub source: EdgeSource,
}

/// Where an edge was declared. Under the alias model every authored
/// edge is `Explicit` (an entry in the auto-managed `## Relationships`
/// section); `Hierarchy` is a derived view over `PART_OF` rather than
/// an authoring channel; `BodyLink` is engine-emitted from a body
/// wiki-link via the alias-synthesis pass (rel-type equals the source
/// schema's `alias_target_rel_type` pointer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeSource {
    /// Declared in the Relationships section.
    Explicit,
    /// Derived from PART_OF hierarchy.
    Hierarchy,
    /// Engine-emitted from a body wiki-link via the alias-synthesis
    /// pass. The discriminator is store-side only — derived at
    /// store-build time from `rel_type == schema.alias_target_rel_type()`.
    BodyLink,
}

/// Incoming edge — stored in in_edges for efficient reverse lookups.
#[derive(Debug, Clone, PartialEq)]
pub struct InEdge {
    pub rel_type: String,
    pub from: EntityId,
    pub source: EdgeSource,
}

/// The graph store. Three maps: nodes, outgoing edges, incoming edges.
///
/// `Clone` backs the atomic-batch rollback: `batch_update` snapshots
/// the store before preparing items so a refused batch can restore the
/// pre-call graph wholesale.
#[derive(Debug, Clone)]
pub struct Store {
    nodes: HashMap<EntityId, Entity>,
    out_edges: HashMap<EntityId, Vec<Edge>>,
    in_edges: HashMap<EntityId, Vec<InEdge>>,
}

impl Store {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            out_edges: HashMap::new(),
            in_edges: HashMap::new(),
        }
    }

    /// Insert or update a node. If the node already exists, replace it.
    pub fn upsert(&mut self, id: EntityId, entity: Entity) {
        if !self.out_edges.contains_key(&id) {
            self.out_edges.insert(id.clone(), Vec::new());
        }
        if !self.in_edges.contains_key(&id) {
            self.in_edges.insert(id.clone(), Vec::new());
        }
        self.nodes.insert(id, entity);
    }

    /// Remove a node and cascade-delete all its edges.
    pub fn remove(&mut self, id: &EntityId) -> Option<Entity> {
        // Remove outgoing edges and their mirrors in in_edges
        if let Some(out) = self.out_edges.remove(id) {
            for edge in &out {
                if let Some(in_list) = self.in_edges.get_mut(&edge.target) {
                    in_list.retain(|e| &e.from != id);
                }
            }
        }
        // Remove incoming edges and their mirrors in out_edges
        if let Some(inc) = self.in_edges.remove(id) {
            for edge in &inc {
                if let Some(out_list) = self.out_edges.get_mut(&edge.from) {
                    out_list.retain(|e| &e.target != id);
                }
            }
        }
        self.nodes.remove(id)
    }

    pub fn get(&self, id: &EntityId) -> Option<&Entity> {
        self.nodes.get(id)
    }

    pub fn get_mut(&mut self, id: &EntityId) -> Option<&mut Entity> {
        self.nodes.get_mut(id)
    }

    pub fn contains(&self, id: &EntityId) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn all_ids(&self) -> impl Iterator<Item = &EntityId> {
        self.nodes.keys()
    }

    pub fn all_entities(&self) -> impl Iterator<Item = &Entity> {
        self.nodes.values()
    }

    /// Drop every entity whose `EntityId::mem()` matches `mem`,
    /// cascading edges via the existing [`Store::remove`] mechanism.
    /// Returns the number of entities removed (excluding edge-only
    /// cascades — same accounting as `remove`).
    ///
    /// Used by [`Engine::reload_one_mem`] to clear one mem's slice
    /// of the store before reloading entities from the on-disk branch
    /// tip. Pure-iteration implementation: walks `all_ids()`, filters
    /// by mem, then calls `remove` on each. The 132-entity workspace
    /// today reloads the whole store in <1 s so the per-mem filtered
    /// case is microseconds; if the workspace ever grows past
    /// 10k entities the loop can switch to a mem-keyed bucket on
    /// `Store` without changing this signature.
    pub fn remove_entities_by_mem(&mut self, mem: &str) -> usize {
        let to_remove: Vec<EntityId> = self
            .nodes
            .keys()
            .filter(|id| id.mem() == mem)
            .cloned()
            .collect();
        let count = to_remove.len();
        for id in to_remove {
            self.remove(&id);
        }
        count
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Add an edge. Idempotent: if (from, to, type) exists, update source; else append.
    /// Stores in both out_edges and in_edges for bidirectional traversal.
    pub fn add_edge(&mut self, from: EntityId, edge: Edge) {
        let target = edge.target.clone();
        let rel_type = edge.rel_type.clone();
        let source = edge.source.clone();

        // Ensure adjacency lists exist
        self.out_edges.entry(from.clone()).or_default();
        self.in_edges.entry(target.clone()).or_default();

        // Check for existing edge (same from, to, type)
        let out_list = self.out_edges.get_mut(&from).unwrap();
        if let Some(existing) = out_list
            .iter_mut()
            .find(|e| e.target == target && e.rel_type == rel_type)
        {
            existing.source = source.clone();
            // Update mirror
            if let Some(in_list) = self.in_edges.get_mut(&target)
                && let Some(mirror) = in_list
                    .iter_mut()
                    .find(|e| e.from == from && e.rel_type == rel_type)
            {
                mirror.source = source;
            }
        } else {
            out_list.push(edge);
            self.in_edges.get_mut(&target).unwrap().push(InEdge {
                rel_type,
                from,
                source,
            });
        }
    }

    /// Remove a specific edge by (from, to, type).
    pub fn remove_edge(&mut self, from: &EntityId, to: &EntityId, rel_type: &str) {
        if let Some(out_list) = self.out_edges.get_mut(from) {
            out_list.retain(|e| !(e.target == *to && e.rel_type == rel_type));
        }
        if let Some(in_list) = self.in_edges.get_mut(to) {
            in_list.retain(|e| !(e.from == *from && e.rel_type == rel_type));
        }
    }

    /// Remove all outgoing edges from a node (and their mirrors).
    pub fn remove_edges_from(&mut self, id: &EntityId) {
        if let Some(out) = self.out_edges.get_mut(id) {
            let edges = std::mem::take(out);
            for edge in edges {
                if let Some(in_list) = self.in_edges.get_mut(&edge.target) {
                    in_list.retain(|e| &e.from != id);
                }
            }
        }
    }

    /// Get all outgoing edges for a node.
    pub fn outgoing(&self, id: &EntityId) -> &[Edge] {
        self.out_edges.get(id).map_or(&[], |v| v.as_slice())
    }

    /// Get all incoming edges for a node.
    pub fn incoming(&self, id: &EntityId) -> &[InEdge] {
        self.in_edges.get(id).map_or(&[], |v| v.as_slice())
    }

    /// Rename a node. Updates all edge references.
    pub fn rename_node(&mut self, old_id: &EntityId, new_id: EntityId) -> bool {
        if old_id == &new_id {
            return false;
        }
        let Some(mut entity) = self.nodes.remove(old_id) else {
            return false;
        };
        entity.id = new_id.clone();
        self.nodes.insert(new_id.clone(), entity);

        // Move edge lists
        let out = self.out_edges.remove(old_id).unwrap_or_default();
        let inc = self.in_edges.remove(old_id).unwrap_or_default();
        self.out_edges.insert(new_id.clone(), out);
        self.in_edges.insert(new_id.clone(), inc);

        // Update all edges referencing old_id
        for edges in self.out_edges.values_mut() {
            for e in edges.iter_mut() {
                if e.target == *old_id {
                    e.target = new_id.clone();
                }
            }
        }
        for edges in self.in_edges.values_mut() {
            for e in edges.iter_mut() {
                if e.from == *old_id {
                    e.from = new_id.clone();
                }
            }
        }

        // Update `entity.relationships` on every node. This is the list that
        // `write_entity` renders into the markdown frontmatter; without this
        // walk a self-loop (`target == old_id`) would be written to disk
        // under the old id, then re-parsed back as a fresh edge pointing at
        // an auto-stubbed copy of the old id. Out/in edges alone aren't
        // enough — the on-disk form is the source of truth that survives
        // the post-rename re-parse cycle in `engine::mutation::rename`.
        for entity in self.nodes.values_mut() {
            for rel in entity.relationships.iter_mut() {
                if rel.target == *old_id {
                    rel.target = new_id.clone();
                }
            }
        }
        true
    }

    /// Total edge count (outgoing edges only, since in_edges are mirrors).
    pub fn edge_count(&self) -> usize {
        self.out_edges.values().map(|v| v.len()).sum()
    }

    /// Clear all nodes and edges.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.out_edges.clear();
        self.in_edges.clear();
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Relationship;
    use indexmap::IndexMap;

    fn stub_entity(id: &str, mem: &str) -> Entity {
        Entity {
            id: EntityId(id.to_string()),
            title: id.to_string(),
            entity_type: "spec".to_string(),
            mem: mem.to_string(),
            file_path: String::new(),
            metadata: IndexMap::new(),
            sections: IndexMap::new(),
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: true,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn new_store_is_empty() {
        let store = Store::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        assert_eq!(store.edge_count(), 0);
    }

    #[test]
    fn upsert_and_get() {
        let mut store = Store::new();
        let id = EntityId("specs--test".to_string());
        store.upsert(id.clone(), stub_entity("specs--test", "specs"));
        assert_eq!(store.len(), 1);
        assert!(store.get(&id).is_some());
        assert_eq!(store.get(&id).unwrap().title, "specs--test");
    }

    #[test]
    fn upsert_replaces_existing() {
        let mut store = Store::new();
        let id = EntityId("specs--test".to_string());
        store.upsert(id.clone(), stub_entity("specs--test", "specs"));
        let mut updated = stub_entity("specs--test", "specs");
        updated.title = "Updated Title".to_string();
        store.upsert(id.clone(), updated);
        assert_eq!(store.len(), 1);
        assert_eq!(store.get(&id).unwrap().title, "Updated Title");
    }

    #[test]
    fn remove_node_cascades_edges() {
        let mut store = Store::new();
        let a = EntityId("specs--a".to_string());
        let b = EntityId("specs--b".to_string());
        store.upsert(a.clone(), stub_entity("specs--a", "specs"));
        store.upsert(b.clone(), stub_entity("specs--b", "specs"));
        store.add_edge(
            a.clone(),
            Edge {
                rel_type: "USES".to_string(),
                target: b.clone(),
                source: EdgeSource::Explicit,
            },
        );
        assert_eq!(store.edge_count(), 1);
        store.remove(&b);
        assert_eq!(store.len(), 1);
        assert_eq!(store.edge_count(), 0);
        assert!(store.outgoing(&a).is_empty());
    }

    #[test]
    fn add_edge_idempotent() {
        let mut store = Store::new();
        let a = EntityId("specs--a".to_string());
        let b = EntityId("specs--b".to_string());
        store.upsert(a.clone(), stub_entity("specs--a", "specs"));
        store.upsert(b.clone(), stub_entity("specs--b", "specs"));

        store.add_edge(
            a.clone(),
            Edge {
                rel_type: "USES".to_string(),
                target: b.clone(),
                source: EdgeSource::Explicit,
            },
        );
        // Add same edge again — idempotent on (from, to, rel_type)
        store.add_edge(
            a.clone(),
            Edge {
                rel_type: "USES".to_string(),
                target: b.clone(),
                source: EdgeSource::Hierarchy,
            },
        );
        assert_eq!(store.edge_count(), 1);
        assert_eq!(store.outgoing(&a)[0].source, EdgeSource::Hierarchy);
        assert_eq!(store.incoming(&b)[0].source, EdgeSource::Hierarchy);
    }

    #[test]
    fn bidirectional_edges() {
        let mut store = Store::new();
        let a = EntityId("specs--a".to_string());
        let b = EntityId("specs--b".to_string());
        store.upsert(a.clone(), stub_entity("specs--a", "specs"));
        store.upsert(b.clone(), stub_entity("specs--b", "specs"));
        store.add_edge(
            a.clone(),
            Edge {
                rel_type: "USES".to_string(),
                target: b.clone(),
                source: EdgeSource::Explicit,
            },
        );
        assert_eq!(store.outgoing(&a).len(), 1);
        assert_eq!(store.outgoing(&a)[0].target, b);
        assert_eq!(store.incoming(&b).len(), 1);
        assert_eq!(store.incoming(&b)[0].from, a);
    }

    #[test]
    fn remove_edges_from() {
        let mut store = Store::new();
        let a = EntityId("specs--a".to_string());
        let b = EntityId("specs--b".to_string());
        let c = EntityId("specs--c".to_string());
        store.upsert(a.clone(), stub_entity("specs--a", "specs"));
        store.upsert(b.clone(), stub_entity("specs--b", "specs"));
        store.upsert(c.clone(), stub_entity("specs--c", "specs"));
        store.add_edge(
            a.clone(),
            Edge {
                rel_type: "USES".to_string(),
                target: b.clone(),
                source: EdgeSource::Explicit,
            },
        );
        store.add_edge(
            a.clone(),
            Edge {
                rel_type: "USES".to_string(),
                target: c.clone(),
                source: EdgeSource::Explicit,
            },
        );
        assert_eq!(store.edge_count(), 2);
        store.remove_edges_from(&a);
        assert_eq!(store.edge_count(), 0);
        assert!(store.outgoing(&a).is_empty());
        assert!(store.incoming(&b).is_empty());
        assert!(store.incoming(&c).is_empty());
    }

    #[test]
    fn remove_specific_edge() {
        let mut store = Store::new();
        let a = EntityId("specs--a".to_string());
        let b = EntityId("specs--b".to_string());
        store.upsert(a.clone(), stub_entity("specs--a", "specs"));
        store.upsert(b.clone(), stub_entity("specs--b", "specs"));
        store.add_edge(
            a.clone(),
            Edge {
                rel_type: "USES".to_string(),
                target: b.clone(),
                source: EdgeSource::Explicit,
            },
        );
        store.add_edge(
            a.clone(),
            Edge {
                rel_type: "PART_OF".to_string(),
                target: b.clone(),
                source: EdgeSource::Explicit,
            },
        );
        assert_eq!(store.edge_count(), 2);
        store.remove_edge(&a, &b, "USES");
        assert_eq!(store.edge_count(), 1);
        assert_eq!(store.outgoing(&a)[0].rel_type, "PART_OF");
    }

    #[test]
    fn rename_node() {
        let mut store = Store::new();
        let a = EntityId("specs--a".to_string());
        let b = EntityId("specs--b".to_string());
        let new_a = EntityId("specs--a-renamed".to_string());
        store.upsert(a.clone(), stub_entity("specs--a", "specs"));
        store.upsert(b.clone(), stub_entity("specs--b", "specs"));
        store.add_edge(
            a.clone(),
            Edge {
                rel_type: "USES".to_string(),
                target: b.clone(),
                source: EdgeSource::Explicit,
            },
        );
        store.add_edge(
            b.clone(),
            Edge {
                rel_type: "PART_OF".to_string(),
                target: a.clone(),
                source: EdgeSource::Explicit,
            },
        );

        assert!(store.rename_node(&a, new_a.clone()));
        assert!(store.get(&a).is_none());
        assert!(store.get(&new_a).is_some());
        assert_eq!(store.outgoing(&new_a).len(), 1);
        assert_eq!(store.incoming(&new_a).len(), 1);
        assert_eq!(store.incoming(&new_a)[0].from, b);
        // Edge from b to old_a should now point to new_a
        assert_eq!(store.outgoing(&b)[0].target, new_a);
    }

    #[test]
    fn rename_node_rewrites_self_loop_in_relationships_vec() {
        // Regression: a self-loop edge (target == self) stored in both the
        // Store's adjacency HashMaps *and* inside `entity.relationships`
        // used to have only the adjacency side rewritten on rename. The
        // `relationships` Vec kept the old id, which then leaked onto disk
        // via `write_entity` and auto-stubbed on re-parse.
        let mut store = Store::new();
        let old_id = EntityId("specs--selfie".to_string());
        let new_id = EntityId("specs--selfie-renamed".to_string());
        let mut entity = stub_entity("specs--selfie", "specs");
        entity.stub = false;
        entity.relationships.push(Relationship {
            rel_type: "REFERENCES".to_string(),
            target: old_id.clone(),
            description: None,
        });
        store.upsert(old_id.clone(), entity);
        store.add_edge(
            old_id.clone(),
            Edge {
                rel_type: "REFERENCES".to_string(),
                target: old_id.clone(),
                source: EdgeSource::Explicit,
            },
        );

        assert!(store.rename_node(&old_id, new_id.clone()));

        let renamed = store.get(&new_id).expect("renamed entity exists");
        assert_eq!(renamed.relationships.len(), 1);
        assert_eq!(
            renamed.relationships[0].target, new_id,
            "self-loop target inside entity.relationships must be rewritten \
             to new_id — otherwise write_entity leaks old id to disk"
        );
        // And the adjacency side stayed consistent (single self-loop, not
        // duplicated into a dangling-to-old-id edge).
        assert_eq!(store.outgoing(&new_id).len(), 1);
        assert_eq!(store.outgoing(&new_id)[0].target, new_id);
        assert_eq!(store.incoming(&new_id).len(), 1);
        assert_eq!(store.incoming(&new_id)[0].from, new_id);
    }

    #[test]
    fn clear_empties_store() {
        let mut store = Store::new();
        let a = EntityId("specs--a".to_string());
        store.upsert(a, stub_entity("specs--a", "specs"));
        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.edge_count(), 0);
    }

    #[test]
    fn outgoing_empty_for_unknown_id() {
        let store = Store::new();
        assert!(store.outgoing(&EntityId("unknown".to_string())).is_empty());
    }
}
