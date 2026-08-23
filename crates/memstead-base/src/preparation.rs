//! The engine-owned **preparation registry** — the one place that says which
//! preparations exist, on which anchor grains, at which engine touchpoint,
//! and what each grain's PREPARED FORM is.
//!
//! A source declares at most one preparation ([`crate::pipeline::Source::preparation`],
//! a string identifier). The engine refuses any identifier this registry
//! does not know ([`crate::binding::CapabilityError::PreparationUnsupported`],
//! raised by [`crate::binding::validate_binding`] and mirrored on the
//! brief-render path for a record that acquired one by hand) and consults the
//! registry at exactly two touchpoints:
//!
//! - **Touchpoint A — prepared form.** Anchor observation asks the registry
//!   for an artifact's prepared form before hashing it (the engine's one
//!   per-anchor observation site). The standalone `verify-anchors` operation
//!   and the binding-backed verify share that site, so both inherit every
//!   registered preparation without redesign.
//! - **Touchpoint B — delivery units.** The ingest delivery path asks the
//!   registry for a source's unit sequence. No delivery preparation is
//!   registered yet; the touchpoint is named here ([`Touchpoint::DeliveryUnits`])
//!   so the registry's shape carries it from the start.
//!
//! **Identity.** [`crate::binding::PREPARATION_IMPL_VERSION`] is hashed into
//! every binding's `hash(D)` next to the declared identifier. Landing or
//! changing an implementation bumps the constant, which invalidates every
//! finding keyed on the old hash by construction (`ingest::findings` keys on
//! `hash(D)` alone).
//!
//! **Prepared forms per grain.** The path grains (`span` / `file`) hash their
//! bytes through [`crate::anchor::prepared_content_hash`] (the minimal
//! canonicalization: BOM, line endings, final newline). The `url` grain uses
//! the **same canonicalization over observation-supplied content** — the
//! engine never fetches, so whoever observed the URL supplies the bytes at
//! write time (`AnchorInput::content`) — and defaults to `hash_stability:
//! unstable`, a served page being a moving target. The `entity` grain's
//! prepared form is computed from the live graph, never from supplied bytes:
//! the canonical rendered markdown by default, or — under
//! [`ENTITY_LOAD_BEARING`] — the stable serialization of the type's
//! load-bearing sections.
//!
//! **Non-goal, by standing decision:** PDF / DOCX / audio conversion. An
//! agent with a capable read tool extracts; the raw-byte fallback of the
//! prepared-content hash already drift-detects a binary artifact.

use serde::{Deserialize, Serialize};

use crate::anchor::{AnchorGrain, AnchorHashStability, prepared_content_hash};
use crate::entity::Entity;

/// The engine touchpoint a registered preparation plugs into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Touchpoint {
    /// Touchpoint A: anchor observation asks the registry for the prepared
    /// form an artifact hashes as (content and code-map flavours).
    PreparedForm,
    /// Touchpoint B: the ingest delivery path asks the registry for a
    /// source's unit sequence (the delivery flavour). Reserved — no
    /// delivery preparation is registered yet.
    DeliveryUnits,
}

/// One registered preparation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preparation {
    /// The identifier a source declares (`Source::preparation`).
    pub id: &'static str,
    /// Which engine touchpoint consults it.
    pub touchpoint: Touchpoint,
    /// The anchor grains it produces a prepared form for. A binding may
    /// declare it only over a medium whose anchor namespace admits at least
    /// one of these grains (checked at binding validation).
    pub grains: &'static [AnchorGrain],
    /// One sentence for the operator and the refusal payloads.
    pub description: &'static str,
}

/// Content preparation on the `entity` grain: the prepared form is the stable
/// serialization of the entity type's **load-bearing sections** (see
/// [`load_bearing_sections`]), so a dependent's prepared hash breaks when a
/// load-bearing sentence changes and holds when a comma lands in the notes.
pub const ENTITY_LOAD_BEARING: &str = "entity-load-bearing";

/// The registry — every preparation this engine implements. The refusal in
/// [`crate::binding::validate_binding`] is exactly "not in this list".
pub const REGISTRY: &[Preparation] = &[Preparation {
    id: ENTITY_LOAD_BEARING,
    touchpoint: Touchpoint::PreparedForm,
    grains: &[AnchorGrain::Entity],
    description: "an entity's prepared form is the stable serialization of its type's \
                  load-bearing sections (explicitly declared, else the required sections, \
                  else every section) — notes-only edits keep dependents' anchors resolving",
}];

/// Every registered preparation.
pub fn registry() -> &'static [Preparation] {
    REGISTRY
}

