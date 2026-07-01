//! Shared helper for turning `ParseResult`s into a populated `Store`.
//!
//! The runtime engine calls this during `Engine::init` + `reload` and
//! `attach_read_vault`. The strict validator calls it during V1 graph
//! construction. Having one implementation guarantees both paths use
//! identical stub + edge semantics.

use indexmap::IndexMap;
use memstead_schema::TypeDefinition;

use super::parser::extract_inline_links_lenient;
use super::{Entity, EntityId, ParseResult};
use crate::ops::WarningHint;
use crate::store::{Edge, EdgeSource, Store};

/// Context passed to `push_entities_into_store` for load-time drift
/// detection. Load-path call sites pass `Some(LoadCollector { .. })` so
/// authored nested-prefix wiki-links (the classic vault-rename drift
/// footprint) emit a `SuspiciousNestedPrefix` warning — mutation-path
/// call sites pass `None` to stay silent (an author editing an entity
/// that still has a drifted link should not see the warning refire on
/// every save; load already caught it).
pub struct LoadCollector<'a> {
    /// Target for emitted warnings — typically `&mut engine.load_warnings`.
    pub warnings: &'a mut Vec<WarningHint>,
    /// Known-vault last-segment suffixes, derived from the vault roster
    /// (e.g. `test-vault-plugin` → `plugin`). A nested-prefix link
    /// is detected when a target id has the shape
    /// `<current-vault>--<suffix>--<rest>` where `<suffix>` is in this
    /// set and `<suffix>` is not the entity's own vault's last segment.
    pub known_suffixes: &'a [String],
    /// Full vault-name roster (writable + read vaults). Used by the
    /// two-pass candidate resolver to probe cross-vault matches.
    pub vault_names: &'a [String],
}

/// Upsert parse results into the store, adding explicit relationship
/// edges and auto-stubbing any unknown targets. Body wiki-links are
/// not edge sources — under the alias model every edge originates
/// from the auto-managed `## Relationships` section.
///
/// The fallback schema parameter is retained for call-site compatibility
/// but no longer consulted for edge emission.
///
/// `load_ctx` is `Some` at load/reload/attach sites (drift-warning
/// emission enabled) and `None` at mutation/validator sites (silent —
/// warnings fire once at load, not on every edit).
pub fn push_entities_into_store(
    store: &mut Store,
    parse_results: Vec<ParseResult>,
    _fallback_schema: &TypeDefinition,
    mut load_ctx: Option<LoadCollector<'_>>,
) {
    // Stash id + vault + sections per entity for a post-upsert drift
    // scan. We can't scan before upsert: the two-pass resolver needs
    // ALL entities in the batch to be present so a bare-slug fallback
    // can find intra-batch targets regardless of filesystem iteration
    // order (e.g. `drifted.md` loaded before its `foo.md` sibling).
    let mut drift_scan_inputs: Vec<(EntityId, String, IndexMap<String, String>)> = Vec::new();

    for parse_result in parse_results {
        let entity_id = parse_result.entity.id.clone();
        let entity_vault = parse_result.entity.vault.clone();

        // Surface parse-time warnings (e.g. duplicate section headings) at
        // load / reload / attach sites. Mutation paths build their own
        // `ParseResult`s without `load_ctx` and ignore these.
        if let Some(ctx) = load_ctx.as_mut()
            && !parse_result.parse_warnings.is_empty()
        {
            ctx.warnings.extend(parse_result.parse_warnings.iter().cloned());
        }

        if load_ctx.is_some() {
            drift_scan_inputs.push((
                entity_id.clone(),
                entity_vault,
                parse_result.entity.sections.clone(),
            ));
        }

        // Clear pre-existing out-edges before upserting so the store
        // reflects exactly the new entity's relationships. Without this,
        // a mutation that drops a relation leaks the stale edge:
        // `add_edge` is idempotent on (from, to, rel_type), so it never
        // removes edges that the post-parse pass no longer emits.
        store.remove_edges_from(&entity_id);

        store.upsert(entity_id.clone(), parse_result.entity);

        let relationships: Vec<_> = store
            .get(&entity_id)
            .map(|e| e.relationships.clone())
            .unwrap_or_default();
        for rel in &relationships {
            if !store.contains(&rel.target) {
                store.upsert(rel.target.clone(), make_stub(rel.target.clone()));
            }
            store.add_edge(
                entity_id.clone(),
                Edge {
                    rel_type: rel.rel_type.clone(),
                    target: rel.target.clone(),
                    source: EdgeSource::Explicit,
                },
            );
        }
    }

    // Post-upsert drift scan — every batch entity is now in the store,
    // so pass-2 (same-vault bare-slug) finds intra-batch targets too.
    if let Some(ctx) = load_ctx.as_mut() {
        for (id, vault, sections) in &drift_scan_inputs {
            scan_nested_prefix_drift(id, vault, sections, ctx, store);
        }
    }
}

