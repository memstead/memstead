//! Filesystem walker — loads all .md files from vault directories into entities.
//!
//! Thin wrapper over `entity::source::EntitySource`: the source hands back
//! `(relative_path, content)` pairs, and this module layers on the
//! entity-level concerns (empty-file skipping, per-file schema resolution,
//! parse). The source abstraction is deliberately narrow so new backing
//! stores (directory, zip archive, …) can be added without touching this
//! file.

use std::path::PathBuf;
use std::sync::Arc;

use memstead_schema::{Schema, TypeDefinition, type_by_name};

use super::ParseResult;
use super::parser;
use super::source::EntitySource;

/// Resolve the per-entity `TypeDefinition` for a markdown entry against
/// the vault's pinned schema. Resolution order:
///
/// 1. If the file's frontmatter declares `type: foo`, look `foo` up in
///    the vault schema. Hit → use that type.
/// 2. Same name, default-schema fallback (`type_by_name(name)`). Hit →
///    use that type. This preserves the pre-cutover behavior for files
///    declaring a type the vault schema does not declare (typo, in-flight
///    schema migration, archived dummy data).
/// 3. No frontmatter type: fall back to the vault schema's `spec` type;
///    if the vault schema has no `spec`, fall back to the default
///    schema's `spec` (always available — used as the engine-wide
///    sentinel via `engine_fallback_type`).
///
/// The function never panics — the final default-schema `spec` lookup is
/// guaranteed to exist by the schema crate's invariants.
fn resolve_type_for_entry(vault_schema: &Schema, content: &str) -> Arc<TypeDefinition> {
    if let Some(name) = parser::peek_type_from_frontmatter(content) {
        if let Some(t) = vault_schema.get_type(&name) {
            return t;
        }
        if let Some(t) = type_by_name(&name) {
            return t;
        }
    }
    vault_schema
        .get_type("spec")
        .or_else(|| type_by_name("spec"))
        .expect("default-schema spec must always exist")
}

/// Result of loading a vault directory.
pub struct LoadResult {
    /// Successfully parsed entities with their inline links.
    pub entities: Vec<ParseResult>,
    /// Parse errors encountered (file path + error message). Non-fatal.
    pub errors: Vec<(PathBuf, String)>,
}

/// Load all entities from a vault directory.
///
/// Walks the directory recursively, finds `.md` files, parses each.
/// Collects parse errors without stopping — returns all entities + all errors.
/// Sequential reads for deterministic ordering.
pub fn load_vault(
    vault_dir: &std::path::Path,
    vault: &str,
    vault_schema: &Schema,
) -> Result<LoadResult, LoadError> {
    load_from_source(
        EntitySource::Directory {
            root: vault_dir.to_path_buf(),
        },
        vault,
        vault_schema,
    )
}

/// Load all entities from a sealed `.mem` vault archive (legacy
/// `.mdgv`-layout archives are read-tolerated).
///
/// Shape-identical to `load_vault` — opens the zip, yields one
/// `ParseResult` per `.md` entry, collects per-file errors. The
/// archive's `.memstead/config.json` is not consulted here; use
/// `vault_cache::read_published_config` up front if you need identity
/// or format-version checks before loading entities.
///
/// Strips any explicit relationship whose target is outside this vault's
/// own vault and logs it. v1 keeps every vault an island, and
/// `memstead_relate` already rejects cross-vault edges on the write side —
/// this is the defensive pass for hand-edited archives that may still
/// carry them. Inline wiki-links are already same-vault by construction
/// (`wiki_link_to_id` resolves every `[[…]]` to `current_vault`), so no
/// sanitization is needed for the inline-links list.
pub fn load_vault_archive(
    archive_path: &std::path::Path,
    vault: &str,
    vault_schema: &Schema,
) -> Result<LoadResult, LoadError> {
    // Archives are self-contained and schema-published — their internal
    // layout is frozen at publish time. No skip list applies.
    let mut result = load_from_source(
        EntitySource::ZipArchive(archive_path.to_path_buf()),
        vault,
        vault_schema,
    )?;
    sanitize_cross_vault_relationships(&mut result.entities, vault);
    Ok(result)
}

