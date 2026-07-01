//! PART_OF hierarchy traversal — ancestors and descendants.

use std::collections::HashSet;

use crate::entity::EntityId;
use crate::store::Store;

/// Get all ancestors of an entity via PART_OF chain (with cycle detection).
/// Returns ancestors from immediate parent to root, same-mem only.
pub fn ancestors(store: &Store, id: &EntityId, hierarchy_rel: &str) -> Vec<EntityId> {
    let node = match store.get(id) {
        Some(n) => n,
        None => return Vec::new(),
    };
    let mem = &node.mem;

    let mut result = Vec::new();
    let mut visited = HashSet::new();
    visited.insert(id.clone());

    let mut current = id.clone();

    loop {
        // Find PART_OF edge in outgoing (child PART_OF parent)
        let parent = store
            .outgoing(&current)
            .iter()
            .find(|e| e.rel_type == hierarchy_rel)
            .map(|e| e.target.clone());

        let parent_id = match parent {
            Some(pid) => pid,
            None => break,
        };

        // Cycle detection
        if visited.contains(&parent_id) {
            break;
        }

        // Cross-mem boundary
        let parent_node = match store.get(&parent_id) {
            Some(n) => n,
            None => break,
        };
        if parent_node.mem != *mem {
            break;
        }

        result.push(parent_id.clone());
        visited.insert(parent_id.clone());
        current = parent_id;
    }

    result
}

/// Get all descendants of an entity via PART_OF hierarchy (BFS).
/// Uses incoming PART_OF edges to find children. Same-mem only.
pub fn descendants(store: &Store, id: &EntityId, hierarchy_rel: &str) -> Vec<EntityId> {
    let node = match store.get(id) {
        Some(n) => n,
        None => return Vec::new(),
    };
    let mem = &node.mem;

    let mut result = Vec::new();
    let mut visited = HashSet::new();
    visited.insert(id.clone());

    // BFS using a manual queue
    let mut queue = vec![id.clone()];
    let mut head = 0;

    while head < queue.len() {
        let current = queue[head].clone();
        head += 1;

        // Children are entities with an outgoing PART_OF edge pointing to `current`
        // So we look at incoming edges of `current` with the hierarchy relationship
        for edge in store.incoming(&current) {
            if edge.rel_type != hierarchy_rel {
                continue;
            }
            if visited.contains(&edge.from) {
                continue;
            }
            let child_node = match store.get(&edge.from) {
                Some(n) => n,
                None => continue,
            };
            if child_node.mem != *mem {
                continue;
            }
            visited.insert(edge.from.clone());
            result.push(edge.from.clone());
            queue.push(edge.from.clone());
        }
    }

    result
}