/// Re-add edges that point INTO `reloaded_vault` from entities living in
/// other vaults, after a per-vault reload of `reloaded_vault`.
///
/// The per-vault removal cascade ([`Store::remove`] via
/// [`Store::remove_entities_by_vault`]) drops every incoming mirror of the
/// reloaded vault's nodes — including cross-vault edges sourced from an
/// un-reloaded vault — and the re-push ([`push_entities_into_store`]) only
/// rebuilds edges authored by the reloaded vault's own entities. So a
/// cross-vault edge `A→B` (A in another vault) survives in A's record and
/// on disk but vanishes from the in-memory adjacency until a workspace-wide
/// reload rebuilds A's side. This pass restores it from the authoritative
/// source records, so a per-vault reload of B and a workspace-wide reload
/// converge to the same incoming adjacency for B.
///
/// Mirrors `push_entities_into_store`'s edge construction exactly: auto-stub
/// a missing target and add the edge as `EdgeSource::Explicit`. A following
/// [`remap_alias_target_edge_sources`] reclassifies alias-derived sources
/// (the same post-pass the reload already runs over the re-pushed vault),
/// so an alias/body-link cross-vault edge keeps its `BodyLink` source. The
/// scan is over in-memory records only — it never re-reads or re-parses
/// another vault's backend, preserving the cheap-per-vault-reload property.
pub fn reconstruct_incoming_cross_vault_edges(store: &mut Store, reloaded_vault: &str) {
    let mut to_add: Vec<(EntityId, Edge)> = Vec::new();
    for entity in store.all_entities() {
        if entity.vault == reloaded_vault {
            continue;
        }
        for rel in &entity.relationships {
            if rel.target.vault() == reloaded_vault {
                to_add.push((
                    entity.id.clone(),
                    Edge {
                        rel_type: rel.rel_type.clone(),
                        target: rel.target.clone(),
                        source: EdgeSource::Explicit,
                    },
                ));
            }
        }
    }
    for (from, edge) in to_add {
        if !store.contains(&edge.target) {
            store.upsert(edge.target.clone(), make_stub(edge.target.clone()));
        }
        store.add_edge(from, edge);
    }
}

/// Extract the last `-`-separated segment of a vault name (e.g.
/// `test-vault-plugin` → `plugin`). Used to derive the
/// known-vault-suffix set from the roster.
pub fn last_segment_suffix(vault_name: &str) -> &str {
    vault_name.rsplit('-').next().unwrap_or(vault_name)
}

/// Scan an entity's section bodies for wiki-links whose vault prefix
/// matches a known vault last-segment but is NOT the full vault name —
/// i.e. the author wrote the short-form (`[[plugin--foo]]`) instead of
/// the bare-slug form (same-vault target) or the canonical
/// fully-qualified form. Each hit produces a `SuspiciousNestedPrefix`
/// warning with a two-pass resolved candidate.
///
/// Tier-0 `<vault>--<slug>` recognition resolves the body link to the
/// named vault directly. A known short-name being used where a bare
/// slug or a fully-qualified id was intended is the canonical drift
/// pattern; the detector matches on the resolved target's vault.
/// Runs before the entity is upserted so the candidate probe reflects
/// the store state *before* this entity's own auto-stub would mask a
/// real intra-vault match.
fn scan_nested_prefix_drift(
    from: &EntityId,
    current_vault: &str,
    sections: &IndexMap<String, String>,
    ctx: &mut LoadCollector<'_>,
    store: &Store,
) {
    for (section, body) in sections {
        // Reuse the same extractor DanglingLink uses so semantics stay
        // aligned (code-block masking, inline-code skipping, alias handling).
        for target_id in extract_inline_links_lenient(body, current_vault) {
            let target_vault = target_id.vault();
            // Skip when the body link resolves into the current vault —
            // bare-slug authoring is the canonical same-vault form, no
            // drift to surface.
            if target_vault == current_vault {
                continue;
            }
            // A colon/dash link whose target vault is itself a full
            // roster member AND whose target entity actually exists is a
            // legitimate cross-vault reference, not drift: pass-1 of the
            // two-pass resolver would just rediscover the same id, so the
            // warning's "did you mean" candidate equals the already-
            // resolved target — a self-contradicting false positive (the
            // macos→engine case). Skip it. A real-vault target whose
            // entity is *missing* is left to fire (it may be genuine
            // rename-drift where a suffix-sibling vault holds the real
            // entity — see `suffix_collision_resolves_first_match`).
            if ctx.vault_names.iter().any(|v| v.as_str() == target_vault)
                && store.get(&target_id).is_some_and(|e| !e.stub)
            {
                continue;
            }
            // Fire when the target's vault matches a known last-segment
            // suffix of some vault in the roster. Self-suffix is NOT
            // excluded — `[[plugin--x]]` inside `test-vault-plugin`
            // (suffix `plugin`, with no `plugin` vault) remains the
            // empirically-dominant drift pattern.
            for suffix in ctx.known_suffixes.iter() {
                if target_vault == suffix {
                    let candidate_target = resolve_two_pass(
                        target_id.path(),
                        current_vault,
                        ctx.vault_names,
                        store,
                    );
                    ctx.warnings.push(WarningHint::SuspiciousNestedPrefix {
                        from: from.clone(),
                        resolved_id: target_id.clone(),
                        candidate_target,
                        section: section.clone(),
                    });
                    break;
                }
            }
        }
    }
}