/// Strip relationships whose target lives outside the given vault.
///
/// Mutates `parse_results.entity.relationships` in place. Logs each
/// stripped relationship at `warn` level so surprises surface in the
/// user's logs, with a summary line per vault when any were removed.
/// Intentionally does not fail the load — the load policy is
/// best-effort-with-warnings, matching the engine's log+skip handling
/// for missing or corrupt archives.
fn sanitize_cross_vault_relationships(parse_results: &mut [ParseResult], vault: &str) {
    let mut stripped_total: usize = 0;
    for parse_result in parse_results.iter_mut() {
        let entity_id = parse_result.entity.id.clone();
        let before = parse_result.entity.relationships.len();
        parse_result.entity.relationships.retain(|rel| {
            let same_vault = rel.target.vault() == vault;
            if !same_vault {
                tracing::warn!(
                    vault = vault,
                    from = %entity_id,
                    to = %rel.target,
                    rel_type = rel.rel_type.as_str(),
                    "stripping cross-vault relationship from read vault \
                     (published archives are self-contained; cross-vault \
                     authorization is workspace-local and does not travel)"
                );
            }
            same_vault
        });
        stripped_total += before - parse_result.entity.relationships.len();
    }
    if stripped_total > 0 {
        tracing::warn!(
            vault = vault,
            stripped = stripped_total,
            "read vault contained {} cross-vault relationship(s); stripped on load",
            stripped_total
        );
    }
}

/// Parse every `.md` entry from the given source. Shared between
/// directory-backed (writable) and archive-backed (read-only) loads.
///
/// Per-entity type resolution goes through `resolve_type_for_entry` —
/// the vault's pinned schema is the authority, with the default schema
/// as a fallback for files declaring a type the vault schema does not
/// declare. This matches the engine's mutation-time schema lookup
/// (`schema_for_vault`) so parse-time consumers (duplicate-section
/// warnings, missing-required-section warnings, write_rules retrieval)
/// see the schema the workspace pinned, not the engine default.
fn load_from_source(
    source: EntitySource,
    vault: &str,
    vault_schema: &Schema,
) -> Result<LoadResult, LoadError> {
    let (source_entries, read_errors) = source.read_all()?;
    Ok(parse_entries(source_entries, read_errors, vault, vault_schema))
}