/// Look a declared identifier up.
pub fn lookup(id: &str) -> Option<&'static Preparation> {
    REGISTRY.iter().find(|p| p.id == id)
}

/// Whether `id` names a registered preparation.
pub fn is_registered(id: &str) -> bool {
    lookup(id).is_some()
}

/// The registered identifiers, in registry order — the recovery payload of
/// the unknown-identifier refusal.
pub fn registered_identifiers() -> Vec<&'static str> {
    REGISTRY.iter().map(|p| p.id).collect()
}

/// Whether a registered preparation can apply over a medium whose anchor
/// namespace is `anchor_namespace` (see
/// [`crate::binding::medium_capabilities`]): at least one of the
/// preparation's grains must be expressible there. `entity-load-bearing`
/// over a `codebase` source would never meet an entity-grain anchor, so it
/// is refused at declaration rather than silently never applying.
pub fn applies_to_namespace(preparation: &Preparation, anchor_namespace: &str) -> bool {
    preparation
        .grains
        .iter()
        .any(|g| g.supported_by_namespace(anchor_namespace))
}

// ---------------------------------------------------------------------------
// Per-grain prepared forms
// ---------------------------------------------------------------------------

/// The medium-declared default hash stability per grain. A `url` anchor
/// defaults to `unstable` — a served page is a moving target, so a hash
/// break resolves `recheck`, never `drifted`, unless the author asserts
/// `stable` explicitly. Every other grain keeps its `stable` default.
pub fn default_hash_stability(grain: AnchorGrain) -> AnchorHashStability {
    match grain {
        AnchorGrain::Url => AnchorHashStability::Unstable,
        AnchorGrain::Span | AnchorGrain::File | AnchorGrain::Tree | AnchorGrain::Entity => {
            AnchorHashStability::Stable
        }
    }
}

/// The `url` grain's canonicalization entry: the prepared form of a URL
/// artifact is the observation-supplied content under the same minimal
/// canonicalization the path grains use, so a `url` anchor's recorded hash
/// means the same thing a `file` anchor's does. The engine never fetches;
/// the observer supplies the bytes.
pub fn url_prepared_hash(content: &[u8]) -> String {
    prepared_content_hash(content)
}

/// The prepared-content hash of **supplied** content for a grain — the
/// write-time observation an agent performs when it hands the engine what it
/// read (`AnchorInput::content`). `None` for a grain whose prepared form
/// is never computed from supplied bytes: `entity` (computed from the live
/// graph, so a supplied rendering could disagree with the store) and `tree`
/// (no prepared form — the recorded-but-unhashed residue the code-map
/// flavour closes).
pub fn supplied_content_hash(grain: AnchorGrain, content: &[u8]) -> Option<String> {
    match grain {
        AnchorGrain::Span | AnchorGrain::File => Some(prepared_content_hash(content)),
        AnchorGrain::Url => Some(url_prepared_hash(content)),
        AnchorGrain::Tree | AnchorGrain::Entity => None,
    }
}

/// The load-bearing sections of a type, in the type's declared order:
///
/// 1. the sections declaring `load_bearing: true`, when any does;
/// 2. otherwise the required sections, minus any declaring
///    `load_bearing: false`, when that leaves at least one;
/// 3. otherwise every section — a type with no required sections and no
///    declaration has no notes/claim split the engine can honour, and an
///    empty set would hash to a constant that never drifts.
pub fn load_bearing_sections(
    type_def: &memstead_schema::types::TypeDefinition,
) -> Vec<&memstead_schema::types::SectionDef> {
    let explicit: Vec<_> = type_def
        .sections
        .iter()
        .filter(|s| s.load_bearing == Some(true))
        .collect();
    if !explicit.is_empty() {
        return explicit;
    }
    let required: Vec<_> = type_def
        .sections
        .iter()
        .filter(|s| s.required && s.load_bearing != Some(false))
        .collect();
    if !required.is_empty() {
        return required;
    }
    type_def.sections.iter().collect()
}