/// Two-pass resolver for a stripped slug (the `<rest>` part of a
/// nested-prefix drift hit).
///
/// Pass 1 (cross-vault-first): probe `<V>--<rest>` against every
/// non-current vault in the roster. If exactly one match resolves to a
/// real entity, the author probably meant that cross-vault entity.
///
/// Pass 2 (same-vault bare-slug): if no unique cross-vault match,
/// probe `<current_vault>--<rest>`. If that resolves to a real entity,
/// the author probably meant the bare slug form in the current vault.
///
/// Returns `None` on zero hits, multiple cross-vault hits (ambiguous),
/// or when the matched candidate is a stub. Callers surface the
/// `None` case so the author can disambiguate by hand — the warning
/// still fires.
fn resolve_two_pass(
    rest: &str,
    current_vault: &str,
    vault_names: &[String],
    store: &Store,
) -> Option<EntityId> {
    let mut hits: Vec<EntityId> = Vec::new();
    for vault in vault_names {
        if vault == current_vault {
            continue;
        }
        let candidate = EntityId::new(vault, rest);
        if let Some(e) = store.get(&candidate)
            && !e.stub
        {
            hits.push(candidate);
        }
    }
    match hits.len() {
        1 => hits.pop(),
        0 => {
            // Pass 2: same-vault bare-slug fallback.
            let candidate = EntityId::new(current_vault, rest);
            if let Some(e) = store.get(&candidate)
                && !e.stub
            {
                Some(candidate)
            } else {
                None
            }
        }
        _ => None, // ambiguous cross-vault match
    }
}