/// Parse a pre-collected set of source entries against the vault's
/// schema. Public so the workspace-side git-tree adapter can reuse the
/// same parse loop without re-implementing empty-file skipping or the
/// per-entity schema lookup.
pub fn parse_entries(
    source_entries: Vec<super::source::SourceEntry>,
    read_errors: Vec<super::source::SourceReadError>,
    vault: &str,
    vault_schema: &Schema,
) -> LoadResult {
    let mut entities = Vec::new();
    let mut errors: Vec<(PathBuf, String)> = read_errors
        .into_iter()
        .map(|e| (e.source_path, e.error.to_string()))
        .collect();

    for entry in source_entries {
        // Skip empty files
        if entry.content.trim().is_empty() {
            continue;
        }

        let resolved_type = resolve_type_for_entry(vault_schema, &entry.content);

        match parser::parse_markdown(
            &entry.content,
            &entry.relative_path,
            resolved_type.as_ref(),
            vault,
        ) {
            Ok(mut result) => {
                result.entity.file_path = entry.relative_path;
                entities.push(result);
            }
            Err(e) => {
                errors.push((entry.source_path, e.to_string()));
            }
        }
    }

    LoadResult { entities, errors }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("vault directory not found: {0}")]
    DirNotFound(String),
    #[error("parse error in {file}: {source}")]
    Parse {
        file: String,
        source: parser::ParseError,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("archive not found: {0}")]
    ArchiveNotFound(String),
    /// A zip-level failure (corrupt header, invalid entry, etc.) or a
    /// policy rejection (zip-slip, symlink, absolute entry path). Kept
    /// as a single variant because both mean "this archive is unsafe to
    /// load" — the message is the action item.
    #[error("invalid archive: {0}")]
    InvalidArchive(String),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// A git ref named by the workspace-side adapter could not be
    /// resolved in the open repository. The ref-name string is echoed
    /// back so an operator log line is self-explanatory. Constructed
    /// only by `memstead-git-branch::entity::git_tree_source`.
    #[error("git ref not found: {0}")]
    RefNotFound(String),
    /// A `gix`-level failure while reading the tree (object missing,
    /// corrupt repository, IO underneath the object database). The
    /// wrapped message names the underlying gix error so the
    /// loader-level message stays one-line and grep-friendly.
    /// Constructed only by `memstead-git-branch::entity::git_tree_source`.
    #[error("git tree read error: {0}")]
    GitTree(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{Entity, EntityId, Relationship};
    use indexmap::IndexMap;
    use memstead_schema::Schema;
    use std::fs;
    use tempfile::TempDir;

    fn setup_vault(entities: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (name, content) in entities {
            let path = dir.path().join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, content).unwrap();
        }
        dir
    }

    // The git-tree round-trip test lives in
    // `memstead-git-branch::entity::git_tree_source` alongside the
    // GitTreeSource impl that constructs the gix-backed source.

    #[test]
    fn load_single_entity() {
        let dir = setup_vault(&[(
            "test-entity.md",
            "---\ntype: spec\n---\n# Test Entity\n\n## Identity\n\nTest.\n",
        )]);
        let schema = Schema::builtin_default();
        let result = load_vault(dir.path(), "specs", &schema).unwrap();
        assert_eq!(result.entities.len(), 1);
        assert!(result.errors.is_empty());
        assert_eq!(result.entities[0].entity.title, "Test Entity");
    }

    #[test]
    fn load_nested_entities() {
        let dir = setup_vault(&[
            (
                "parent.md",
                "---\ntype: spec\n---\n# Parent\n\n## Identity\n\nParent entity.\n",
            ),
            (
                "parent/child.md",
                "---\ntype: spec\n---\n# Child\n\n## Identity\n\nChild entity.\n",
            ),
        ]);
        let schema = Schema::builtin_default();
        let result = load_vault(dir.path(), "specs", &schema).unwrap();
        assert_eq!(result.entities.len(), 2);
    }

    #[test]
    fn load_skips_engine_internal_dirs() {
        // `.git/` and `.memstead/` are engine-internal and must never
        // yield entities. Other dot-prefixed directories (e.g.
        // `.obsidian/`) DO load by default.
        let dir = setup_vault(&[
            (
                "visible.md",
                "---\ntype: spec\n---\n# Visible\n\n## Identity\n\nTest.\n",
            ),
            (
                ".git/secret.md",
                "---\ntype: spec\n---\n# GitSecret\n\n## Identity\n\nSecret.\n",
            ),
            (
                ".memstead/note.md",
                "---\ntype: spec\n---\n# MemsteadNote\n\n## Identity\n\nNote.\n",
            ),
        ]);
        let schema = Schema::builtin_default();
        let result = load_vault(dir.path(), "specs", &schema).unwrap();
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].entity.title, "Visible");
    }

    #[test]
    fn load_skips_empty_files() {
        let dir = setup_vault(&[
            (
                "real.md",
                "---\ntype: spec\n---\n# Real\n\n## Identity\n\nContent.\n",
            ),
            ("empty.md", ""),
            ("whitespace.md", "   \n  \n  "),
        ]);
        let schema = Schema::builtin_default();
        let result = load_vault(dir.path(), "specs", &schema).unwrap();
        assert_eq!(result.entities.len(), 1);
    }

    #[test]
    fn load_nonexistent_dir() {
        let schema = Schema::builtin_default();
        let result = load_vault(std::path::Path::new("/nonexistent/path"), "specs", &schema);
        assert!(result.is_err());
    }

    #[test]
    fn load_mixed_schema_vault_uses_per_file_schema() {
        // Principle file and concept file in the same vault, loaded with the
        // concept schema as the (fallback) default. Each entity must parse
        // against its own frontmatter-declared schema.
        let principle_body = "---\ntype: principle\n---\n\
# My Principle\n\n\
## Statement\n\nPrinciple statement body.\n\n\
## Scope\n\nScope body.\n\n\
## Justification\n\nJustification body.\n\n\
## Exceptions\n\n- one\n- two\n\n\
## Consequences\n\nConsequences body.\n";
        let concept_body = "---\ntype: concept\n---\n\
# My Concept\n\n\
## Definition\n\nConcept definition.\n\n\
## Explanation\n\nExplanation body.\n\n\
## Boundaries\n\nBoundaries body.\n\n\
## Significance\n\nSignificance body.\n";
        let dir = setup_vault(&[("p.md", principle_body), ("c.md", concept_body)]);

        // Both files declare their type explicitly, so the loader's
        // schema-driven type lookup picks the right TypeDefinition per
        // entity from the default schema regardless of which "fallback"
        // would apply.
        let schema = Schema::builtin_default();
        let result = load_vault(dir.path(), "knowledge", &schema).unwrap();
        assert_eq!(result.entities.len(), 2);
        assert!(result.errors.is_empty());

        let by_title: std::collections::HashMap<_, _> = result
            .entities
            .iter()
            .map(|r| (r.entity.title.as_str(), &r.entity))
            .collect();

        let principle = by_title.get("My Principle").expect("principle entity");
        assert_eq!(principle.entity_type, "principle");
        assert!(principle.sections.contains_key("statement"));
        assert!(principle.sections.contains_key("scope"));
        assert!(principle.sections.contains_key("justification"));
        // Must NOT carry concept-schema keys
        assert!(!principle.sections.contains_key("definition"));
        assert!(!principle.sections.contains_key("explanation"));
        assert!(
            !principle.sections["statement"].is_empty(),
            "principle's Statement must retain content"
        );

        let concept = by_title.get("My Concept").expect("concept entity");
        assert_eq!(concept.entity_type, "concept");
        assert!(concept.sections.contains_key("definition"));
        assert!(!concept.sections.contains_key("statement"));
    }

    #[test]
    fn load_vault_falls_back_when_frontmatter_missing_schema() {
        let body = "---\nlevel: M0\n---\n\
# Fallback Case\n\n\
## Identity\n\nBody.\n";
        let dir = setup_vault(&[("x.md", body)]);
        let schema = Schema::builtin_default();
        let result = load_vault(dir.path(), "specs", &schema).unwrap();
        assert_eq!(result.entities.len(), 1);
        let entity = &result.entities[0].entity;
        assert_eq!(entity.entity_type, "spec");
        assert!(entity.sections.contains_key("identity"));
    }

    #[test]
    fn load_vault_falls_back_on_unknown_type_name() {
        let body = "---\ntype: nonexistent-type\n---\n\
# Unknown Case\n\n\
## Identity\n\nBody.\n";
        let dir = setup_vault(&[("x.md", body)]);
        let schema = Schema::builtin_default();
        let result = load_vault(dir.path(), "specs", &schema).unwrap();
        assert_eq!(result.entities.len(), 1);
        let entity = &result.entities[0].entity;
        // Parser preserves the frontmatter type name verbatim in entity.entity_type.
        // The fallback only dictates which type's sections are used to parse.
        assert_eq!(entity.entity_type, "nonexistent-type");
        assert!(entity.sections.contains_key("identity"));
    }

    // --- cross-vault relationship sanitization ---

    /// Build a ParseResult directly. The markdown parser can't naturally
    /// emit a cross-vault relationship (`wiki_link_to_id` forces every
    /// target into the current vault), so the defensive strip is
    /// exercised by synthesizing the poisoned state directly.
    fn synthetic_parse_result(
        entity_vault: &str,
        entity_slug: &str,
        rels: Vec<Relationship>,
    ) -> ParseResult {
        let id = EntityId::new(entity_vault, entity_slug);
        ParseResult {
            entity: Entity {
                id: id.clone(),
                title: entity_slug.to_string(),
                entity_type: "spec".to_string(),
                vault: entity_vault.to_string(),
                file_path: format!("{entity_slug}.md"),
                metadata: IndexMap::new(),
                sections: IndexMap::new(),
                relationships: rels,
                content_hash: String::new(),
                stub: false,
                stub_kind: None,
                heading_spans: std::collections::HashMap::new(),
            },
            inline_links: Vec::new(),
            parse_warnings: Vec::new(),
        }
    }

    #[test]
    fn sanitize_strips_cross_vault_relationships() {
        // Poisoned fixture: one in-vault edge (kept) and one out-of-vault
        // edge (stripped). Guards against hand-edited archives that carry
        // pre-v1 cross-vault references — the read-side mirror of the
        // write-side guard in `engine::mutation::relate`.
        let same = Relationship {
            rel_type: "USES".to_string(),
            target: EntityId::new("aws-patterns", "lambda"),
            description: None,
        };
        let cross = Relationship {
            rel_type: "DERIVES_FROM".to_string(),
            target: EntityId::new("specs", "readme"),
            description: None,
        };
        let mut results = vec![synthetic_parse_result(
            "aws-patterns",
            "api-gateway",
            vec![same.clone(), cross.clone()],
        )];

        sanitize_cross_vault_relationships(&mut results, "aws-patterns");

        let kept = &results[0].entity.relationships;
        assert_eq!(kept.len(), 1, "cross-vault edge must be stripped");
        assert_eq!(kept[0].target, same.target);
        assert_eq!(kept[0].rel_type, same.rel_type);
    }

    #[test]
    fn sanitize_is_noop_when_all_relationships_are_same_vault() {
        let rel = Relationship {
            rel_type: "USES".to_string(),
            target: EntityId::new("aws-patterns", "lambda"),
            description: None,
        };
        let mut results = vec![synthetic_parse_result(
            "aws-patterns",
            "api-gateway",
            vec![rel.clone()],
        )];

        sanitize_cross_vault_relationships(&mut results, "aws-patterns");

        assert_eq!(results[0].entity.relationships.len(), 1);
        assert_eq!(results[0].entity.relationships[0].target, rel.target);
    }

    #[test]
    fn sanitize_handles_multiple_entities_with_mixed_edges() {
        // Two entities: first has only same-vault edges, second has only
        // cross-vault ones. After sanitization the second ends up empty
        // and the first is untouched.
        let a_rel = Relationship {
            rel_type: "USES".to_string(),
            target: EntityId::new("aws-patterns", "lambda"),
            description: None,
        };
        let b_cross1 = Relationship {
            rel_type: "MENTIONS".to_string(),
            target: EntityId::new("specs", "one"),
            description: None,
        };
        let b_cross2 = Relationship {
            rel_type: "MENTIONS".to_string(),
            target: EntityId::new("internal-notes", "two"),
            description: None,
        };
        let mut results = vec![
            synthetic_parse_result("aws-patterns", "a", vec![a_rel.clone()]),
            synthetic_parse_result(
                "aws-patterns",
                "b",
                vec![b_cross1.clone(), b_cross2.clone()],
            ),
        ];

        sanitize_cross_vault_relationships(&mut results, "aws-patterns");

        assert_eq!(results[0].entity.relationships.len(), 1);
        assert!(results[1].entity.relationships.is_empty());
    }

    #[test]
    fn load_collects_parse_errors() {
        let dir = setup_vault(&[
            (
                "good.md",
                "---\ntype: spec\n---\n# Good\n\n## Identity\n\nGood.\n",
            ),
            // This file has content but no title — should still parse (title defaults to id)
            (
                "no-title.md",
                "---\ntype: spec\n---\n\n## Identity\n\nNo title.\n",
            ),
        ]);
        let schema = Schema::builtin_default();
        let result = load_vault(dir.path(), "specs", &schema).unwrap();
        // Both should parse — no-title falls back to filename-derived title
        assert_eq!(result.entities.len(), 2);
    }
}