/// The `entity-load-bearing` prepared form: the entity's load-bearing
/// sections serialized stably — each as `## <key>`, a blank line, the
/// trimmed content, a blank line — in the type's declared section order.
/// Keyed by section KEY (not heading) so a heading rename in the schema
/// does not read as a content change; a section the entity does not carry
/// is skipped. Title, metadata, and relationships are outside the form:
/// the anchor's artifact is the entity id, and a rename orphans the anchor
/// on its own. Without a type definition (a type the mem's schema does not
/// declare) every section the entity carries is load-bearing, in the
/// entity's own order.
pub fn entity_load_bearing_form(
    entity: &Entity,
    type_def: Option<&memstead_schema::types::TypeDefinition>,
) -> String {
    fn push(out: &mut String, key: &str, content: &str) {
        out.push_str("## ");
        out.push_str(key);
        out.push_str("\n\n");
        out.push_str(content.trim());
        out.push_str("\n\n");
    }
    let mut out = String::new();
    match type_def {
        Some(td) => {
            for section in load_bearing_sections(td) {
                if let Some(content) = entity.sections.get(&section.key) {
                    push(&mut out, &section.key, content);
                }
            }
        }
        None => {
            for (key, content) in &entity.sections {
                push(&mut out, key, content);
            }
        }
    }
    out
}

