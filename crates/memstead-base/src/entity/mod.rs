//! Entity types and the markdown pipeline (parse, generate, write,
//! load).
//!
//! Backend-agnostic surface — `Directory` and `ZipArchive` sources
//! live here in [`source`]; the gix-backed `GitTreeSource` lives in
//! `memstead-git-branch::entity::git_tree_source` and shares the same parse
//! pipeline via [`loader::parse_entries`].

pub mod generator;
pub mod id;
pub mod loader;
pub mod parser;
pub mod source;
pub mod store_builder;
pub(crate) mod wikilink_rewrite;
pub mod writer;

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;

/// Unique entity identifier: `mem--entity-path`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct EntityId(pub String);

impl EntityId {
    /// Construct a mem-qualified entity id. Both inputs are NFC-
    /// normalised before joining so the in-memory `HashMap<EntityId, ..>`
    /// key matches across compose-form variants. Without this gate, a
    /// title `"café"` (NFC) creates an entity whose id is the NFC byte
    /// sequence; an NFD-form lookup (`"café"` with combining acute)
    /// produces a byte-different `EntityId` and the store reports
    /// `ENTITY_NOT_FOUND`. The write path already NFC-normalises the
    /// title before slug derivation; this constructor closes the read
    /// path. Mem names are constrained to ASCII (per the mem grammar
    /// in [`crate::entity::id`]), so the NFC pass on the mem half is
    /// structurally a no-op — applied uniformly for symmetry.
    pub fn new(mem: &str, slug: &str) -> Self {
        use unicode_normalization::UnicodeNormalization;
        let mem_nfc: String = mem.nfc().collect();
        let slug_nfc: String = slug.nfc().collect();
        Self(format!("{mem_nfc}--{slug_nfc}"))
    }

    /// Construct from a full `mem--slug` id, NFC-normalising the
    /// string. Use this at every read-path entry point that receives an
    /// id string from outside the engine (MCP tool params, CLI argv,
    /// file-path reconstruction). Direct `EntityId(s)` construction
    /// bypasses normalisation and re-introduces the NFC/NFD lookup
    /// hazard this constructor closes — prefer it.
    pub fn canonical(id: &str) -> Self {
        use unicode_normalization::UnicodeNormalization;
        Self(id.nfc().collect())
    }

    /// Extract the mem part: `specs--my-entity` → `specs`.
    pub fn mem(&self) -> &str {
        match self.0.find("--") {
            Some(idx) => &self.0[..idx],
            None => "",
        }
    }

    /// Extract the name part (last segment): `specs--parent/child` → `child`.
    pub fn name(&self) -> &str {
        let path = self.path();
        match path.rfind('/') {
            Some(i) => &path[i + 1..],
            None => path,
        }
    }

    /// Extract the full path after mem: `specs--parent/child` → `parent/child`.
    pub fn path(&self) -> &str {
        match self.0.find("--") {
            Some(idx) => &self.0[idx + 2..],
            None => &self.0,
        }
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for EntityId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A metadata value with type coercion matching the JS parser behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MetadataValue {
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
}

impl MetadataValue {
    /// Serialize to the string form used in YAML frontmatter.
    pub fn to_frontmatter_string(&self) -> String {
        match self {
            Self::String(s) => s.clone(),
            Self::Integer(n) => n.to_string(),
            Self::Float(v) => format!("{v}"),
            Self::Bool(b) => b.to_string(),
        }
    }

    /// Get as a string reference (only for String variant).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// Check if the value is falsy (for omit-when-falsy serialization).
    pub fn is_falsy(&self) -> bool {
        match self {
            Self::Bool(b) => !b,
            Self::Integer(n) => *n == 0,
            Self::Float(f) => *f == 0.0,
            Self::String(s) => s.is_empty(),
        }
    }
}

impl fmt::Display for MetadataValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_frontmatter_string())
    }
}

