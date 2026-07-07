//! Writing-guidance resolution — merge a schema's `default_writing_guidance`
//! with a mem's per-mem `writeGuidance` into the goal/avoid prose the run
//! brief renders.
//!
//! Engine-side port of the plugin's `lib/writing-guidance.mjs`
//! `resolveWritingGuidance` (the goal/avoid merge). The plugin's YAML
//! extractor (`extractDefaultWritingGuidance`) is **not** ported: it exists
//! only because the skill reads schema YAML off disk, whereas the engine
//! already holds a parsed schema and reads `default_writing_guidance`
//! directly.
//!
//! Precedence, per field, mirrors the plugin's `mergeBlock`:
//!   1. a **legacy** per-mem literal (`writeGuidance.goal` / `.avoid`,
//!      pre-migration) wins verbatim and the schema default is ignored — a
//!      half-finished migration must not silently lose the operator's prose;
//!   2. otherwise the schema default and the per-mem `*_additions` combine:
//!      default alone, additions alone, or `default + "\n\n" + additions`.
//!
//! An empty string is treated as absent everywhere.
//!
//! Pass-through `writeGuidance` keys (granularity, stack, language, …) and
//! their fallback rendering (`renderResolvedGuidance`) are **not** modelled
//! here yet — they land with the operative-data / fallback block that
//! consumes them.

/// A schema's `default_writing_guidance` — goal/avoid prose the schema
/// author ships for every mem pinned to that schema.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuidanceDefaults {
    /// The schema's default goal prose.
    pub goal: Option<String>,
    /// The schema's default failure-modes-to-avoid prose.
    pub avoid: Option<String>,
}

/// A mem's `writeGuidance`: optional per-mem additions to the schema
/// defaults and an optional legacy literal override (pre-migration).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemGuidance {
    /// Per-mem prose appended to the schema's default goal.
    pub goal_additions: Option<String>,
    /// Per-mem prose appended to the schema's default avoid.
    pub avoid_additions: Option<String>,
    /// A legacy pre-migration literal goal — wins verbatim if present.
    pub legacy_goal: Option<String>,
    /// A legacy pre-migration literal avoid — wins verbatim if present.
    pub legacy_avoid: Option<String>,
}

/// The merged goal/avoid prose, ready for the brief's Goal / Failure-modes
/// blocks. A field absent everywhere is `None` (the brief renders no header
/// for it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedGuidance {
    /// The resolved goal prose, if any.
    pub goal: Option<String>,
    /// The resolved avoid prose, if any.
    pub avoid: Option<String>,
}

/// An empty string counts as absent (matching the plugin's truthiness check).
fn present(s: Option<&str>) -> Option<&str> {
    s.filter(|x| !x.is_empty())
}

/// Merge one field (goal or avoid) from its schema default, per-mem
/// additions, and any legacy literal. Mirrors the plugin's `mergeBlock`.
pub fn merge_guidance_block(
    default: Option<&str>,
    additions: Option<&str>,
    legacy: Option<&str>,
) -> Option<String> {
    let default = present(default);
    let additions = present(additions);

    // Legacy literal wins verbatim; the schema default is ignored for this
    // field. The migration sweep removes these keys.
    if let Some(legacy) = present(legacy) {
        tracing::warn!(
            "ingest guidance: mem carries a legacy writeGuidance literal; \
             the schema's default_writing_guidance is ignored for this field \
             (migrate the prose into the schema, or rename to *_additions)"
        );
        return Some(legacy.to_string());
    }

    match (default, additions) {
        (None, None) => None,
        (Some(default), None) => Some(default.to_string()),
        (None, Some(additions)) => Some(additions.to_string()),
        (Some(default), Some(additions)) => Some(format!("{}\n\n{}", default.trim_end(), additions)),
    }
}

/// Resolve a mem's writing guidance against its schema defaults.
pub fn resolve_writing_guidance(
    defaults: &GuidanceDefaults,
    mem: &MemGuidance,
) -> ResolvedGuidance {
    ResolvedGuidance {
        goal: merge_guidance_block(
            defaults.goal.as_deref(),
            mem.goal_additions.as_deref(),
            mem.legacy_goal.as_deref(),
        ),
        avoid: merge_guidance_block(
            defaults.avoid.as_deref(),
            mem.avoid_additions.as_deref(),
            mem.legacy_avoid.as_deref(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four non-legacy merge shapes: neither, default-only, additions-
    /// only, and default + additions joined by a blank line (with the
    /// default's trailing whitespace trimmed before the join).
    #[test]
    fn merge_combines_default_and_additions() {
        assert_eq!(merge_guidance_block(None, None, None), None);
        assert_eq!(merge_guidance_block(Some(""), Some(""), None), None);
        assert_eq!(
            merge_guidance_block(Some("D"), None, None),
            Some("D".to_string())
        );
        assert_eq!(
            merge_guidance_block(None, Some("A"), None),
            Some("A".to_string())
        );
        assert_eq!(
            merge_guidance_block(Some("D\n"), Some("A"), None),
            Some("D\n\nA".to_string()),
            "default's trailing newline is trimmed, then joined by a blank line"
        );
    }

    /// A legacy literal wins verbatim and suppresses the schema default and
    /// the additions.
    #[test]
    fn legacy_literal_wins() {
        assert_eq!(
            merge_guidance_block(Some("schema default"), Some("additions"), Some("legacy prose")),
            Some("legacy prose".to_string())
        );
    }

    /// The whole-object resolve wires goal and avoid independently and drops
    /// fields absent everywhere.
    #[test]
    fn resolve_wires_goal_and_avoid() {
        let defaults = GuidanceDefaults {
            goal: Some("build coverage".to_string()),
            avoid: None,
        };
        let mem = MemGuidance {
            goal_additions: Some("prefer small entities".to_string()),
            avoid_additions: None,
            legacy_goal: None,
            legacy_avoid: None,
        };
        let r = resolve_writing_guidance(&defaults, &mem);
        assert_eq!(
            r.goal.as_deref(),
            Some("build coverage\n\nprefer small entities")
        );
        assert_eq!(r.avoid, None, "avoid absent everywhere is dropped");
    }
}