/// Validate every loaded entity's `## Relationships` entries against
/// the source vault's schema and the wiki-link grammar. Invalid
/// relations are dropped from both the store's edge index and the
/// entity's in-memory `relationships` list; each drop emits a
/// `PARSED_RELATION_INVALID` warning naming the offending entity,
/// rel-type, target, and reason.
///
/// Four reasons fire today:
/// - `grammar` — the target id's path does not match the wiki-link
///   grammar (`^[a-z0-9-]+(/[a-z0-9-]+)*$`).
/// - `unknown_rel_type` — the rel-type is not declared in the vault's
///   schema and the schema is in `strict` mode. Open-mode schemas
///   admit the relation without a warning (mirrors the mutation
///   surface).
/// - `shape` — the `(source_type, target_type)` pair is not allowed
///   by the declared `source_types` / `target_types`. `target_type`
///   is looked up from the store post-load, so the check sees the
///   real type for any target — including cross-vault targets
///   loaded from another mount. Stub targets (no `entity_type`) skip
///   the target-side check; the relation lands and the shape will be
///   re-verified when the stub is promoted to a real entity.
/// - `cycle` — the relation closes a cycle in an `acyclic: true`
///   rel-type's subgraph. Emitted by the second pass after grammar /
///   rel-type / shape drops; the two-pass structure runs cycle
///   detection after the initial relation-load so loading order
///   doesn't determine which edge
///   gets blamed. Each cycle drops exactly one back-edge per DFS
///   visit; multiple independent cycles each lose one edge.
///
/// Runs once at boot after every mount's entities are pushed into the
/// store. Mutation paths do not call this — they pre-validate via
/// `validate_rel_type` + `validate_rel_shape` before the write and
/// the existing same-call `would_cycle` check guards acyclic adds.
pub fn validate_loaded_relations(
    store: &mut Store,
    schemas: &std::collections::HashMap<String, std::sync::Arc<memstead_schema::Schema>>,
    mount_caps: &std::collections::HashMap<String, crate::workspace::MountCapability>,
    warnings: &mut Vec<WarningHint>,
) {
    use crate::entity::Relationship;
    use crate::entity::id::validate_id_path_grammar;
    use crate::runtime_validator::{
        CrossVaultRelCheck, validate_cross_vault_edge, validate_rel_shape, validate_rel_type,
    };
    use crate::workspace::MountCapability;
    use memstead_schema::SchemaRef;

    let origin_for = |vault: &str| -> &'static str {
        match mount_caps.get(vault) {
            Some(MountCapability::ReadOnly) => "readonly",
            _ => "writable",
        }
    };

    // Pass 1: schema-shape + grammar + rel-type-known drops.
    let mut to_drop: Vec<(EntityId, Relationship, &'static str)> = Vec::new();
    for entity in store.all_entities() {
        if entity.stub {
            continue;
        }
        let Some(schema) = schemas.get(entity.vault.as_str()) else {
            continue;
        };
        for rel in &entity.relationships {
            if validate_id_path_grammar(rel.target.path()).is_err() {
                to_drop.push((entity.id.clone(), rel.clone(), "grammar"));
                continue;
            }
            // Cross-vault-different edges validate against the
            // source schema's `cross_vault_relationships:` section,
            // not its intra-vault `relationships.definitions`. Same-
            // schema cross-vault and same-vault fall through to the
            // intra-vault path — matching the runtime relate flow's
            // routing rule.
            let target_vault = rel.target.vault();
            let target_schema = if entity.vault.as_str() == target_vault {
                None
            } else {
                schemas.get(target_vault).cloned()
            };
            let target_schema_ref: Option<SchemaRef> = target_schema.as_ref().map(|s| {
                let (name, version) = s.id();
                SchemaRef::new(name, version)
            });
            let cross_vault_different = match (&target_schema_ref, schema.id()) {
                (Some(target), (src_name, _)) => target.name != src_name,
                (None, _) => false,
            };
            let target_type = store
                .get(&rel.target)
                .map(|e| e.entity_type.clone())
                .filter(|t| !t.is_empty());
            if cross_vault_different {
                let target_ref = target_schema_ref.as_ref().expect("present when different");
                match validate_cross_vault_edge(
                    &rel.rel_type,
                    entity.entity_type.as_str(),
                    target_type.as_deref(),
                    schema.as_ref(),
                    target_ref,
                ) {
                    CrossVaultRelCheck::Ok => {}
                    CrossVaultRelCheck::EdgeNotDeclared => {
                        to_drop.push((
                            entity.id.clone(),
                            rel.clone(),
                            "cross_vault_not_declared",
                        ));
                        continue;
                    }
                    CrossVaultRelCheck::Invalid(_) => {
                        // Same drop semantics as the intra-vault
                        // shape/vocabulary branch — boot is silent
                        // best-effort cleanup.
                        to_drop.push((entity.id.clone(), rel.clone(), "cross_vault_shape"));
                        continue;
                    }
                }
            } else {
                if validate_rel_type(&rel.rel_type, schema.as_ref()).is_err() {
                    to_drop.push((entity.id.clone(), rel.clone(), "unknown_rel_type"));
                    continue;
                }
                if validate_rel_shape(
                    &rel.rel_type,
                    entity.entity_type.as_str(),
                    target_type.as_deref(),
                    schema.as_ref(),
                )
                .is_err()
                {
                    to_drop.push((entity.id.clone(), rel.clone(), "shape"));
                    continue;
                }
            }
        }
    }
    for (from_id, rel, reason) in to_drop {
        let origin = origin_for(from_id.vault()).to_string();
        store.remove_edge(&from_id, &rel.target, &rel.rel_type);
        if let Some(entity) = store.get_mut(&from_id) {
            entity
                .relationships
                .retain(|r| !(r.rel_type == rel.rel_type && r.target == rel.target));
        }
        let recovery = if origin == "writable" {
            Some(crate::ops::ParsedRelationRecovery::remove_explicit_relation(
                from_id.clone(),
                rel.target.clone(),
                rel.rel_type.clone(),
            ))
        } else {
            None
        };
        warnings.push(WarningHint::ParsedRelationInvalid {
            entity_id: from_id,
            rel_type: rel.rel_type,
            target: rel.target,
            reason: reason.to_string(),
            origin,
            recovery,
        });
    }

    // Pass 1b: per-edge description posture against the rel-type's
    // schema declaration. Forbidden + description present → drop the
    // description in-memory and warn; the next render normalises the
    // row to the simple form. Required + description absent → warn
    // and leave the relation intact; the operator's follow-up
    // mutation (or a hand-edit using the em-dash delimiter) supplies
    // the text. Runs after the shape drops so the surviving
    // relationships have known-valid rel-types in this schema.
    {
        use memstead_schema::PerEdgeDescription;
        let mut posture_warnings: Vec<WarningHint> = Vec::new();
        let mut to_strip_description: Vec<(EntityId, String, EntityId)> = Vec::new();
        for entity in store.all_entities() {
            if entity.stub {
                continue;
            }
            let Some(schema) = schemas.get(entity.vault.as_str()) else {
                continue;
            };
            for rel in &entity.relationships {
                // Look up the posture in the routing-appropriate
                // definition. Cross-vault-different routes through
                // the source schema's cross_vault_relationships entry
                // for the target schema; intra-vault and same-schema
                // cross-vault fall through to the intra-vault
                // relationships.definitions.
                let target_vault = rel.target.vault();
                let target_schema = if entity.vault.as_str() == target_vault {
                    None
                } else {
                    schemas.get(target_vault).cloned()
                };
                let target_schema_ref: Option<SchemaRef> =
                    target_schema.as_ref().map(|s| {
                        let (name, version) = s.id();
                        SchemaRef::new(name, version)
                    });
                let cross_vault_different = match (&target_schema_ref, schema.id()) {
                    (Some(target), (src_name, _)) => target.name != src_name,
                    (None, _) => false,
                };
                let posture = if cross_vault_different {
                    let target_ref = target_schema_ref
                        .as_ref()
                        .expect("target_schema_ref is Some when cross_vault_different");
                    schema
                        .cross_vault_entry(&target_ref.name)
                        .and_then(|entry| {
                            entry.definitions.iter().find(|d| d.name == rel.rel_type)
                        })
                        .map(|d| d.per_edge_description)
                } else {
                    schema
                        .relationship_def(&rel.rel_type)
                        .map(|d| d.per_edge_description)
                };
                match posture {
                    Some(PerEdgeDescription::Required) if rel.description.is_none() => {
                        posture_warnings.push(
                            WarningHint::ParseMissingRequiredDescription {
                                from: entity.id.clone(),
                                rel_type: rel.rel_type.clone(),
                                target: rel.target.clone(),
                            },
                        );
                    }
                    Some(PerEdgeDescription::Forbidden) if rel.description.is_some() => {
                        posture_warnings.push(
                            WarningHint::ParseDescriptionNotPermitted {
                                from: entity.id.clone(),
                                rel_type: rel.rel_type.clone(),
                                target: rel.target.clone(),
                            },
                        );
                        to_strip_description.push((
                            entity.id.clone(),
                            rel.rel_type.clone(),
                            rel.target.clone(),
                        ));
                    }
                    _ => {}
                }
            }
        }
        // Apply the description-strip in a second pass to avoid
        // borrowing the store mutably while iterating it.
        for (from_id, rel_type, target) in to_strip_description {
            if let Some(entity) = store.get_mut(&from_id) {
                for rel in entity.relationships.iter_mut() {
                    if rel.rel_type == rel_type && rel.target == target {
                        rel.description = None;
                    }
                }
            }
        }
        warnings.extend(posture_warnings);
    }

    // Pass 2: cycle detection per acyclic rel-type. Runs after the
    // schema-shape drops above so the input subgraph is already
    // schema-clean; cycles closed by edges that pass shape are the
    // residual hazard hand-edits can produce. Single pass per
    // rel-type — for each acyclic rel-type, build the workspace-wide
    // adjacency list of edges whose source vault declares that
    // rel-type as acyclic, then DFS with three-color marking
    // (white / gray / black). On encountering a gray node from a
    // gray parent, the traversing edge is a back-edge — drop it and
    // continue. The chosen back-edge is the *latest-visited* edge
    // in the cycle, not the "earliest" or "structural" one. That's
    // intentionally stable: DFS order is determined by `EntityId`
    // hash iteration (`HashMap` keys), which is consistent within a
    // process. Different processes may pick different back-edges;
    // either way the cycle is broken and the agent sees a typed
    // warning naming the dropped relation.

    // Collect the union of acyclic rel-types declared by any schema
    // in this workspace.
    let mut acyclic_rel_types: Vec<String> = Vec::new();
    for schema in schemas.values() {
        for def in &schema.manifest.relationships.definitions {
            if def.acyclic && !acyclic_rel_types.contains(&def.name) {
                acyclic_rel_types.push(def.name.clone());
            }
        }
    }

    let mut cycle_drops: Vec<(EntityId, EntityId, String)> = Vec::new();
    for rel_type in &acyclic_rel_types {
        // Adjacency list scoped to this rel-type. Includes edges
        // whose source vault's schema declares the rel-type as
        // acyclic — a vault whose schema doesn't declare the type
        // acyclic shouldn't have its edges dropped just because a
        // sibling vault does.
        let mut adj: std::collections::HashMap<EntityId, Vec<EntityId>> =
            std::collections::HashMap::new();
        for entity in store.all_entities() {
            let Some(schema) = schemas.get(entity.vault.as_str()) else {
                continue;
            };
            if !schema.relationship_acyclic(rel_type) {
                continue;
            }
            for edge in store.outgoing(&entity.id) {
                if &edge.rel_type == rel_type {
                    adj.entry(entity.id.clone())
                        .or_default()
                        .push(edge.target.clone());
                }
            }
        }

        // Three-color DFS. Each entity is white initially. Push to
        // gray on entry; demote to black on full descent. A gray
        // child reached from a gray parent is a back-edge.
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Color {
            White,
            Gray,
            Black,
        }
        let mut color: std::collections::HashMap<EntityId, Color> =
            adj.keys().map(|k| (k.clone(), Color::White)).collect();
        // Stable iteration order — sort the seeds so the dropped
        // edge depends only on the workspace's id set, not on hash
        // iteration order.
        let mut seeds: Vec<EntityId> = adj.keys().cloned().collect();
        seeds.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
        for seed in seeds {
            if color.get(&seed).copied() != Some(Color::White) {
                continue;
            }
            // Iterative DFS to avoid stack blow-ups on deep graphs.
            // Stack entry: (node, sorted-adjacency-index, sorted-adjacency-snapshot).
            let mut stack: Vec<(EntityId, usize, Vec<EntityId>)> = Vec::new();
            let mut start_targets: Vec<EntityId> =
                adj.get(&seed).cloned().unwrap_or_default();
            start_targets.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
            color.insert(seed.clone(), Color::Gray);
            stack.push((seed.clone(), 0, start_targets));
            while let Some((node, idx, targets)) = stack.last_mut() {
                if *idx >= targets.len() {
                    let done = node.clone();
                    color.insert(done, Color::Black);
                    stack.pop();
                    continue;
                }
                let target = targets[*idx].clone();
                *idx += 1;
                let node_id = node.clone();
                match color.get(&target).copied() {
                    Some(Color::White) => {
                        let mut next_targets: Vec<EntityId> =
                            adj.get(&target).cloned().unwrap_or_default();
                        next_targets.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
                        color.insert(target.clone(), Color::Gray);
                        stack.push((target, 0, next_targets));
                    }
                    Some(Color::Gray) => {
                        // Back-edge — closes a cycle. Drop it.
                        cycle_drops.push((
                            node_id,
                            target,
                            rel_type.clone(),
                        ));
                    }
                    Some(Color::Black) | None => {
                        // Already fully explored or not in the
                        // subgraph — no cycle through this edge.
                    }
                }
            }
        }
    }

    for (from_id, target, rel_type) in cycle_drops {
        let origin = origin_for(from_id.vault()).to_string();
        store.remove_edge(&from_id, &target, &rel_type);
        if let Some(entity) = store.get_mut(&from_id) {
            entity
                .relationships
                .retain(|r| !(r.rel_type == rel_type && r.target == target));
        }
        let recovery = if origin == "writable" {
            Some(crate::ops::ParsedRelationRecovery::remove_explicit_relation(
                from_id.clone(),
                target.clone(),
                rel_type.clone(),
            ))
        } else {
            None
        };
        warnings.push(WarningHint::ParsedRelationInvalid {
            entity_id: from_id,
            rel_type,
            target,
            reason: "cycle".to_string(),
            origin,
            recovery,
        });
    }
}