/// A parsed entity with metadata, sections, and relationships.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: EntityId,
    pub title: String,
    pub entity_type: String,
    pub mem: String,
    pub file_path: String,
    /// Frontmatter metadata. Ordered (IndexMap) so iteration preserves the
    /// YAML key order the parser saw — required for deterministic rendering
    /// by `render::render_entity_markdown`, which iterates this map directly.
    pub metadata: IndexMap<String, MetadataValue>,
    /// Section keys map to raw content. Ordered (IndexMap) so iteration yields
    /// sections in the order the parser inserted them — today that matches the
    /// schema's declared order, which is what renderers rely on for
    /// deterministic output.
    pub sections: IndexMap<String, String>,
    pub relationships: Vec<Relationship>,
    /// SHA-256 hash of the raw markdown content for optimistic locking.
    pub content_hash: String,
    /// True if this is a stub entity (created from an unresolved reference).
    /// Tracks `stub_kind.is_some()` and is preserved for compatibility with
    /// readers that don't need the typed provenance. New code branches on
    /// [`Self::stub_kind`] when the origin matters (`ForwardReference` /
    /// `LoadTime` / `Residual`).
    pub stub: bool,
    /// Typed stub provenance — set at stub creation and persists for the
    /// engine instance's lifetime. `None` for real (non-stub) entities;
    /// `Some(kind)` matches `stub == true` by construction (every
    /// `make_stub` site sets both fields atomically). See [`StubKind`]
    /// for the variant semantics and lifecycle rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stub_kind: Option<StubKind>,
    /// Per-H2-section H3–H6 heading spans — a derived parse artefact used by
    /// search to attach `heading_path` to per-term matches. Keyed by H2
    /// section key; value lists spans in document order with byte offsets
    /// into the raw section content. Regenerated on every parse; never
    /// written back to markdown, never hashed, never round-tripped. The
    /// in-memory graph stays flat — sub-sections are search-only metadata.
    #[serde(skip, default)]
    pub heading_spans: HashMap<String, Vec<HeadingSpan>>,
}

/// Typed provenance for stub entities. Set at stub creation and lives
/// for the engine instance's
/// lifetime. Boot reconstructs stubs via the parser and always tags
/// them `LoadTime`; the `ForwardReference` and `Residual` variants
/// therefore only appear during the engine lifetime in which they
/// were created and reduce to `LoadTime` after a reload — this is
/// intentional ("annotation, not state") and consistent with the
/// "no tombstones" decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StubKind {
    /// Created by `memstead_relate` against an absent target; the
    /// source entity declared the relation before the target
    /// existed. Promotion via `memstead_create` clears the kind back
    /// to `None` (stub adoption).
    ForwardReference,
    /// Auto-emitted at parse time from a wiki-link / Relationships
    /// entry pointing at an entity not present in the current load.
    /// The canonical post-reload variant for every stub.
    LoadTime,
    /// Left over by a `memstead_delete` or `memstead_rename` of a real
    /// Write-Mem entity whose surviving incoming references all
    /// live in ReadOnly mounts at the time. The in-memory stub
    /// preserves those edges so `incoming(<old-id>)` stays
    /// consistent with what a fresh boot from disk would produce.
    /// `since_commit` records the commit that produced the demote;
    /// `readonly_referrers` snapshots the surviving source ids at
    /// mutation time (not live-updated).
    Residual {
        since_commit: String,
        readonly_referrers: Vec<EntityId>,
    },
}

/// A single H3–H6 heading span recorded under one H2 section. Byte offsets
/// reference the raw section content (the string stored in
/// `Entity.sections[key]`). Spans are stored flat — level skips (H2 → H4
/// without H3) are tolerated and ancestry is resolved at query time via
/// offset containment, not by inserting virtual intermediate spans.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeadingSpan {
    /// Markdown heading level: 3, 4, 5, or 6.
    pub level: u8,
    /// Heading title (trimmed, no `#` prefix).
    pub title: String,
    /// Byte offset into the section content where this heading starts
    /// (the `#` character position).
    pub start_offset: usize,
    /// Byte offset into the section content where this heading's scope ends
    /// — either the start of the next heading with the same or lower level,
    /// or the end of the section.
    pub end_offset: usize,
}

