//! Entity-ID uniqueness across the archive after Unicode normalization.
//!
//! The filesystem lets two files coexist whose names are equivalent
//! under NFC/NFD (think "Björn" typed two different ways) or whose
//! title-to-slug pipelines collapse into the same id ("Hello
//! World.md" and "hello-world.md" both slugify to `hello-world`).
//! The last-written file wins silently on steady-state load. At
//! strict ingress we reject.

use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization;

use super::ValidationError;
use crate::entity::Entity;

/// Reject if any two entities have the same id under NFC normalization.
/// Reports the two `file_path`s so the error points at both colliding
/// files.
pub fn check_unique_ids(entities: &[Entity]) -> Result<(), ValidationError> {
    let mut seen: HashMap<String, &str> = HashMap::new();
    for entity in entities {
        let nfc: String = entity.id.as_ref().nfc().collect();
        if let Some(prev_path) = seen.get(&nfc) {
            return Err(ValidationError::DuplicateEntityId {
                id: nfc,
                paths: ((*prev_path).to_string(), entity.file_path.clone()),
            });
        }
        seen.insert(nfc, entity.file_path.as_str());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{Entity, EntityId};
    use indexmap::IndexMap;

    fn stub_entity(id: &str, file_path: &str) -> Entity {
        Entity {
            id: EntityId(id.to_string()),
            title: String::new(),
            entity_type: "spec".to_string(),
            vault: "v".to_string(),
            file_path: file_path.to_string(),
            metadata: IndexMap::new(),
            sections: IndexMap::new(),
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn accepts_distinct_ids() {
        let entities = vec![
            stub_entity("v--a", "a.md"),
            stub_entity("v--b", "b.md"),
            stub_entity("v--a/child", "a/child.md"),
        ];
        check_unique_ids(&entities).unwrap();
    }

    #[test]
    fn rejects_ascii_duplicates() {
        let entities = vec![
            stub_entity("v--hello-world", "a.md"),
            stub_entity("v--hello-world", "b.md"),
        ];
        let err = check_unique_ids(&entities).unwrap_err();
        match err {
            ValidationError::DuplicateEntityId { id, paths } => {
                assert_eq!(id, "v--hello-world");
                assert_eq!(paths, ("a.md".to_string(), "b.md".to_string()));
            }
            other => panic!("expected DuplicateEntityId, got {other:?}"),
        }
    }

    #[test]
    fn rejects_nfc_nfd_duplicates() {
        // "Björn" can be encoded NFC (B-j-ö-r-n, 'ö' is one codepoint)
        // or NFD (B-j-o-\u{0308}-r-n). The filesystem preserves both;
        // the validator normalizes to NFC and catches the collision.
        let entities = vec![
            stub_entity("v--bj\u{00F6}rn", "nfc.md"),
            stub_entity("v--bjo\u{0308}rn", "nfd.md"),
        ];
        let err = check_unique_ids(&entities).unwrap_err();
        assert!(matches!(err, ValidationError::DuplicateEntityId { .. }));
    }

    #[test]
    fn accepts_empty() {
        check_unique_ids(&[]).unwrap();
    }
}