/// Remap edge sources to reflect each source vault's
/// `alias_target_rel_type` schema pointer: edges whose `rel_type`
/// equals the pointer are flipped from `Explicit` to `BodyLink`.
/// Idempotent — running it repeatedly produces the same result.
///
/// The discriminator is store-side only (no entity-side field). Under
/// the schema-load coupling (Option C), the pointer rel-type is also
/// `manual_authoring: forbidden`, so the only path to an edge of that
/// rel-type is via the alias-synthesis pass — making this remap
/// uniform across the workspace once the test sweep completes.
///
/// During the transitional window (synthesis pass landed but the 5
/// built-ins not yet flipped to `manual_authoring: forbidden`),
/// explicit `memstead_relate type=REFERENCES` still works for tests, and
/// those edges will also be remapped to `BodyLink` here. The wire
/// shape distinguishes synthesised vs. explicit only through this
/// label, so the relabel is observable but harmless — no test
/// asserts the legacy `"explicit"` string for REFERENCES.
pub fn remap_alias_target_edge_sources(
    store: &mut Store,
    schemas: &std::collections::HashMap<String, std::sync::Arc<memstead_schema::Schema>>,
) {
    let mut remaps: Vec<(EntityId, EntityId, String)> = Vec::new();
    for entity in store.all_entities() {
        let Some(schema) = schemas.get(entity.vault.as_str()) else {
            continue;
        };
        let Some(pointer) = schema.alias_target_rel_type() else {
            continue;
        };
        for edge in store.outgoing(&entity.id) {
            if edge.rel_type == pointer && edge.source != EdgeSource::BodyLink {
                remaps.push((entity.id.clone(), edge.target.clone(), edge.rel_type.clone()));
            }
        }
    }
    for (from, to, rel_type) in remaps {
        store.add_edge(
            from,
            Edge {
                rel_type,
                target: to,
                source: EdgeSource::BodyLink,
            },
        );
    }
}