/// A declared relationship in the Relationships section.
///
/// The optional `description` carries the per-edge text after the
/// trailing em-dash on the markdown row (`- **TYPE**: [[X]] — text`).
/// Validated against the rel-type's `per_edge_description` posture at
/// mutation and parse time. Empty-string normalises to `None` so the
/// renderer never emits a bare em-dash followed by nothing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub rel_type: String,
    pub target: EntityId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Relationship {
    /// Construct a relation with no per-edge description — the common
    /// case for rel-types declared `forbidden` or `optional` (no text)
    /// in their schema.
    pub fn new(rel_type: impl Into<String>, target: EntityId) -> Self {
        Self {
            rel_type: rel_type.into(),
            target,
            description: None,
        }
    }
}

/// Normalise a per-edge description from the wire/argument surface:
/// trims surrounding whitespace and collapses empty / whitespace-only
/// inputs to `None`. Applied at every mutation entry-point so the
/// renderer never emits a bare em-dash followed by zero characters and
/// the posture-validation step sees a canonical input.
pub fn normalise_description(description: Option<&str>) -> Option<String> {
    description
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Result of parsing a markdown file. Includes the entity, extracted inline
/// links, and parse-time warnings (e.g. duplicate section headings).
pub struct ParseResult {
    pub entity: Entity,
    /// Wiki-links found in text sections (become REFERENCES edges in the store).
    pub inline_links: Vec<EntityId>,
    /// Warnings raised during parsing — surface only at load / reload / attach
    /// sites (the store builder pushes them into `LoadCollector::warnings`
    /// when present). Validator and mutation sites consume `ParseResult`
    /// without emitting these.
    pub parse_warnings: Vec<crate::ops::WarningHint>,
}

/// Rewrite relationship and inline-link targets whose slug has a
/// `"<mem>--<rest>"` shape where `<mem>` is a visible-writable mem
/// name. Same-mem wiki-links (no `--` prefix, or a prefix that is not a
/// known mem) are left unchanged — the resolver is additive and byte-
/// identical for inputs without a known-mem prefix.
///
/// The parser is single-mem by construction: every produced target is
/// `EntityId(current_mem, slug)`. This helper runs immediately after
/// `parse_markdown` / `parse_file` at every parse-result consumer site so
/// cross-mem references land in the store with the correct target mem.
pub fn resolve_cross_mem_refs(
    relationships: &mut [Relationship],
    inline_links: &mut [EntityId],
    current_mem: &str,
    visible_writable: &HashSet<String>,
) {
    for rel in relationships.iter_mut() {
        if let Some(resolved) = rewrite_target(&rel.target, current_mem, visible_writable) {
            rel.target = resolved;
        }
    }
    for link in inline_links.iter_mut() {
        if let Some(resolved) = rewrite_target(link, current_mem, visible_writable) {
            *link = resolved;
        }
    }
}

/// If `target`'s slug has a `"<prefix>--<rest>"` shape where `prefix` is a
/// visible-writable mem name (and differs from `current_mem`), return
/// the rewritten `EntityId(prefix, rest)`. Otherwise return `None`.
///
/// The split is on the **first** `--` so legacy slugs like
/// `"some-legacy--slug"` stay same-mem whenever `"some-legacy"` is not a
/// registered mem. Self-prefix (`prefix == current_mem`) is also a
/// no-op — it would round-trip to the same `EntityId`, but skipping the
/// rewrite avoids an unnecessary allocation.
fn rewrite_target(
    target: &EntityId,
    current_mem: &str,
    visible_writable: &HashSet<String>,
) -> Option<EntityId> {
    if target.mem() != current_mem {
        // Already points at another mem — this can happen when the
        // resolver is chained (idempotent) or when the target came from
        // a non-parser path. Nothing to do.
        return None;
    }
    let path = target.path();
    let (prefix, rest) = path.split_once("--")?;
    if prefix == current_mem {
        return None;
    }
    if visible_writable.contains(prefix) {
        Some(EntityId::new(prefix, rest))
    } else {
        None
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    fn roster(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn rel(rel_type: &str, mem: &str, slug: &str) -> Relationship {
        Relationship {
            rel_type: rel_type.to_string(),
            target: EntityId::new(mem, slug),
            description: None,
        }
    }

    #[test]
    fn rewrites_cross_mem_relationship_when_prefix_is_known() {
        let mut rels = vec![rel("USES", "plan", "main--foo")];
        let mut inline: Vec<EntityId> = Vec::new();
        resolve_cross_mem_refs(&mut rels, &mut inline, "plan", &roster(&["main", "plan"]));
        assert_eq!(rels[0].target.mem(), "main");
        assert_eq!(rels[0].target.path(), "foo");
    }

    #[test]
    fn leaves_relationship_unchanged_when_prefix_not_in_roster() {
        let mut rels = vec![rel("USES", "plan", "main--foo")];
        let mut inline: Vec<EntityId> = Vec::new();
        resolve_cross_mem_refs(&mut rels, &mut inline, "plan", &roster(&["plan"]));
        assert_eq!(rels[0].target.mem(), "plan");
        assert_eq!(rels[0].target.path(), "main--foo");
    }

    #[test]
    fn target_without_double_dash_is_unchanged() {
        let mut rels = vec![rel("USES", "plan", "foo")];
        let mut inline: Vec<EntityId> = Vec::new();
        resolve_cross_mem_refs(&mut rels, &mut inline, "plan", &roster(&["main", "plan"]));
        assert_eq!(rels[0].target.mem(), "plan");
        assert_eq!(rels[0].target.path(), "foo");
    }

    #[test]
    fn legacy_slug_with_unknown_prefix_stays_same_mem() {
        let mut rels = vec![rel("USES", "plan", "some-legacy--slug")];
        let mut inline: Vec<EntityId> = Vec::new();
        resolve_cross_mem_refs(&mut rels, &mut inline, "plan", &roster(&["main", "plan"]));
        assert_eq!(rels[0].target.mem(), "plan");
        assert_eq!(rels[0].target.path(), "some-legacy--slug");
    }

    #[test]
    fn rewrites_inline_links_identically_to_relationships() {
        let mut rels = vec![rel("USES", "plan", "main--foo")];
        let mut inline: Vec<EntityId> = vec![EntityId::new("plan", "main--bar")];
        resolve_cross_mem_refs(&mut rels, &mut inline, "plan", &roster(&["main", "plan"]));
        assert_eq!(rels[0].target.mem(), "main");
        assert_eq!(rels[0].target.path(), "foo");
        assert_eq!(inline[0].mem(), "main");
        assert_eq!(inline[0].path(), "bar");
    }

    #[test]
    fn self_prefix_is_same_mem_noop() {
        let mut rels = vec![rel("USES", "plan", "plan--foo")];
        let mut inline: Vec<EntityId> = Vec::new();
        resolve_cross_mem_refs(&mut rels, &mut inline, "plan", &roster(&["main", "plan"]));
        // "plan--foo" with current_mem="plan" is a same-mem slug
        // containing `--`; the resolver leaves it alone so the entity
        // id stays `plan--plan--foo` (which equals the input).
        assert_eq!(rels[0].target.mem(), "plan");
        assert_eq!(rels[0].target.path(), "plan--foo");
    }

    #[test]
    fn split_on_first_double_dash() {
        // `main--foo--bar` splits at the first `--`, so prefix=`main`
        // and rest=`foo--bar` → `EntityId("main", "foo--bar")`.
        let mut rels = vec![rel("USES", "plan", "main--foo--bar")];
        let mut inline: Vec<EntityId> = Vec::new();
        resolve_cross_mem_refs(&mut rels, &mut inline, "plan", &roster(&["main", "plan"]));
        assert_eq!(rels[0].target.mem(), "main");
        assert_eq!(rels[0].target.path(), "foo--bar");
    }

    #[test]
    fn target_already_in_another_mem_is_untouched() {
        // A relationship whose target already points at another mem
        // (e.g. carried through a chained-resolver path) is idempotent
        // under a second call.
        let mut rels = vec![rel("USES", "main", "foo")];
        let mut inline: Vec<EntityId> = Vec::new();
        resolve_cross_mem_refs(&mut rels, &mut inline, "plan", &roster(&["main", "plan"]));
        assert_eq!(rels[0].target.mem(), "main");
        assert_eq!(rels[0].target.path(), "foo");
    }
}
