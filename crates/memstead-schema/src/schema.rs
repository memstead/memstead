//! In-memory representation of a loaded schema.

use std::collections::HashMap;
use std::sync::Arc;

use crate::manifest::{
    Cardinality, CrossVaultRelationshipEntry, RelationshipDef, RelationshipMode, SchemaManifest,
};
use crate::types::TypeDefinition;

/// A validated, in-memory schema — the product of the loader.
///
/// Holds the parsed manifest, the resolved semver version, and the set of
/// fully-validated type definitions with `edge_weights` precomputed.
#[derive(Debug)]
pub struct Schema {
    pub manifest: SchemaManifest,
    pub version: semver::Version,
    pub types: HashMap<String, Arc<TypeDefinition>>,
}

impl Schema {
    pub fn get_type(&self, name: &str) -> Option<Arc<TypeDefinition>> {
        self.types.get(name).cloned()
    }

    pub fn relationship_known(&self, name: &str) -> bool {
        self.manifest
            .relationships
            .definitions
            .iter()
            .any(|d| d.name == name)
    }

    /// Returns `true` iff `name` is declared with `acyclic: true` in this
    /// schema. Undeclared names resolve to `false` (permissive): the write
    /// path already rejects undeclared names in strict mode via
    /// `relationship_known`, and open mode is explicitly opt-in to cycles.
    pub fn relationship_acyclic(&self, name: &str) -> bool {
        self.manifest
            .relationships
            .definitions
            .iter()
            .any(|d| d.name == name && d.acyclic)
    }

    /// Returns `true` iff entities of `source_type` propagate `rel_type`
    /// outward — i.e. the type's `propagating_relationships` list
    /// declares this rel-type. A propagating-from-source self-loop is a
    /// weight-bomb (the propagation accumulates into the entity that
    /// originated it), so the engine
    /// refuses self-loops on any `(source_type,
    /// rel_type)` pair where this returns `true`, independent of the
    /// rel-type's `acyclic` flag (which governs longer cycles, a
    /// different concern). Unknown `source_type` returns `false`
    /// (permissive — the type's existence is checked elsewhere).
    pub fn type_propagates(&self, source_type: &str, rel_type: &str) -> bool {
        self.types
            .get(source_type)
            .map(|td| td.propagating_relationships.iter().any(|r| r == rel_type))
            .unwrap_or(false)
    }

    /// Returns the schema-declared manual-authoring posture for a
    /// rel-type. Unknown names resolve to `Allow` (permissive — the
    /// validator path already rejects unknown rel-types in strict
    /// mode). The
    /// explicit-author boundary (`memstead_relate`, `memstead_create`'s
    /// `relations:` inline list, `memstead_update`'s `declare_relations`)
    /// gates on this; the body-link → relation alias machinery does
    /// NOT — the alias path is the *intended* way schema-emitted
    /// rel-types (e.g. REFERENCES) appear on entities.
    pub fn relationship_manual_authoring(&self, name: &str) -> crate::ManualAuthoring {
        self.manifest
            .relationships
            .definitions
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.manual_authoring)
            .unwrap_or_default()
    }

    /// `when_to_use` description for a rel-type, returned as the
    /// recovery hint on `RELATION_MANUAL_AUTHORING_FORBIDDEN`
    /// envelopes. `None` when the rel-type is unknown or the schema
    /// author didn't author the field.
    pub fn relationship_when_to_use(&self, name: &str) -> Option<String> {
        self.manifest
            .relationships
            .definitions
            .iter()
            .find(|d| d.name == name)
            .and_then(|d| d.when_to_use.clone())
    }

    pub fn mode(&self) -> RelationshipMode {
        self.manifest.relationships.mode
    }

    pub fn id(&self) -> (String, semver::Version) {
        (self.manifest.name.clone(), self.version.clone())
    }

    pub fn suggest_type(&self, name: &str) -> Option<String> {
        closest_match(name, self.types.keys().map(String::as_str))
    }

    /// Look up a relationship definition by name. `None` for unknown
    /// names; the `_default` sentinel is reachable but never a real
    /// edge's rel_type.
    pub fn relationship_def(&self, name: &str) -> Option<&RelationshipDef> {
        self.manifest
            .relationships
            .definitions
            .iter()
            .find(|d| d.name == name)
    }

    /// Cardinality hint declared on `name`. `None` for unknown names or
    /// for relationships with no declared cardinality (the default —
    /// shape-free).
    pub fn relationship_cardinality(&self, name: &str) -> Option<Cardinality> {
        self.relationship_def(name)
            .and_then(|d| d.cardinality_per_source)
    }

    pub fn suggest_relationship(&self, name: &str) -> Option<String> {
        closest_match(
            name,
            self.manifest
                .relationships
                .definitions
                .iter()
                .map(|d| d.name.as_str()),
        )
    }

    /// Schema-level `alias_target_rel_type` pointer — names the rel-type
    /// that body wiki-links `[[target]]` should auto-emit as
    /// engine-synthesised relations. `None` means the schema is opt-out
    /// of alias synthesis (unbacked body wiki-links continue to refuse
    /// with `WIKILINK_WITHOUT_RELATION`). The loader has already
    /// validated that the named rel-type is declared.
    pub fn alias_target_rel_type(&self) -> Option<&str> {
        self.manifest.alias_target_rel_type.as_deref()
    }

    /// Look up the cross-vault entry whose `to_schema` matches the
    /// target schema's *name*. Returns `None` when this schema declares
    /// no outbound entry for that domain.
    ///
    /// Eligibility is name-based: a schema names a domain, and a
    /// version is one iteration of describing it. The target vault's
    /// pinned version never participates in the match, so a version
    /// bump on the target side cannot invalidate the declaration. The
    /// loader guarantees `to_schema` is a validated bare schema name,
    /// so plain string equality is exact here.
    pub fn cross_vault_entry(&self, target_name: &str) -> Option<&CrossVaultRelationshipEntry> {
        self.manifest
            .cross_vault_relationships
            .iter()
            .find(|entry| entry.to_schema == target_name)
    }

    /// Load the embedded `default` builtin schema.
    ///
    /// Backed by the embedded YAML bundle under `builtins/schemas/default/`
    /// that ships with every binary. Cached via `OnceLock` so repeated
    /// calls are cheap.
    pub fn builtin_default() -> Arc<Schema> {
        use std::sync::OnceLock;
        static CACHE: OnceLock<Arc<Schema>> = OnceLock::new();
        CACHE
            .get_or_init(|| {
                crate::builtins::load_builtin_schemas()
                    .expect("embedded default schema must load")
                    .into_iter()
                    .find(|s| s.manifest.name == "default")
                    .expect("default schema must be embedded")
            })
            .clone()
    }
}

