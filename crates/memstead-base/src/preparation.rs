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
//!   registry for a source's unit sequence ([`unitize`]): one file can carry
//!   many delivery units, addressed `<path>#<key>`, and the units of a whole
//!   source form one deterministic total order derived from the units' own
//!   keys ([`Touchpoint::DeliveryUnits`], first entry [`DATED_ENTRIES`]).
//!   A source declaring no delivery preparation keeps file-granularity
//!   delivery unchanged.
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
    /// source's unit sequence (the delivery flavour).
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

/// Delivery preparation on path-shaped sources: a file is a sequence of
/// **dated entries**. A unit begins at every line that opens with an ISO
/// date or date-time (`2026-08-24`, `2026-08-24 10:05`, `2026-08-24T10:05:00Z`,
/// after any leading markdown markers such as `## `, `- `, `> `, `[`); it
/// runs to the next such line. Text before the first entry folds into the
/// first unit; a file with no dated line is one unit keyed
/// [`WHOLE_FILE_UNIT`]. The unit key is the stamp normalized to
/// `YYYY-MM-DDTHH:MM:SS` (missing time parts read `00`), with `.2`, `.3`, …
/// appended to the second, third, … entry carrying the same stamp in one
/// file, in file order — so appending entries never renames an existing
/// unit. The order key is the normalized stamp; across a whole source, units
/// sort by stamp, then path, then key, which is what makes a chronological
/// corpus deliver in its own order regardless of how files were discovered.
/// Undated files (order key empty) come first, in path order. Fractional
/// seconds and zone designators are accepted and ignored for ordering.
pub const DATED_ENTRIES: &str = "dated-entries";

/// The key of the single unit a file yields when a delivery preparation finds
/// no unit boundary in it — the whole file, still addressable as
/// `<path>#whole`.
pub const WHOLE_FILE_UNIT: &str = "whole";

/// The registry — every preparation this engine implements. The refusal in
/// [`crate::binding::validate_binding`] is exactly "not in this list".
pub const REGISTRY: &[Preparation] = &[
    Preparation {
        id: ENTITY_LOAD_BEARING,
        touchpoint: Touchpoint::PreparedForm,
        grains: &[AnchorGrain::Entity],
        description: "an entity's prepared form is the stable serialization of its type's \
                      load-bearing sections (explicitly declared, else the required sections, \
                      else every section) — notes-only edits keep dependents' anchors resolving",
    },
    Preparation {
        id: DATED_ENTRIES,
        touchpoint: Touchpoint::DeliveryUnits,
        grains: &[AnchorGrain::Span],
        description: "a file is a sequence of entries opening with an ISO date or date-time; \
                      each entry is one delivery unit `<path>#<stamp>`, and a source's units \
                      deliver in stamp order, identical on every pass — a chronological corpus \
                      (logs, transcripts, journals, mail threads) is never shuffled",
    },
];

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

/// The delivery preparation a source declares, if its declared identifier
/// is a registered touchpoint-B entry — `None` for no declaration, an
/// unregistered identifier, or a prepared-form (touchpoint A) flavour.
pub fn delivery_preparation(declared: Option<&str>) -> Option<&'static Preparation> {
    lookup(declared?).filter(|p| p.touchpoint == Touchpoint::DeliveryUnits)
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

// ---------------------------------------------------------------------------
// Touchpoint B: delivery units
// ---------------------------------------------------------------------------

/// One delivery unit of a file under a delivery preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryUnit {
    /// The unit's key, unique within its file; the addressed form is
    /// `<path>#<key>` ([`unit_id`]).
    pub key: String,
    /// The intrinsic key the source's units sort by (a normalized stamp for
    /// [`DATED_ENTRIES`]); empty for a [`WHOLE_FILE_UNIT`].
    pub order_key: String,
    /// First line of the unit, 1-based.
    pub start_line: usize,
    /// Last line of the unit, 1-based, inclusive.
    pub end_line: usize,
    /// The prepared-content hash of the unit's text — what a span anchor
    /// over the unit records, and what a change run compares.
    pub hash: String,
}

/// How a unit changed between two states of its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnitChange {
    /// The key is new.
    Added,
    /// The key existed; the unit's text changed.
    Modified,
    /// The key is gone.
    Deleted,
}

/// The addressed form of a unit: `<path>#<key>`.
pub fn unit_id(path: &str, key: &str) -> String {
    format!("{path}#{key}")
}

/// Split an artifact id into its path and, when it addresses a unit, the
/// unit key after the first `#`.
pub fn split_unit_id(id: &str) -> (&str, Option<&str>) {
    match id.find('#') {
        Some(cut) => (&id[..cut], Some(&id[cut + 1..])),
        None => (id, None),
    }
}

/// Touchpoint B: the delivery units of one file's content under a delivery
/// preparation, in file order. `None` when `preparation` is not a registered
/// delivery preparation (the caller keeps file-granularity delivery).
pub fn unitize(preparation: &str, content: &str) -> Option<Vec<DeliveryUnit>> {
    match preparation {
        DATED_ENTRIES => Some(dated_entries(content)),
        _ => None,
    }
}