/// Minimal placeholder entity for a wiki-link target that has no
/// markdown file. Tagged `StubKind::LoadTime` — this constructor
/// fires from parser-driven paths (boot, reload, attach) where the
/// stub is auto-emitted from a wiki-link to a not-yet-present
/// target. Mutation paths that need `ForwardReference` /
/// `Residual` use the engine-internal `make_stub` in
/// `engine/mutation/mod.rs` which takes an explicit kind.
pub fn make_stub(id: EntityId) -> Entity {
    Entity {
        title: id.name().to_string(),
        entity_type: String::new(),
        vault: id.vault().to_string(),
        file_path: String::new(),
        metadata: IndexMap::new(),
        sections: IndexMap::new(),
        relationships: Vec::new(),
        content_hash: String::new(),
        stub: true,
        stub_kind: Some(crate::entity::StubKind::LoadTime),
        id,
        heading_spans: std::collections::HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Entity;
    use memstead_schema::type_by_name;

    fn default_fallback() -> std::sync::Arc<TypeDefinition> {
        type_by_name("spec").expect("spec type must exist")
    }

    fn real_entity(id_str: &str, sections: &[(&str, &str)]) -> ParseResult {
        let id = EntityId(id_str.to_string());
        let vault = id.vault().to_string();
        let mut sec = IndexMap::new();
        for (k, v) in sections {
            sec.insert(k.to_string(), v.to_string());
        }
        ParseResult {
            entity: Entity {
                title: id.name().to_string(),
                entity_type: "spec".to_string(),
                vault,
                file_path: format!("{}.md", id.name()),
                metadata: IndexMap::new(),
                sections: sec,
                relationships: Vec::new(),
                content_hash: "deadbeef00000000".to_string(),
                stub: false,
                stub_kind: None,
                id,
                heading_spans: std::collections::HashMap::new(),
            },
            inline_links: Vec::new(),
            parse_warnings: Vec::new(),
        }
    }

    /// Plugin-vault entity with `[[plugin--foo]]` in a section and a
    /// real `test-vault-plugin--foo` already in the store → warning
    /// fires with a populated `candidate_target` (same-vault bare-slug
    /// resolution, pass 2 of the two-pass resolver).
    #[test]
    fn nested_prefix_emits_warning_with_candidate() {
        let fallback = default_fallback();
        let mut store = Store::new();

        let target = real_entity("test-vault-plugin--foo", &[]);
        push_entities_into_store(&mut store, vec![target], &fallback, None);

        let author = real_entity(
            "test-vault-plugin--author",
            &[("constraints", "See [[plugin--foo]] for details.")],
        );
        let mut warnings = Vec::new();
        let vault_names = vec!["test-vault-plugin".to_string()];
        let known_suffixes = vec!["plugin".to_string()];
        push_entities_into_store(
            &mut store,
            vec![author],
            &fallback,
            Some(LoadCollector {
                warnings: &mut warnings,
                known_suffixes: &known_suffixes,
                vault_names: &vault_names,
            }),
        );

        assert_eq!(warnings.len(), 1, "one nested-prefix warning expected");
        match &warnings[0] {
            WarningHint::SuspiciousNestedPrefix {
                from,
                resolved_id,
                candidate_target,
                section,
            } => {
                assert_eq!(from.as_ref(), "test-vault-plugin--author");
                // Tier-0 resolves `[[plugin--foo]]` to `plugin--foo`
                // directly (not a phantom
                // `test-vault-plugin--plugin--foo`).
                assert_eq!(resolved_id.as_ref(), "plugin--foo");
                assert_eq!(
                    candidate_target.as_ref().map(|c| c.as_ref()),
                    Some("test-vault-plugin--foo")
                );
                assert_eq!(section, "constraints");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    /// #41 narrowing: a colon/dash cross-vault link whose target vault
    /// is itself a full roster member is legitimate — no nested-prefix
    /// warning, even though that vault name also appears as a known
    /// suffix. This is the macos→engine false positive the heuristic
    /// used to emit (the "did you mean" candidate equalled the resolved
    /// target — self-contradicting).
    #[test]
    fn nested_prefix_skips_when_target_is_a_real_vault() {
        let fallback = default_fallback();
        let mut store = Store::new();

        let target = real_entity("engine--foo", &[]);
        push_entities_into_store(&mut store, vec![target], &fallback, None);

        let author = real_entity(
            "macos--author",
            &[("constraints", "See [[engine--foo]] for details.")],
        );
        let mut warnings = Vec::new();
        let vault_names = vec!["macos".to_string(), "engine".to_string()];
        // `engine` is both a real vault AND its own last-segment suffix.
        let known_suffixes = vec!["macos".to_string(), "engine".to_string()];
        push_entities_into_store(
            &mut store,
            vec![author],
            &fallback,
            Some(LoadCollector {
                warnings: &mut warnings,
                known_suffixes: &known_suffixes,
                vault_names: &vault_names,
            }),
        );

        assert!(
            warnings.is_empty(),
            "a cross-vault link to a real vault must not warn: {warnings:?}"
        );
    }

    /// Same scenario but the candidate is missing — the warning still
    /// fires so the author sees drift, with `candidate_target: None`.
    #[test]
    fn nested_prefix_emits_warning_without_candidate() {
        let fallback = default_fallback();
        let mut store = Store::new();

        let author = real_entity(
            "test-vault-plugin--author",
            &[("constraints", "[[plugin--ghost]]")],
        );
        let mut warnings = Vec::new();
        let vault_names = vec!["test-vault-plugin".to_string()];
        let known_suffixes = vec!["plugin".to_string()];
        push_entities_into_store(
            &mut store,
            vec![author],
            &fallback,
            Some(LoadCollector {
                warnings: &mut warnings,
                known_suffixes: &known_suffixes,
                vault_names: &vault_names,
            }),
        );

        assert_eq!(warnings.len(), 1);
        match &warnings[0] {
            WarningHint::SuspiciousNestedPrefix {
                candidate_target, ..
            } => assert!(candidate_target.is_none()),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    /// Bare-slug link (`[[foo]]`) resolves to `<current-vault>--foo` —
    /// no nested prefix, no warning.
    #[test]
    fn non_nested_link_no_warning() {
        let fallback = default_fallback();
        let mut store = Store::new();

        let author = real_entity(
            "test-vault-plugin--author",
            &[("constraints", "[[foo]]")],
        );
        let mut warnings = Vec::new();
        let vault_names = vec!["test-vault-plugin".to_string()];
        let known_suffixes = vec!["plugin".to_string()];
        push_entities_into_store(
            &mut store,
            vec![author],
            &fallback,
            Some(LoadCollector {
                warnings: &mut warnings,
                known_suffixes: &known_suffixes,
                vault_names: &vault_names,
            }),
        );
        assert!(warnings.is_empty());
    }

    /// Fully-qualified cross-vault link resolves to a different vault's
    /// id, not `<current-vault>--<suffix>--...`, so no nested prefix.
    /// Note: `[[<vault>--slug]]` in the section body literally resolves
    /// via wiki_link_to_id to `<current>--<vault>--slug` (nested), so
    /// this pattern is ambiguous by construction — the detector fires
    /// with a candidate that points at the fully-qualified target.
    /// Callers should write the full id or bare slug, not
    /// `<vault>--slug` from outside that vault.
    #[test]
    fn cross_vault_qualified_fires_with_cross_vault_candidate() {
        let fallback = default_fallback();
        let mut store = Store::new();

        // Real entity in the engine vault.
        let target = real_entity("test-vault-engine--health", &[]);
        push_entities_into_store(&mut store, vec![target], &fallback, None);

        // Plugin-vault author writes `[[engine--health]]`.
        let author = real_entity(
            "test-vault-plugin--author",
            &[("purpose", "See [[engine--health]].")],
        );
        let mut warnings = Vec::new();
        let vault_names = vec![
            "test-vault-engine".to_string(),
            "test-vault-plugin".to_string(),
        ];
        let known_suffixes = vec!["engine".to_string(), "plugin".to_string()];
        push_entities_into_store(
            &mut store,
            vec![author],
            &fallback,
            Some(LoadCollector {
                warnings: &mut warnings,
                known_suffixes: &known_suffixes,
                vault_names: &vault_names,
            }),
        );
        assert_eq!(warnings.len(), 1);
        match &warnings[0] {
            WarningHint::SuspiciousNestedPrefix {
                candidate_target, ..
            } => {
                assert_eq!(
                    candidate_target.as_ref().map(|c| c.as_ref()),
                    Some("test-vault-engine--health"),
                    "cross-vault pass-1 must find the engine vault candidate"
                );
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    /// Two vaults sharing a last-segment suffix: both contribute to the
    /// known-suffix set, the warning fires, and `candidate_target` is
    /// the one that has a real entity. Locks the suffix-collision
    /// resolution semantics.
    #[test]
    fn suffix_collision_resolves_first_match() {
        let fallback = default_fallback();
        let mut store = Store::new();

        // Vault A = `alpha`, Vault B = `beta-alpha`, both have suffix "alpha".
        // Real entity lives in `beta-alpha--target`.
        let target = real_entity("beta-alpha--target", &[]);
        push_entities_into_store(&mut store, vec![target], &fallback, None);

        // An author in `beta-alpha` writes `[[alpha--target]]`.
        let author = real_entity(
            "beta-alpha--author",
            &[("purpose", "[[alpha--target]]")],
        );
        let mut warnings = Vec::new();
        let vault_names = vec!["alpha".to_string(), "beta-alpha".to_string()];
        let known_suffixes = vec!["alpha".to_string(), "alpha".to_string()]; // collision
        push_entities_into_store(
            &mut store,
            vec![author],
            &fallback,
            Some(LoadCollector {
                warnings: &mut warnings,
                known_suffixes: &known_suffixes,
                vault_names: &vault_names,
            }),
        );
        assert_eq!(warnings.len(), 1, "collision must not duplicate the warning");
        match &warnings[0] {
            WarningHint::SuspiciousNestedPrefix {
                candidate_target, ..
            } => {
                // Pass 1 cross-vault probe excludes `beta-alpha` (self),
                // probes `alpha` — no real entity there, so pass 2
                // falls back to same-vault bare-slug `beta-alpha--target`
                // which is real.
                assert_eq!(
                    candidate_target.as_ref().map(|c| c.as_ref()),
                    Some("beta-alpha--target")
                );
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
