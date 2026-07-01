//! Entity → markdown projection writer for the export paths.
//!
//! [`write_entity`] renders an entity to its `{slug}.md` file under a
//! vault directory. It is used by the disk-export and working-tree
//! export paths (`Engine`'s archive export and the git-branch
//! `ops::export`), not by the store-mutation pipeline — live mutations
//! persist through the storage backend's `write_entity`, and entities
//! live flat at `{vault}/{slug}.md` (no PART_OF-hierarchy path
//! computation or file moves).

use std::fs;
use std::path::{Path, PathBuf};

use memstead_schema::TypeDefinition;

use super::Entity;
use super::generator::generate_markdown;

/// Write an entity to its file path under the vault directory.
/// Creates parent directories as needed.
/// Returns the absolute path where the file was written.
pub fn write_entity(
    entity: &Entity,
    vault_dir: &Path,
    schema: &TypeDefinition,
) -> Result<PathBuf, WriteError> {
    if entity.file_path.is_empty() {
        return Err(WriteError::NoFilePath(entity.id.to_string()));
    }

    let full_path = vault_dir.join(&entity.file_path);

    // Verify path doesn't escape vault dir
    let resolved = full_path
        .canonicalize()
        .unwrap_or_else(|_| full_path.clone());
    let resolved_root = vault_dir
        .canonicalize()
        .unwrap_or_else(|_| vault_dir.to_path_buf());
    if !resolved.starts_with(&resolved_root) && full_path != vault_dir.join(&entity.file_path) {
        return Err(WriteError::PathTraversal(entity.file_path.clone()));
    }

    // Create parent directories
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Generate markdown and write
    let content = generate_markdown(entity, schema);
    fs::write(&full_path, content)?;

    Ok(full_path)
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("path traversal detected: {0}")]
    PathTraversal(String),
    #[error("entity has no file_path: {0}")]
    NoFilePath(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{EntityId, MetadataValue};
    use indexmap::IndexMap;
    use memstead_schema::{builtin_names, type_by_name};
    use tempfile::TempDir;

    fn make_test_entity(name: &str) -> Entity {
        let mut metadata = IndexMap::new();
        metadata.insert("level".to_string(), MetadataValue::String("M0".to_string()));
        metadata.insert(
            "created_date".to_string(),
            MetadataValue::String("2026-01-15".to_string()),
        );
        metadata.insert(
            "last_modified".to_string(),
            MetadataValue::String("2026-04-12".to_string()),
        );
        metadata.insert(
            "type".to_string(),
            MetadataValue::String("spec".to_string()),
        );

        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "Test identity.".to_string());
        sections.insert("purpose".to_string(), "Test purpose.".to_string());

        Entity {
            id: EntityId::new("specs", name),
            title: name.to_string(),
            entity_type: "spec".to_string(),
            vault: "specs".to_string(),
            file_path: format!("{name}.md"),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    fn make_concept_entity(name: &str) -> Entity {
        let mut metadata = IndexMap::new();
        metadata.insert(
            "maturity".to_string(),
            MetadataValue::String("emerging".to_string()),
        );
        metadata.insert(
            "abstraction_level".to_string(),
            MetadataValue::String("concrete".to_string()),
        );
        metadata.insert(
            "created_date".to_string(),
            MetadataValue::String("2026-01-15".to_string()),
        );
        metadata.insert(
            "last_modified".to_string(),
            MetadataValue::String("2026-04-12".to_string()),
        );
        metadata.insert(
            "type".to_string(),
            MetadataValue::String("concept".to_string()),
        );

        let mut sections = IndexMap::new();
        sections.insert(
            "definition".to_string(),
            "A precise mental model of X.".to_string(),
        );
        sections.insert(
            "explanation".to_string(),
            "How X operates in practice.".to_string(),
        );
        sections.insert("boundaries".to_string(), "Not Y, not Z.".to_string());
        sections.insert(
            "significance".to_string(),
            "Foundational for understanding W.".to_string(),
        );

        Entity {
            id: EntityId::new("concepts", name),
            title: name.to_string(),
            entity_type: "concept".to_string(),
            vault: "concepts".to_string(),
            file_path: format!("{name}.md"),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn write_entity_creates_file() {
        let dir = TempDir::new().unwrap();
        let schema = type_by_name(builtin_names::SPEC).unwrap();
        let entity = make_test_entity("test-entity");

        let path = write_entity(&entity, dir.path(), &schema).unwrap();
        assert!(path.exists());

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# test-entity"));
    }

    #[test]
    fn write_entity_concept_uses_schema_headings_and_order() {
        let dir = TempDir::new().unwrap();
        let schema = type_by_name(builtin_names::CONCEPT).unwrap();
        let entity = make_concept_entity("clarity");

        let path = write_entity(&entity, dir.path(), &schema).unwrap();
        let content = fs::read_to_string(&path).unwrap();

        // Concept headings, not spec headings
        assert!(content.contains("## Definition"));
        assert!(content.contains("## Explanation"));
        assert!(content.contains("## Boundaries"));
        assert!(content.contains("## Significance"));
        assert!(!content.contains("## Identity"));
        assert!(!content.contains("## Purpose"));

        // Sections appear in schema-declared order: definition, explanation,
        // boundaries, significance
        let def_pos = content.find("## Definition").unwrap();
        let exp_pos = content.find("## Explanation").unwrap();
        let bnd_pos = content.find("## Boundaries").unwrap();
        let sig_pos = content.find("## Significance").unwrap();
        assert!(def_pos < exp_pos);
        assert!(exp_pos < bnd_pos);
        assert!(bnd_pos < sig_pos);

        // Frontmatter uses concept type name
        assert!(content.contains("type: concept"));
        assert!(content.contains("maturity: emerging"));
    }

    #[test]
    fn write_entity_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let schema = type_by_name(builtin_names::SPEC).unwrap();
        let mut entity = make_test_entity("child");
        entity.file_path = "parent/child.md".to_string();

        let path = write_entity(&entity, dir.path(), &schema).unwrap();
        assert!(path.exists());
        assert!(dir.path().join("parent").exists());
    }
}