/// Compute file path for an entity based on its PART_OF ancestry.
/// Path is: `grandparent/parent/entity-name.md` (reversed ancestor chain).
pub fn compute_file_path(store: &Store, id: &EntityId, hierarchy_rel: &str) -> String {
    let ancs = ancestors(store, id, hierarchy_rel);
    let name = id.name();

    if ancs.is_empty() {
        return format!("{name}.md");
    }

    let segments: Vec<&str> = ancs.iter().rev().map(|a| a.name()).collect();
    format!("{}/{name}.md", segments.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Entity;
    use crate::store::{Edge, EdgeSource};
    use indexmap::IndexMap;

    fn entity(id: &str, mem: &str) -> Entity {
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
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    fn add_part_of(store: &mut Store, child: &str, parent: &str) {
        store.add_edge(
            EntityId(child.to_string()),
            Edge {
                rel_type: "PART_OF".to_string(),
                target: EntityId(parent.to_string()),
                source: EdgeSource::Explicit,
            },
        );
    }

    #[test]
    fn ancestors_empty_for_root() {
        let mut store = Store::new();
        store.upsert(EntityId("s--root".into()), entity("s--root", "s"));
        let ancs = ancestors(&store, &EntityId("s--root".into()), "PART_OF");
        assert!(ancs.is_empty());
    }

    #[test]
    fn ancestors_single_parent() {
        let mut store = Store::new();
        store.upsert(EntityId("s--parent".into()), entity("s--parent", "s"));
        store.upsert(EntityId("s--child".into()), entity("s--child", "s"));
        add_part_of(&mut store, "s--child", "s--parent");

        let ancs = ancestors(&store, &EntityId("s--child".into()), "PART_OF");
        assert_eq!(ancs, vec![EntityId("s--parent".into())]);
    }

    #[test]
    fn ancestors_chain() {
        let mut store = Store::new();
        store.upsert(EntityId("s--root".into()), entity("s--root", "s"));
        store.upsert(EntityId("s--mid".into()), entity("s--mid", "s"));
        store.upsert(EntityId("s--leaf".into()), entity("s--leaf", "s"));
        add_part_of(&mut store, "s--leaf", "s--mid");
        add_part_of(&mut store, "s--mid", "s--root");

        let ancs = ancestors(&store, &EntityId("s--leaf".into()), "PART_OF");
        assert_eq!(
            ancs,
            vec![EntityId("s--mid".into()), EntityId("s--root".into())]
        );
    }

    #[test]
    fn ancestors_cycle_detection() {
        let mut store = Store::new();
        store.upsert(EntityId("s--a".into()), entity("s--a", "s"));
        store.upsert(EntityId("s--b".into()), entity("s--b", "s"));
        add_part_of(&mut store, "s--a", "s--b");
        add_part_of(&mut store, "s--b", "s--a");

        let ancs = ancestors(&store, &EntityId("s--a".into()), "PART_OF");
        assert_eq!(ancs, vec![EntityId("s--b".into())]); // stops at cycle
    }

    #[test]
    fn ancestors_stops_at_mem_boundary() {
        let mut store = Store::new();
        store.upsert(EntityId("s--child".into()), entity("s--child", "s"));
        store.upsert(
            EntityId("other--parent".into()),
            entity("other--parent", "other"),
        );
        add_part_of(&mut store, "s--child", "other--parent");

        let ancs = ancestors(&store, &EntityId("s--child".into()), "PART_OF");
        assert!(ancs.is_empty()); // parent is in different mem
    }

    #[test]
    fn descendants_of_root() {
        let mut store = Store::new();
        store.upsert(EntityId("s--root".into()), entity("s--root", "s"));
        store.upsert(EntityId("s--a".into()), entity("s--a", "s"));
        store.upsert(EntityId("s--b".into()), entity("s--b", "s"));
        store.upsert(EntityId("s--a/c".into()), entity("s--a/c", "s"));
        add_part_of(&mut store, "s--a", "s--root");
        add_part_of(&mut store, "s--b", "s--root");
        add_part_of(&mut store, "s--a/c", "s--a");

        let desc = descendants(&store, &EntityId("s--root".into()), "PART_OF");
        assert_eq!(desc.len(), 3);
    }

    #[test]
    fn descendants_stops_at_mem_boundary() {
        let mut store = Store::new();
        store.upsert(EntityId("s--root".into()), entity("s--root", "s"));
        store.upsert(
            EntityId("other--child".into()),
            entity("other--child", "other"),
        );
        add_part_of(&mut store, "other--child", "s--root");

        let desc = descendants(&store, &EntityId("s--root".into()), "PART_OF");
        assert!(desc.is_empty());
    }

    #[test]
    fn descendants_empty_for_leaf() {
        let mut store = Store::new();
        store.upsert(EntityId("s--leaf".into()), entity("s--leaf", "s"));

        let desc = descendants(&store, &EntityId("s--leaf".into()), "PART_OF");
        assert!(desc.is_empty());
    }

    #[test]
    fn compute_file_path_root_entity() {
        let mut store = Store::new();
        store.upsert(EntityId("s--my-entity".into()), entity("s--my-entity", "s"));

        let path = compute_file_path(&store, &EntityId("s--my-entity".into()), "PART_OF");
        assert_eq!(path, "my-entity.md");
    }

    #[test]
    fn compute_file_path_nested() {
        let mut store = Store::new();
        store.upsert(EntityId("s--root".into()), entity("s--root", "s"));
        store.upsert(EntityId("s--child".into()), entity("s--child", "s"));
        add_part_of(&mut store, "s--child", "s--root");

        let path = compute_file_path(&store, &EntityId("s--child".into()), "PART_OF");
        assert_eq!(path, "root/child.md");
    }

    #[test]
    fn compute_file_path_deeply_nested() {
        let mut store = Store::new();
        store.upsert(EntityId("s--gp".into()), entity("s--gp", "s"));
        store.upsert(EntityId("s--parent".into()), entity("s--parent", "s"));
        store.upsert(EntityId("s--child".into()), entity("s--child", "s"));
        add_part_of(&mut store, "s--child", "s--parent");
        add_part_of(&mut store, "s--parent", "s--gp");

        let path = compute_file_path(&store, &EntityId("s--child".into()), "PART_OF");
        assert_eq!(path, "gp/parent/child.md");
    }
}