pub(crate) fn closest_match<'a>(
    needle: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    // Noise floor of `chars/2`: beyond that the input shares almost
    // nothing with the vocabulary, so a "did you mean" suggestion is
    // noise dressed as a hint (MCP F1 — a confidently-wrong suggestion
    // misleads). Mirrors `nearest_str_match` in memstead-base verbatim, so
    // `closest_match`-backed codes (`UNKNOWN_SECTION`, `INVALID_REL_TYPE`)
    // gate consistently with the already-floored `INVALID_ENUM_VALUE`.
    // Returns `None` when nothing is close — the caller omits `suggestion`
    // while still shipping the full declared-list recovery payload.
    let noise_floor = (needle.chars().count() / 2).max(1);
    let mut best: Option<(usize, String)> = None;
    for cand in candidates {
        let d = strsim::levenshtein(needle, cand);
        if d == 0 || d > noise_floor {
            continue;
        }
        match &best {
            Some((bd, _)) if *bd <= d => {}
            _ => best = Some((d, cand.to_string())),
        }
    }
    best.map(|(_, c)| c)
}

#[cfg(test)]
mod closest_match_tests {
    use super::closest_match;

    /// MCP F1: a token with no close declared candidate yields no
    /// suggestion (the `chars/2` noise floor rejects it) — a confidently-
    /// wrong "did you mean" is noise dressed as a hint.
    #[test]
    fn far_token_yields_no_suggestion() {
        let candidates = ["identity", "purpose", "context"];
        // distance to every candidate (14/17/14) far exceeds the
        // chars/2 floor (9) — the egregious wrong-suggestion case.
        assert_eq!(
            closest_match("nonexistent_section", candidates.into_iter()),
            None,
            "a semantically-unrelated token must not get a suggestion",
        );
        // A rel-type token sharing nothing with the vocabulary (distance
        // well past floor) is suppressed too.
        assert_eq!(
            closest_match("TOTALLY_UNRELATED", ["MOTIVATES", "REFERENCES", "PART_OF"].into_iter()),
            None,
            "a far rel-type token must not get a suggestion",
        );
    }

    /// MCP F1 complement: a genuine near-typo (within `chars/2`) still
    /// gets its suggestion.
    #[test]
    fn near_typo_still_suggests() {
        let candidates = ["identity", "purpose", "context"];
        assert_eq!(
            closest_match("identty", candidates.into_iter()),
            Some("identity".to_string()),
            "a one-edit typo must still suggest the intended candidate",
        );
    }

    /// An exact match is not a "did you mean" — `closest_match` is for
    /// unknown tokens, so a zero-distance hit returns None (matches
    /// `nearest_str_match`).
    #[test]
    fn exact_match_returns_none() {
        let candidates = ["identity", "purpose"];
        assert_eq!(closest_match("identity", candidates.into_iter()), None);
    }
}