/// Touchpoint A for the `entity` grain: the prepared-content hash of an
/// entity under the source's declared preparation. `None` declares
/// nothing — the canonical rendered markdown, byte-for-byte today's form.
/// [`ENTITY_LOAD_BEARING`] hashes [`entity_load_bearing_form`]. An
/// identifier the registry does not know yields `None`: the form cannot be
/// computed, and observation reports the anchor unobserved rather than
/// hashing a fabricated form (validation refuses such a record at every
/// edit path; only a hand-edited file reaches here).
pub fn entity_prepared_hash(
    entity: &Entity,
    type_def: Option<&memstead_schema::types::TypeDefinition>,
    preparation: Option<&str>,
) -> Option<String> {
    let form = match preparation {
        None => crate::render::render_entity_markdown(entity, None),
        Some(ENTITY_LOAD_BEARING) => entity_load_bearing_form(entity, type_def),
        Some(_) => return None,
    };
    Some(prepared_content_hash(form.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityId;
    use indexmap::IndexMap;
    use memstead_schema::types::{SectionDef, TypeDefinition};

    fn section(key: &str, required: bool, load_bearing: Option<bool>) -> SectionDef {
        let mut v = serde_json::json!({
            "key": key, "heading": key, "required": required, "search_weight": 1.0
        });
        if let Some(lb) = load_bearing {
            v["load_bearing"] = serde_json::json!(lb);
        }
        serde_json::from_value(v).unwrap()
    }

    /// A real builtin type with its sections replaced — the fixture never
    /// has to track `TypeDefinition`'s required-field roster.
    fn type_with(sections: Vec<SectionDef>) -> TypeDefinition {
        let schemas = memstead_schema::builtins::load_builtin_schemas().unwrap();
        let base = schemas
            .iter()
            .find_map(|s| s.get_type("assertion"))
            .expect("a builtin schema declares `assertion`");
        let mut td = (*base).clone();
        td.sections = sections;
        td
    }

    fn entity(sections: &[(&str, &str)]) -> Entity {
        let mut map = IndexMap::new();
        for (k, v) in sections {
            map.insert(k.to_string(), v.to_string());
        }
        Entity {
            id: EntityId::canonical("m--e"),
            title: "E".into(),
            entity_type: "t".into(),
            mem: "m".into(),
            file_path: "e.md".into(),
            metadata: IndexMap::new(),
            sections: map,
            relationships: Vec::new(),
            content_hash: "h".into(),
            stub: false,
            stub_kind: None,
            heading_spans: Default::default(),
            raw_section_headings: Vec::new(),
        }
    }

    #[test]
    fn registry_knows_its_one_content_flavour_and_nothing_else() {
        assert!(is_registered(ENTITY_LOAD_BEARING));
        assert!(!is_registered("pdf-to-markdown"));
        assert!(!is_registered(""));
        assert_eq!(registered_identifiers(), vec![ENTITY_LOAD_BEARING]);
        let p = lookup(ENTITY_LOAD_BEARING).unwrap();
        assert_eq!(p.touchpoint, Touchpoint::PreparedForm);
        assert!(applies_to_namespace(p, "entity"));
        assert!(!applies_to_namespace(p, "path"));
        assert!(!applies_to_namespace(p, "url"));
    }

    #[test]
    fn url_defaults_unstable_every_other_grain_stable() {
        assert_eq!(
            default_hash_stability(AnchorGrain::Url),
            AnchorHashStability::Unstable
        );
        for g in [
            AnchorGrain::Span,
            AnchorGrain::File,
            AnchorGrain::Tree,
            AnchorGrain::Entity,
        ] {
            assert_eq!(default_hash_stability(g), AnchorHashStability::Stable);
        }
    }

    /// The url grain's prepared form IS the path grains' canonicalization:
    /// same bytes, same hash, and the same noise (CRLF, BOM, final newline)
    /// is invisible.
    #[test]
    fn url_prepared_form_is_the_shared_canonicalization() {
        let a = url_prepared_hash(b"<p>hello</p>\n");
        assert_eq!(a, prepared_content_hash(b"<p>hello</p>\n"));
        assert_eq!(a, url_prepared_hash(b"\xEF\xBB\xBF<p>hello</p>\r\n\r\n"));
        assert_ne!(a, url_prepared_hash(b"<p>hello!</p>\n"));
        assert_eq!(
            supplied_content_hash(AnchorGrain::Url, b"<p>hello</p>").as_deref(),
            Some(a.as_str())
        );
        assert!(supplied_content_hash(AnchorGrain::File, b"x").is_some());
        assert!(supplied_content_hash(AnchorGrain::Span, b"x").is_some());
        assert!(supplied_content_hash(AnchorGrain::Tree, b"x").is_none());
        assert!(supplied_content_hash(AnchorGrain::Entity, b"x").is_none());
    }

    #[test]
    fn load_bearing_resolves_explicit_then_required_then_all() {
        let explicit = type_with(vec![
            section("claim", true, Some(true)),
            section("evidence", true, Some(false)),
            section("notes", false, None),
        ]);
        let keys: Vec<_> = load_bearing_sections(&explicit)
            .iter()
            .map(|s| s.key.as_str())
            .collect();
        assert_eq!(keys, vec!["claim"]);

        let required = type_with(vec![
            section("claim", true, None),
            section("evidence", true, Some(false)),
            section("notes", false, None),
        ]);
        let keys: Vec<_> = load_bearing_sections(&required)
            .iter()
            .map(|s| s.key.as_str())
            .collect();
        assert_eq!(
            keys,
            vec!["claim"],
            "a required section opted out is excluded"
        );

        let none = type_with(vec![section("a", false, None), section("b", false, None)]);
        let keys: Vec<_> = load_bearing_sections(&none)
            .iter()
            .map(|s| s.key.as_str())
            .collect();
        assert_eq!(keys, vec!["a", "b"], "no declaration at all: every section");
    }

    /// The anker metric, mechanised: a notes-only edit leaves the prepared
    /// hash intact; a load-bearing edit breaks it.
    #[test]
    fn notes_edit_keeps_the_hash_load_bearing_edit_breaks_it() {
        let td = type_with(vec![
            section("decision", true, None),
            section("notes", false, None),
        ]);
        let base = entity(&[("decision", "We ship."), ("notes", "first draft")]);
        let notes_edit = entity(&[("decision", "We ship."), ("notes", "first draft, revised")]);
        let claim_edit = entity(&[("decision", "We do not ship."), ("notes", "first draft")]);
        let h = |e: &Entity| entity_prepared_hash(e, Some(&td), Some(ENTITY_LOAD_BEARING)).unwrap();
        assert_eq!(h(&base), h(&notes_edit));
        assert_ne!(h(&base), h(&claim_edit));

        // The default form (no preparation) sees BOTH edits — today's
        // behaviour, byte-for-byte the canonical rendered markdown.
        let d = |e: &Entity| entity_prepared_hash(e, Some(&td), None).unwrap();
        assert_ne!(d(&base), d(&notes_edit));
        assert_eq!(
            d(&base),
            prepared_content_hash(crate::render::render_entity_markdown(&base, None).as_bytes())
        );

        // An unregistered identifier computes nothing.
        assert!(entity_prepared_hash(&base, Some(&td), Some("pdf-to-markdown")).is_none());
    }

    /// Content moving between two load-bearing sections changes the form
    /// (keys are part of it); trailing whitespace inside a section does not.
    #[test]
    fn form_is_keyed_and_trimmed() {
        let td = type_with(vec![
            section("claim", true, None),
            section("evidence", true, None),
        ]);
        let a = entity(&[("claim", "x"), ("evidence", "y")]);
        let b = entity(&[("claim", "y"), ("evidence", "x")]);
        let c = entity(&[("claim", "x  \n\n"), ("evidence", "\n y")]);
        let form = |e: &Entity| entity_load_bearing_form(e, Some(&td));
        assert_ne!(form(&a), form(&b));
        assert_eq!(form(&a), form(&c));
        assert_eq!(form(&a), "## claim\n\nx\n\n## evidence\n\ny\n\n");
        // No type definition: every section the entity carries, its order.
        assert_eq!(
            entity_load_bearing_form(&entity(&[("z", "1"), ("a", "2")]), None),
            "## z\n\n1\n\n## a\n\n2\n\n"
        );
    }
}