/// The text of one unit, lines `start_line..=end_line` of `content`.
pub fn unit_text(content: &str, unit: &DeliveryUnit) -> String {
    content
        .lines()
        .skip(unit.start_line.saturating_sub(1))
        .take(unit.end_line + 1 - unit.start_line.max(1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The units that differ between two states of one file, keyed by unit key:
/// a key only in `after` is [`UnitChange::Added`], a key in both whose hash
/// differs is [`UnitChange::Modified`] (the `after` unit), a key only in
/// `before` is [`UnitChange::Deleted`] (the `before` unit, so its order key
/// still places it). Unchanged units are not delivered again.
pub fn diff_units(
    before: &[DeliveryUnit],
    after: &[DeliveryUnit],
) -> Vec<(DeliveryUnit, UnitChange)> {
    let old: std::collections::BTreeMap<&str, &DeliveryUnit> =
        before.iter().map(|u| (u.key.as_str(), u)).collect();
    let new: std::collections::BTreeMap<&str, &DeliveryUnit> =
        after.iter().map(|u| (u.key.as_str(), u)).collect();
    let mut out = Vec::new();
    for u in after {
        match old.get(u.key.as_str()) {
            None => out.push((u.clone(), UnitChange::Added)),
            Some(prev) if prev.hash != u.hash => out.push((u.clone(), UnitChange::Modified)),
            Some(_) => {}
        }
    }
    for u in before {
        if !new.contains_key(u.key.as_str()) {
            out.push((u.clone(), UnitChange::Deleted));
        }
    }
    out
}

fn dated_entries(content: &str) -> Vec<DeliveryUnit> {
    let lines: Vec<&str> = content.lines().collect();
    let starts: Vec<(usize, String)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| leading_stamp(line).map(|stamp| (i, stamp)))
        .collect();
    if starts.is_empty() {
        return vec![DeliveryUnit {
            key: WHOLE_FILE_UNIT.to_string(),
            order_key: String::new(),
            start_line: 1,
            end_line: lines.len().max(1),
            hash: prepared_content_hash(content.as_bytes()),
        }];
    }
    let mut seen: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut units = Vec::with_capacity(starts.len());
    for (n, (start, stamp)) in starts.iter().enumerate() {
        // The preamble (anything before the first stamp) folds into the
        // first unit: it is context for the entries, never a unit of its own.
        let from = if n == 0 { 0 } else { *start };
        let to = starts.get(n + 1).map_or(lines.len(), |(next, _)| *next);
        let text = lines[from..to].join("\n");
        let count = seen
            .entry(stamp.clone())
            .and_modify(|c| *c += 1)
            .or_insert(1);
        let key = if *count == 1 {
            stamp.clone()
        } else {
            format!("{stamp}.{count}")
        };
        units.push(DeliveryUnit {
            key,
            order_key: stamp.clone(),
            start_line: from + 1,
            end_line: to,
            hash: prepared_content_hash(text.as_bytes()),
        });
    }
    units
}

/// The ISO stamp a line opens with (after leading markdown markers),
/// normalized to `YYYY-MM-DDTHH:MM:SS`; `None` when the line opens with
/// anything else or the stamp is out of range.
fn leading_stamp(line: &str) -> Option<String> {
    static STAMP: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = STAMP.get_or_init(|| {
        regex::Regex::new(
            r"^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2})(?::(\d{2}))?(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?)?\b",
        )
        .expect("the stamp regex compiles")
    });
    let s = line.trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, '#' | '-' | '*' | '>' | '[' | '(' | '|' | '`' | '+')
    });
    let caps = re.captures(s)?;
    let num = |i: usize| -> u32 {
        caps.get(i)
            .map(|m| m.as_str().parse().unwrap_or(0))
            .unwrap_or(0)
    };
    let (y, mo, d, h, mi, sec) = (num(1), num(2), num(3), num(4), num(5), num(6));
    let days_in_month = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 29,
        _ => return None,
    };
    if !(1..=days_in_month).contains(&d) || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    Some(format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{sec:02}"))
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
    fn registry_knows_its_two_flavours_and_nothing_else() {
        assert!(is_registered(ENTITY_LOAD_BEARING));
        assert!(is_registered(DATED_ENTRIES));
        assert!(!is_registered("pdf-to-markdown"));
        assert!(!is_registered(""));
        assert_eq!(
            registered_identifiers(),
            vec![ENTITY_LOAD_BEARING, DATED_ENTRIES]
        );
        let p = lookup(ENTITY_LOAD_BEARING).unwrap();
        assert_eq!(p.touchpoint, Touchpoint::PreparedForm);
        assert!(applies_to_namespace(p, "entity"));
        assert!(!applies_to_namespace(p, "path"));
        assert!(!applies_to_namespace(p, "url"));
        let d = lookup(DATED_ENTRIES).unwrap();
        assert_eq!(d.touchpoint, Touchpoint::DeliveryUnits);
        assert!(applies_to_namespace(d, "path"));
        assert!(applies_to_namespace(d, "path+commit"));
        assert!(!applies_to_namespace(d, "entity"));
        assert!(!applies_to_namespace(d, "url"));
        // Touchpoint B lookup: only a delivery flavour answers.
        assert_eq!(
            delivery_preparation(Some(DATED_ENTRIES)).map(|p| p.id),
            Some(DATED_ENTRIES)
        );
        assert!(delivery_preparation(Some(ENTITY_LOAD_BEARING)).is_none());
        assert!(delivery_preparation(Some("pdf-to-markdown")).is_none());
        assert!(delivery_preparation(None).is_none());
        assert!(unitize(ENTITY_LOAD_BEARING, "x").is_none());
        assert!(unitize("pdf-to-markdown", "x").is_none());
    }

    const LOG: &str = "# Ops log\n\nPreamble text.\n\n## 2026-08-24 10:05 boot\nline a\n\n\
                       - 2026-08-24T10:05:00Z boot again\nline b\n2026-08-25 shutdown\nline c\n";

    /// Unitization: entries open at dated lines, the preamble folds into the
    /// first unit, same-stamp entries get an ordinal, an undated file is one
    /// `whole` unit, and the stamp normalizes across the accepted spellings.
    #[test]
    fn dated_entries_unitize_deterministically() {
        let units = unitize(DATED_ENTRIES, LOG).unwrap();
        let keys: Vec<&str> = units.iter().map(|u| u.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "2026-08-24T10:05:00",
                "2026-08-24T10:05:00.2",
                "2026-08-25T00:00:00"
            ]
        );
        assert_eq!(
            units[0].start_line, 1,
            "the preamble folds into the first unit"
        );
        assert_eq!((units[0].end_line, units[1].start_line), (7, 8));
        assert_eq!(units[2].end_line, 11);
        assert_eq!(units[1].order_key, "2026-08-24T10:05:00");
        assert!(unit_text(LOG, &units[2]).starts_with("2026-08-25 shutdown"));
        assert_eq!(
            units[2].hash,
            prepared_content_hash(unit_text(LOG, &units[2]).as_bytes())
        );

        let whole = unitize(DATED_ENTRIES, "no stamps here\njust prose\n").unwrap();
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].key, WHOLE_FILE_UNIT);
        assert_eq!(whole[0].order_key, "");

        assert_eq!(
            leading_stamp("[2026-02-30] bad day"),
            None,
            "day out of range"
        );
        assert_eq!(
            leading_stamp("2026-08-24T25:00 x"),
            None,
            "hour out of range"
        );
        assert_eq!(leading_stamp("v2026-08-24"), None, "not at the line start");
        assert_eq!(leading_stamp("2026-08-2400"), None, "digits run on");
        assert_eq!(
            leading_stamp("> **2026-08-24T10:05:00.250+02:00** note").as_deref(),
            Some("2026-08-24T10:05:00")
        );
        assert_eq!(
            unit_id("logs/ops.md", "2026-08-25T00:00:00"),
            "logs/ops.md#2026-08-25T00:00:00"
        );
        assert_eq!(
            split_unit_id("logs/ops.md#2026-08-25T00:00:00"),
            ("logs/ops.md", Some("2026-08-25T00:00:00"))
        );
        assert_eq!(split_unit_id("logs/ops.md"), ("logs/ops.md", None));
    }

    /// Keys are stable under growth: appending entries leaves every existing
    /// unit's key and hash untouched, so a change run delivers only the new
    /// unit; an edited entry delivers as modified, a removed one as deleted.
    #[test]
    fn unit_keys_survive_growth_and_diff_delivers_only_what_changed() {
        let before = unitize(DATED_ENTRIES, LOG).unwrap();
        let grown = format!("{LOG}2026-08-26 09:00 restart\nline d\n");
        let after = unitize(DATED_ENTRIES, &grown).unwrap();
        assert_eq!(
            &after[..3],
            &before[..],
            "existing units are byte-identical"
        );
        let delta = diff_units(&before, &after);
        assert_eq!(delta.len(), 1);
        assert_eq!(delta[0].0.key, "2026-08-26T09:00:00");
        assert_eq!(delta[0].1, UnitChange::Added);

        let edited = LOG.replace("line c", "line c, revised");
        let delta = diff_units(&before, &unitize(DATED_ENTRIES, &edited).unwrap());
        assert_eq!(
            delta
                .iter()
                .map(|(u, c)| (u.key.as_str(), *c))
                .collect::<Vec<_>>(),
            vec![("2026-08-25T00:00:00", UnitChange::Modified)]
        );

        let shrunk = LOG.replace("2026-08-25 shutdown\nline c\n", "");
        let delta = diff_units(&before, &unitize(DATED_ENTRIES, &shrunk).unwrap());
        assert_eq!(
            delta
                .iter()
                .map(|(u, c)| (u.key.as_str(), *c))
                .collect::<Vec<_>>(),
            vec![("2026-08-25T00:00:00", UnitChange::Deleted)]
        );
        assert!(diff_units(&before, &before).is_empty());
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
